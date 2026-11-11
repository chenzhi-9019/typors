//! Editor-side rich-content state: image loading through GPUI's asset system and
//! the math engine.
//!
//! The document layer knows *what* an image/formula is; this module knows how to
//! turn it into pixels. It keeps a small state machine per image source so the
//! layout can use a stable placeholder until the asynchronous decode completes,
//! then invalidates the block cache exactly once.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use gpui_kit::gpui::{
    App, DevicePixels, Image, ImageFormat, ImgResourceLoader, Pixels, RenderImage, Resource, Size,
    SvgRenderer, SvgSize, Window, px,
};
use typors_core::doc::{Document, ObjectKind, SpanKind};
use typors_media::mermaid::Diagram;
use typors_media::{ImageRef, MathEngine, MathMetrics, classify};
use typors_render::{MediaMeasures, ObjectMetrics};

/// A resolved image, ready to paint.
#[derive(Clone)]
pub enum ImageState {
    Pending,
    Ready {
        image: Arc<RenderImage>,
        /// Logical (CSS pixel) size after dividing by the display scale factor.
        width: f32,
        height: f32,
    },
    Failed(String),
}

/// One paintable object resolved for this frame.
pub enum ObjectPaint {
    Image {
        bounds: typors_render::ObjectBox,
        image: Arc<RenderImage>,
    },
    Svg {
        bounds: typors_render::ObjectBox,
        svg: Arc<str>,
    },
    Text {
        bounds: typors_render::ObjectBox,
        text: String,
    },
}

/// Image state plus the math engine, shared by layout and paint.
pub struct MediaContext {
    math: MathEngine,
    images: HashMap<String, ImageState>,
    diagrams: Mutex<HashMap<String, Result<Arc<Diagram>, String>>>,
    diagram_images: Mutex<HashMap<(u64, i32, i32), Arc<RenderImage>>>,
    doc_dir: Option<PathBuf>,
    generation: u64,
}

impl Default for MediaContext {
    fn default() -> Self {
        Self::new(None)
    }
}

impl MediaContext {
    pub fn new(doc_dir: Option<PathBuf>) -> Self {
        Self {
            math: MathEngine::new(),
            images: HashMap::new(),
            diagrams: Mutex::new(HashMap::new()),
            diagram_images: Mutex::new(HashMap::new()),
            doc_dir,
            generation: 0,
        }
    }

    pub fn generation(&self) -> u64 {
        self.generation
    }

    pub fn doc_dir(&self) -> Option<&std::path::Path> {
        self.doc_dir.as_deref()
    }

    pub fn math(&self) -> &MathEngine {
        &self.math
    }

    /// Render (and cache) a native Mermaid diagram.
    pub fn diagram(&self, source: &str) -> Result<Arc<Diagram>, String> {
        if let Ok(cache) = self.diagrams.lock()
            && let Some(hit) = cache.get(source)
        {
            return hit.clone();
        }
        let rendered = typors_media::mermaid::render(source)
            .map(Arc::new)
            .map_err(|error| error.to_string());
        if let Ok(mut cache) = self.diagrams.lock() {
            // Keep the cache bounded; a document rarely has hundreds of diagrams.
            if cache.len() > 64 {
                cache.clear();
            }
            cache.insert(source.to_string(), rendered.clone());
        }
        rendered
    }

    /// Rasterize a Mermaid SVG at the exact device size of its box, preserving
    /// Mermaid's own colours (unlike `paint_svg`'s monochrome alpha mask).
    pub fn diagram_image(
        &self,
        source: &str,
        width: i32,
        height: i32,
        renderer: &SvgRenderer,
    ) -> Option<Arc<RenderImage>> {
        let width = width.clamp(1, 8192);
        let height = height.clamp(1, 8192);
        let key = (hash_str(source), width, height);
        if let Ok(cache) = self.diagram_images.lock()
            && let Some(hit) = cache.get(&key)
        {
            return Some(hit.clone());
        }
        let diagram = self.diagram(source).ok()?;
        let parsed = renderer.parse_svg(diagram.svg.as_bytes()).ok()?;
        let image = renderer
            .render_parsed(
                &parsed,
                SvgSize::ExactSize(Size::new(DevicePixels(width), DevicePixels(height))),
            )
            .ok()?;
        if let Ok(mut cache) = self.diagram_images.lock() {
            if cache.len() > 32 {
                cache.clear();
            }
            cache.insert(key, image.clone());
        }
        Some(image)
    }

