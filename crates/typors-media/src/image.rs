//! Image-source classification: turn the Markdown destination string into the
//! thing the editor will hand to GPUI's asset system.
//!
//! This module performs no I/O and no decoding: local paths stay paths, remote
//! URLs stay URLs, and `data:` URIs are base64-decoded here because the GPUI
//! loader only understands paths and http(s) URLs.

use std::path::{Path, PathBuf};

use base64::Engine as _;

/// Where an image's bytes come from.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ImageRef {
    /// A path on disk. Relative destinations were resolved against the document
    /// directory; absolute destinations are kept as written.
    Local(PathBuf),
    /// An `http://` or `https://` URL, exactly as written.
    Remote(String),
    /// A `data:` URI whose payload has already been base64-decoded.
    Data { mime: String, bytes: Vec<u8> },
    /// A destination this build cannot load. The string is a user-facing reason.
    Invalid(String),
}

/// Classify one Markdown image destination.
///
/// `doc_dir` is the directory of the Markdown file; `None` means the document was
/// opened without a path (stdin/new document), in which case relative paths are
/// still kept as relative paths for the asset loader to try.
pub fn classify(destination: &str, doc_dir: Option<&Path>) -> ImageRef {
    let destination = destination.trim();
    if destination.is_empty() {
        return ImageRef::Invalid("empty image source".to_string());
    }
    if let Some(rest) = destination.strip_prefix("data:") {
        return classify_data_uri(rest);
    }
    if has_scheme(destination) {
        let lower = destination.to_ascii_lowercase();
        if lower.starts_with("http://") || lower.starts_with("https://") {
            return ImageRef::Remote(destination.to_string());
        }
        return ImageRef::Invalid(format!("unsupported image scheme: {destination}"));
    }
    let path = PathBuf::from(destination);
    let path = match (path.is_absolute(), doc_dir) {
        (true, _) => path,
        (false, Some(dir)) => dir.join(path),
        (false, None) => path,
    };
    ImageRef::Local(path)
}

/// A Windows drive path (`C:\...`, `C:/...`) is absolute, but `C:` also looks
/// like a URL scheme. Treat it as a path before the generic scheme check.
fn has_scheme(destination: &str) -> bool {
    let bytes = destination.as_bytes();
    if bytes.len() >= 2
        && bytes[0].is_ascii_alphabetic()
        && bytes[1] == b':'
        && (bytes.len() == 2 || bytes[2] == b'\\' || bytes[2] == b'/')
    {
        return false;
    }
    match destination.find("://") {
        Some(index) => index > 0,
        None => false,
    }
}

fn classify_data_uri(rest: &str) -> ImageRef {
    let Some((meta, payload)) = rest.split_once(',') else {
        return ImageRef::Invalid("malformed data URI: missing comma".to_string());
    };
    let mut parts = meta.split(';');
    let mime = parts.next().unwrap_or_default().to_string();
    if !mime.starts_with("image/") {
        return ImageRef::Invalid(format!("data URI is not an image: {mime}"));
    }
    let is_base64 = parts.any(|part| part.eq_ignore_ascii_case("base64"));
    if !is_base64 {
        return ImageRef::Invalid("data URI is not base64-encoded".to_string());
    }
    match base64::engine::general_purpose::STANDARD.decode(payload.trim()) {
        Ok(bytes) if !bytes.is_empty() => ImageRef::Data { mime, bytes },
        Ok(_) => ImageRef::Invalid("data URI payload is empty".to_string()),
        Err(error) => ImageRef::Invalid(format!("invalid base64 in data URI: {error}")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const ONE_PIXEL_PNG: &str = "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR42mP8z8BQDwAFgwJ/lw5GZQAAAABJRU5ErkJggg==";

    #[test]
    fn local_paths_resolve_against_the_document_directory() {
        let dir = Path::new("C:/docs");
        assert_eq!(
            classify("assets/a.png", Some(dir)),
            ImageRef::Local(PathBuf::from("C:/docs/assets/a.png"))
        );
        assert_eq!(
            classify("C:\\docs\\a.png", Some(dir)),
            ImageRef::Local(PathBuf::from("C:\\docs\\a.png"))
        );
        assert_eq!(
            classify("assets/a.png", None),
            ImageRef::Local(PathBuf::from("assets/a.png"))
        );
    }

    #[test]
    fn http_urls_stay_urls_and_other_schemes_are_invalid() {
        assert_eq!(
            classify("https://example.com/a.png", None),
            ImageRef::Remote("https://example.com/a.png".to_string())
        );
        assert!(matches!(
            classify("ftp://example.com/a.png", None),
            ImageRef::Invalid(_)
        ));
        assert!(matches!(classify("", None), ImageRef::Invalid(_)));
    }

    #[test]
    fn data_uris_are_decoded_and_validated() {
        let uri = format!("data:image/png;base64,{ONE_PIXEL_PNG}");
        match classify(&uri, None) {
            ImageRef::Data { mime, bytes } => {
                assert_eq!(mime, "image/png");
                assert_eq!(&bytes[..8], b"\x89PNG\r\n\x1a\n");
            }
            other => panic!("expected data URI, got {other:?}"),
        }
        assert!(matches!(
            classify("data:text/plain;base64,aGVsbG8=", None),
            ImageRef::Invalid(_)
        ));
        assert!(matches!(
            classify("data:image/png;base64,not base64!", None),
            ImageRef::Invalid(_)
        ));
        assert!(matches!(
            classify("data:image/png,raw", None),
            ImageRef::Invalid(_)
        ));
    }
}
