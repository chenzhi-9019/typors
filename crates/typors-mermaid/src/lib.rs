//! Lazy-loaded Mermaid renderer DLL.
//!
//! This crate exists only to keep merman (and its full diagram set) out of
//! `typors_runtime.dll`. The runtime loads `typors_mermaid.dll` on the first
//! Mermaid fence and calls this C ABI.

use std::ffi::{CString, c_char};

use merman::{OperationControl, RenderOutput, RenderRequest, Renderer, SvgRequest};

/// Render Mermaid source to an SVG. Returns a NUL-terminated UTF-8 string owned
/// by this DLL; `typors_mermaid_free` releases it. Null means an error.
///
/// # Safety
/// `source` must point to `len` readable bytes. `out_width`/`out_height` must be
/// valid writable floats (or null).
#[unsafe(no_mangle)]
pub unsafe extern "C" fn typors_mermaid_render(
    source: *const u8,
    len: usize,
    out_width: *mut f32,
    out_height: *mut f32,
) -> *mut c_char {
    let Ok(source) = std::panic::catch_unwind(|| {
        let bytes = unsafe { std::slice::from_raw_parts(source, len) };
        String::from_utf8_lossy(bytes).into_owned()
    }) else {
        return std::ptr::null_mut();
    };
    match render(&source) {
        Ok((svg, width, height)) => {
            if !out_width.is_null() {
                unsafe { *out_width = width };
            }
            if !out_height.is_null() {
                unsafe { *out_height = height };
            }
            match CString::new(svg) {
                Ok(c) => c.into_raw(),
                Err(_) => std::ptr::null_mut(),
            }
        }
        Err(_) => std::ptr::null_mut(),
    }
}

/// Free a string returned by [`typors_mermaid_render`].
///
/// # Safety
/// `ptr` must be null or a pointer returned by this DLL and not freed yet.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn typors_mermaid_free(ptr: *mut c_char) {
    if !ptr.is_null() {
        drop(unsafe { CString::from_raw(ptr) });
    }
}

/// Diagnostic entry point used by the runtime to verify the DLL version.
#[unsafe(no_mangle)]
pub extern "C" fn typors_mermaid_version() -> *const c_char {
    static VERSION: &[u8] = b"0.9.3\0";
    VERSION.as_ptr().cast()
}

fn render(source: &str) -> Result<(String, f32, f32), String> {
    let with_config = format!(
        "%%{{init: {{\"htmlLabels\": false, \"flowchart\": {{\"htmlLabels\": false}}}}}}%%\n{source}"
    );
    let output = Renderer::new()
        .render(RenderRequest::svg(
            &with_config,
            OperationControl::new(),
            SvgRequest::default(),
        ))
        .map_err(|error| error.to_string())?;
    let RenderOutput::Svg(Some(output)) = output else {
        return Err("source did not contain a supported Mermaid diagram".to_string());
    };
    let (svg, _) = output.into_parts();
    let (width, height) = svg_size(&svg).ok_or("Mermaid SVG has no width/height/viewBox")?;
    Ok((svg, width, height))
}

fn svg_size(svg: &str) -> Option<(f32, f32)> {
    let root_end = svg.find('>')?;
    let root = &svg[..root_end];
    if let (Some(w), Some(h)) = (attr(root, "width"), attr(root, "height"))
        && let (Some(w), Some(h)) = (parse_length(w), parse_length(h))
    {
        return Some((w, h));
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
    value
        .chars()
        .take_while(|c| c.is_ascii_digit() || *c == '.' || *c == '-')
        .collect::<String>()
        .parse()
        .ok()
}

#[cfg(test)]
mod tests {
    #[test]
    fn renders_flowchart_and_sequence() {
        let svg = super::render("flowchart LR\n A-->B").unwrap();
        assert_eq!(svg.1 > 0.0, true);
        let svg = super::render("sequenceDiagram\n A->>B: hi").unwrap();
        assert!(svg.0.contains("<text"));
    }

    #[test]
    fn bad_source_is_error() {
        assert!(super::render("not mermaid").is_err());
    }
}