    pub fn image_state(&self, destination: &str) -> &ImageState {
        self.images
            .get(destination)
            .unwrap_or(&ImageState::Pending)
    }

    /// Look up already-decoded images and kick off loads for new ones. Called
    /// once per layout pass, before geometry is computed.
    pub fn refresh(&mut self, doc: &Document, window: &mut Window, cx: &mut App) {
        for block in doc.blocks() {
            for span in &block.spans {
                let SpanKind::Object(ObjectKind::Image { destination, .. }) = &span.kind else {
                    continue;
                };
                let next = measure_image(destination, self.doc_dir.as_deref(), window, cx);
                let should_replace = match self.images.get(destination) {
                    None => true,
                    Some(previous) => !same_state(previous, &next),
                };
                if should_replace {
                    self.generation = self.generation.saturating_add(1);
                    self.images.insert(destination.clone(), next);
                }
            }
        }
    }
}

fn hash_str(text: &str) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    text.hash(&mut hasher);
    hasher.finish()
}

fn same_state(a: &ImageState, b: &ImageState) -> bool {    match (a, b) {
        (ImageState::Pending, ImageState::Pending) => true,
        (ImageState::Failed(x), ImageState::Failed(y)) => x == y,
        (
            ImageState::Ready {
                width: aw,
                height: ah,
                image: ai,
            },
            ImageState::Ready {
                width: bw,
                height: bh,
                image: bi,
            },
        ) => ai.id == bi.id && aw.to_bits() == bw.to_bits() && ah.to_bits() == bh.to_bits(),
        _ => false,
    }
}

impl MediaMeasures for MediaContext {
    fn measure(&self, kind: &ObjectKind, font_size: Pixels) -> ObjectMetrics {
        match kind {
            ObjectKind::Math { tex, display } => match self.math.measure(tex, font_size.as_f32()) {
                Ok(metrics) => math_object_metrics(metrics, *display),
                Err(_) => failed_object_metrics(*display),
            },
            ObjectKind::Image { destination, .. } => match self.images.get(destination) {
                Some(ImageState::Ready {
                    width, height, ..
                }) => ObjectMetrics {
                    width: px(*width),
                    height: px(*height),
                    ascent: px(*height * 0.75),
                    descent: px(*height * 0.25),
                    display: false,
                },
                _ => placeholder_image_metrics(),
            },
            ObjectKind::Diagram { source } => match self.diagram(source) {
                Ok(diagram) => {
                    let width = diagram.width.clamp(80., 1200.);
                    let height = diagram.height.clamp(40., 1200.);
                    ObjectMetrics {
                        width: px(width),
                        height: px(height),
                        ascent: px(height * 0.75),
                        descent: px(height * 0.25),
                        display: true,
                    }
                }
                Err(_) => ObjectMetrics {
                    width: px(480.),
                    height: px(240.),
                    ascent: px(180.),
                    descent: px(60.),
                    display: true,
                },
            },
        }
    }
}

fn math_object_metrics(metrics: MathMetrics, display: bool) -> ObjectMetrics {
    ObjectMetrics {
        width: px(metrics.width_px),
        height: px(metrics.total_height_px()),
        ascent: px(metrics.height_px),
        descent: px(metrics.depth_px),
        display,
    }
}

