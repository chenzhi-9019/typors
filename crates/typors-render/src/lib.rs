//! typors render-projection geometry.
//!
//! Everything here is geometry only: block boxes, fragments, byte <-> x, hit
//! testing and the block/shape caches. Text measurement is abstracted behind
//! [`render::Metrics`] so the geometry can be built with the window text system or
//! headlessly with approximate metrics.

pub mod render;

pub use render::{
    ApproxMetrics, BlockBox, BlockCache, BlockDecor, BlockMode, DocLayout, DrawSpan, EditorTheme,
    LayoutMode, MediaMeasures, Metrics, NoMedia, ObjectBox, ObjectHit, ObjectMetrics, RenderHit,
    ShapeCache, Viewport, WindowMetrics, geometry_fingerprint, SOURCE_SPAN,
};
