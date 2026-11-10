//! typors rich-content engines.
//!
//! * [`image`] classifies Markdown image destinations into local paths, http(s)
//!   URLs and decoded `data:` payloads.
//! * [`math`] turns LaTeX into box metrics and self-contained SVG through RaTeX.
//!
//! The crate is deliberately free of GPUI types so it can be unit-tested without a
//! window.

#![forbid(unsafe_code)]

pub mod image;
pub mod math;

pub use image::{ImageRef, classify};
pub use math::{DEFAULT_CACHE_CAPACITY, MathEngine, MathError, MathMetrics};