fn failed_object_metrics(display: bool) -> ObjectMetrics {
    ObjectMetrics {
        width: px(if display { 240. } else { 80. }),
        height: px(if display { 48. } else { 22. }),
        ascent: px(if display { 36. } else { 17. }),
        descent: px(if display { 12. } else { 5. }),
        display,
    }
}

fn placeholder_image_metrics() -> ObjectMetrics {
    ObjectMetrics {
        width: px(320.),
        height: px(180.),
        ascent: px(135.),
        descent: px(45.),
        display: false,
    }
}

/// Convert the editor's image source into a GPUI resource.
fn resource_for(image: &ImageRef) -> Result<Resource, String> {
    match image {
        ImageRef::Local(path) => Ok(Resource::Path(path.clone().into())),
        ImageRef::Remote(uri) => Ok(Resource::Uri(uri.as_str().into())),
        ImageRef::Data { .. } => Err("data URIs are decoded directly".to_string()),
        ImageRef::Invalid(reason) => Err(reason.clone()),
    }
}

fn format_for_mime(mime: &str) -> ImageFormat {
    match mime.to_ascii_lowercase().as_str() {
        "image/jpeg" | "image/jpg" => ImageFormat::Jpeg,
        "image/gif" => ImageFormat::Gif,
        "image/webp" => ImageFormat::Webp,
        "image/bmp" => ImageFormat::Bmp,
        "image/tiff" => ImageFormat::Tiff,
        "image/x-icon" | "image/vnd.microsoft.icon" => ImageFormat::Ico,
        _ => ImageFormat::Png,
    }
}

/// Resolve one image for painting. Uses `use_asset`, so the first frame
/// subscribes the view and the decode completion re-draws it.
pub fn paint_image(
    destination: &str,
    doc_dir: Option<&std::path::Path>,
    window: &mut Window,
    cx: &mut App,
) -> Option<Arc<RenderImage>> {
    let image = classify(destination, doc_dir);
    match image {
        ImageRef::Local(_) | ImageRef::Remote(_) => {
            let resource = resource_for(&image).ok()?;
            window
                .use_asset::<ImgResourceLoader>(&resource, cx)
                .and_then(|result| result.ok())
        }
        ImageRef::Data { mime, bytes } => {
            let image = Image::from_bytes(format_for_mime(&mime), bytes);
            Arc::new(image).use_render_image(window, cx)
        }
        ImageRef::Invalid(_) => None,
    }
}

/// Peek at an image without subscribing, for the layout pass.
pub fn measure_image(
    destination: &str,
    doc_dir: Option<&std::path::Path>,
    window: &mut Window,
    cx: &mut App,
) -> ImageState {
    let image = classify(destination, doc_dir);
    match image {
        ImageRef::Local(_) | ImageRef::Remote(_) => {
            let Ok(resource) = resource_for(&image) else {
                return ImageState::Failed("unsupported image source".to_string());
            };
            match window.get_asset::<ImgResourceLoader>(&resource, cx) {
                Some(Ok(data)) => ready_state(data, window),
                Some(Err(error)) => ImageState::Failed(format!("{error:?}")),
                None => ImageState::Pending,
            }
        }
        ImageRef::Data { mime, bytes } => {
            let image = Image::from_bytes(format_for_mime(&mime), bytes);
            match Arc::new(image).get_render_image(window, cx) {
                Some(data) => ready_state(data, window),
                None => ImageState::Pending,
            }
        }
        ImageRef::Invalid(reason) => ImageState::Failed(reason),
    }
}

fn ready_state(data: Arc<RenderImage>, window: &Window) -> ImageState {
    let size = data.size(0);
    let scale = window.scale_factor().max(0.1);
    let width = size.width.0 as f32 / scale;
    let height = size.height.0 as f32 / scale;
    if width <= 0.0 || height <= 0.0 {
        return ImageState::Failed("image has zero size".to_string());
    }
    ImageState::Ready {
        image: data,
        width,
        height,
    }
}
