//! Native Mermaid rendering through `merman` (headless Rust Mermaid).
//!
//! This is the no-WebView path: Mermaid source is parsed, laid out and exported as
//! SVG entirely in process. Only the flowchart family is compiled in for now;
//! enabling more `diagram-*` features in `Cargo.toml` extends coverage without
//! changing this module.

use std::sync::Arc;

use merman::{OperationControl, RenderOutput, RenderRequest, Renderer, SvgRequest};

#[derive(Clone, Debug)]
pub struct Diagram {
    pub svg: Arc<str>,
    /// Logical width/height from the SVG root.
    pub width: f32,
    pub height: f32,
}

#[derive(Debug)]
pub struct DiagramError {
    pub message: String,
}

impl std::fmt::Display for DiagramError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.message)
    }
}

impl std::error::Error for DiagramError {}

/// Render one Mermaid fence body. `Err` means the fence is not a Mermaid diagram
/// this build can render; callers fall back to the source text.
pub fn render(source: &str) -> Result<Diagram, DiagramError> {
    // Mermaid's default flowchart labels are HTML (`<foreignObject>`), which
    // usvg/GPUI does not paint. Disable them so Merman emits real `<text>`
    // elements; sequence/class/state labels are already text.
    let with_config = format!(
        "%%{{init: {{\"htmlLabels\": false, \"flowchart\": {{\"htmlLabels\": false}}}}}}%%\n{source}"
    );
    let output = Renderer::new()
        .render(RenderRequest::svg(
            &with_config,
            OperationControl::new(),
            SvgRequest::default(),
        ))
        .map_err(|error| DiagramError {
            message: error.to_string(),
        })?;
    let RenderOutput::Svg(Some(output)) = output else {
        return Err(DiagramError {
            message: "source did not contain a supported Mermaid diagram".to_string(),
        });
    };
    let (svg, _evidence) = output.into_parts();
    let (width, height) = svg_size(&svg).ok_or_else(|| DiagramError {
        message: "Mermaid SVG has no width/height/viewBox".to_string(),
    })?;
    Ok(Diagram {
        svg: Arc::from(svg),
        width,
        height,
    })
}

fn svg_size(svg: &str) -> Option<(f32, f32)> {
    let root_end = svg.find('>')?;
    let root = &svg[..root_end];
    if let (Some(w), Some(h)) = (attr(root, "width"), attr(root, "height")) {
        if let (Some(w), Some(h)) = (parse_length(w), parse_length(h)) {
            return Some((w, h));
        }
    }
    let view_box = attr(root, "viewBox")?;
    let mut parts = view_box.split_whitespace();
    let _x = parts.next()?;
    let _y = parts.next()?;
    let w = parts.next()?.parse::<f32>().ok()?;
    let h = parts.next()?.parse::<f32>().ok()?;
    Some((w, h))
}

fn attr<'a>(root: &'a str, name: &str) -> Option<&'a str> {
    let needle = format!("{name}=\"");
    let start = root.find(&needle)? + needle.len();
    let rest = &root[start..];
    let end = rest.find('"')?;
    Some(&rest[..end])
}

fn parse_length(value: &str) -> Option<f32> {
    let number: String = value
        .chars()
        .take_while(|c| c.is_ascii_digit() || *c == '.' || *c == '-')
        .collect();
    number.parse().ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn renders_a_flowchart_as_svg() {
        let diagram = render("flowchart TD\n  A[Start] --> B[Done]").expect("diagram");
        assert!(diagram.svg.starts_with("<svg") || diagram.svg.contains("<svg"));
        assert!(diagram.width > 0.0 && diagram.height > 0.0);
        assert!(
            !diagram.svg.contains("<foreignObject"),
            "flowchart labels must be SVG text/paths, not HTML"
        );
        assert!(diagram.svg.contains("<text"));
    }

    #[test]
    fn non_mermaid_source_is_an_error_not_a_panic() {
        assert!(render("this is not mermaid").is_err());
    }

    #[test]
    fn all_compiled_diagram_families_render() {
        for source in [
            "sequenceDiagram\n  Alice->>Bob: Hi\n  Bob-->>Alice: Hello",
            "classDiagram\n  class Animal\n  Animal : +String name",
            "stateDiagram-v2\n  [*] --> Idle\n  Idle --> [*]",
            "erDiagram\n  CUSTOMER ||--o{ ORDER : places",
            "pie title Pets\n  \"Dogs\" : 3\n  \"Cats\" : 2",
            "gantt\n  title A plan\n  section One\n  Task :a1, 2026-01-01, 3d",
            "mindmap\n  root((root))\n    child",
        ] {
            let diagram = render(source).unwrap_or_else(|error| panic!("{source}: {error}"));
            assert!(diagram.width > 0.0 && diagram.height > 0.0, "{source}");
        }
    }
}
