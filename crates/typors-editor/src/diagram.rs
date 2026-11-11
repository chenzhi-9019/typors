//! Lazy Mermaid DLL client.
//!
//! Merman is the largest dependency in the product. It lives in
//! `typors_mermaid.dll` so the runtime DLL and the editor stay small until a
//! document actually contains a Mermaid fence.

use std::ffi::{CStr, c_char};
use std::path::PathBuf;
use std::sync::Arc;

use libloading::Library;

type RenderFn = unsafe extern "C" fn(*const u8, usize, *mut f32, *mut f32) -> *mut c_char;
type FreeFn = unsafe extern "C" fn(*mut c_char);

#[derive(Clone, Debug)]
pub struct Diagram {
    pub svg: Arc<str>,
    pub width: f32,
    pub height: f32,
}

struct Api {
    _library: Library,
    render: RenderFn,
    free: FreeFn,
}

pub struct DiagramEngine {
    api: Option<Result<Api, String>>,
}

impl Default for DiagramEngine {
    fn default() -> Self {
        Self::new()
    }
}

impl DiagramEngine {
    pub fn new() -> Self {
        Self { api: None }
    }

    fn api(&mut self) -> Result<&Api, String> {
        if self.api.is_none() {
            self.api = Some(load());
        }
        match self.api.as_ref().expect("just initialized") {
            Ok(api) => Ok(api),
            Err(error) => Err(error.clone()),
        }
    }

    pub fn render(&mut self, source: &str) -> Result<Diagram, String> {
        let api = self.api()?;
        let ptr = unsafe {
            (api.render)(
                source.as_ptr(),
                source.len(),
                std::ptr::null_mut(),
                std::ptr::null_mut(),
            )
        };
        if ptr.is_null() {
            return Err("typors_mermaid.dll could not render this diagram".to_string());
        }
        let svg = unsafe { CStr::from_ptr(ptr) }
            .to_string_lossy()
            .into_owned();
        unsafe { (api.free)(ptr) };
        let (width, height) = svg_size(&svg).ok_or("Mermaid SVG has no size")?;
        Ok(Diagram {
            svg: Arc::from(svg),
            width,
            height,
        })
    }
}

fn dll_candidates() -> Vec<PathBuf> {
    let mut paths = Vec::new();
    if let Ok(exe) = std::env::current_exe()
        && let Some(dir) = exe.parent()
    {
        paths.push(dir.join("typors_mermaid.dll"));
    }
    paths.push(PathBuf::from("typors_mermaid.dll"));
    paths
}

fn load() -> Result<Api, String> {
    let mut last = String::new();
    for path in dll_candidates() {
        let library = match unsafe { Library::new(&path) } {
            Ok(library) => library,
            Err(error) => {
                last = format!("{}: {error}", path.display());
                continue;
            }
        };
        let render = unsafe { library.get::<RenderFn>(b"typors_mermaid_render\0") }
            .map(|symbol| *symbol)
            .map_err(|error| error.to_string());
        let free = unsafe { library.get::<FreeFn>(b"typors_mermaid_free\0") }
            .map(|symbol| *symbol)
            .map_err(|error| error.to_string());
        match (render, free) {
            (Ok(render), Ok(free)) => {
                return Ok(Api {
                    _library: library,
                    render,
                    free,
                });
            }
            (Err(error), _) | (_, Err(error)) => last = error,
        }
    }
    Err(format!(
        "typors_mermaid.dll not loaded (build `cargo build -p typors-mermaid`): {last}"
    ))
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
    Some((parts.next()?.parse().ok()?, parts.next()?.parse().ok()?))
}

fn attr<'a>(root: &'a str, name: &str) -> Option<&'a str> {
    let needle = format!("{name}=\"");
    let start = root.find(&needle)? + needle.len();
    let rest = &root[start..];
    Some(&rest[..rest.find('"')?])
}

fn parse_length(value: &str) -> Option<f32> {
    value
        .chars()
        .take_while(|c| c.is_ascii_digit() || *c == '.' || *c == '-')
        .collect::<String>()
        .parse()
        .ok()
}

impl std::fmt::Debug for DiagramEngine {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("DiagramEngine")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn missing_or_present_dll_does_not_panic() {
        let mut engine = DiagramEngine::new();
        let _ = engine.render("flowchart LR\n A-->B");
    }

    #[test]
    fn size_parser_handles_viewbox() {
        assert_eq!(
            svg_size(r#"<svg viewBox="0 0 120 80"></svg>"#),
            Some((120.0, 80.0))
        );
    }
}
