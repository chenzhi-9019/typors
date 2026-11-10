# Design

## Context

`proposal.md` 说明了动机与范围；`specs/editor/rich-content-rendering/spec.md` 与 `specs/editor/hybrid-markdown-editing/spec.md` 是行为契约。这里只记录现状、约束与技术选择。

**现状（重构前）**

- 两个彼此独立的 crate：`typors/`（产品）与 `spike/`（M0 研究探针），各自带 `[workspace]` 空表，无根 workspace。
- 产品是单个二进制 crate，10 个模块共约 1.4 万行：`doc.rs`（1 145 行）把 Markdown 解析成块 + span 表；`render.rs`（2 936 行）做几何、折行、命中、缓存、dump；`view.rs`（2 752 行）是自绘 `Element` 与输入；`selection.rs` / `ime.rs` / `probe.rs` / `perf.rs` 各自 700–2 100 行；`main.rs` 负责模式分发。
- 解析用 `Parser::new_ext(text, Options::empty())`。因此 `$$E = mc^2$$` 只是普通段落文本；`Event::InlineMath` / `DisplayMath` 分支当前不可达。图片在 `Tag::Image` 分支只被当作 link 样式，alt 文本可见，`![` 与 `](...)` 是零宽 marker。
- 每块只有一个 `line_height`；文本片段是 `DrawSpan`，绘制前由 `shape_frame` 生成 `TextCmd` / `QuadCmd`，`paint` 依次 `ShapedLine::paint` / `paint_quad`。
- 不变量：`Block::render` 与 `Block::spans` 铺满块；`RenderPos` 映射无损；可见投影 `src_to_render_offset` / `render_offset_to_src` 多对一但 canonical；所有位置落在 UTF-8 边界。
- 依赖刻意固定：`gpui-kit`（内部精确 pin `gpui-pre 0.3.9`）、`ropey =2.0.0-beta.1`、`pulldown-cmark`，另 `winresource` 是 build-dependency。
- `openspec/specs/` 为空：M0 change 的 tasks 全部完成但从未 archive。本 change 对 `editor/hybrid-markdown-editing` 的 MODIFIED delta 需要该 base spec，`openspec validate` 已给出 INFO：归档顺序必须先 M0、后本 change。

**已实测的技术事实（本机，2026-11-10）**

| 事实 | 证据 |
| --- | --- |
| `pulldown-cmark 0.13.5` 开启 `ENABLE_MATH` 后，行内/块级公式事件范围包含 `$` / `$$` 定界符，内容不含；`价格 $5 和 $6` 不产生公式 | 临时探针逐事件打印 range |
| `Start(Image)` / `End(Image)` 的 range 都覆盖完整 `![alt](dest "title")`（含尖括号写法）；alt 的 Text 事件嵌套在两者之间 | 同上 |
| RaTeX 0.1.15 可离线编译；`parse` → `layout` → `to_display_list` → `render_to_svg` 对 `E=mc^2`、分式、根号、求和、积分、矩阵、`\text{你好}` 均输出纯 `<path>` SVG，无 `<text>`；未知命令返回 `ParseError` | `%TEMP%\ratex-probe` 实跑，`cargo run` 通过 |
| `ratex-svg` 的 `standalone` + `embed-fonts` + `embed_glyphs: true` 把字形转成路径，KaTeX 字体资源约 0.5 MB | 同上；`ratex-katex-fonts` 解包 526 800 B |
| GPUI 有 `Window::paint_image` 与 `Window::paint_svg`（alpha mask + 单色，按 path+size 缓存于 sprite atlas）；`Window::use_asset` 异步加载并在完成时重绘当前 view；`Window::get_asset` 只查缓存；`Image::from_bytes` + `use_render_image` 可走内嵌字节解码 | `gpui-pre-0.3.9/src/window.rs:4983/5053/4280/4289`、`elements/img.rs`、`platform.rs:3221` |
| 默认平台提供基于 reqwest 的 `HttpClient`，`ImageAssetLoader` 对 `Resource::Uri` 走 HTTP；`Resource::Path` 直接读文件；`data:` URI 不走该加载器 | `gpui_pre_platform` 装配、`elements/img.rs:623-676` |
| `TransformationMatrix::default()` 存在；`Window::scale_factor()` 存在 | `scene.rs:702`、`window.rs:3091` |

## Goals / Non-Goals

**Goals:**

- 先把工程收敛为一个正常的 Rust workspace：所有成员在 `crates/`、包名前缀 `typors-`、依赖单向、公共 API 最小，大文件按职责拆开。
- `dev` 与 `release` 两套 profile 分别可调、可解释：日常构建可调试，发布构建小且快。
- 在不破坏既有映射/光标/选区/IME/性能契约的前提下，把图片与公式做成「原子富内容对象」接入现有自绘 Element。

**Non-Goals:**

- 不改变任何既有 Markdown 语法的可见行为（除本 change 明确新增的图片/公式）。
- 不做表格、原始 HTML、脚注、图表、导出、图片拖放/粘贴、公式编辑器、GIF 动画、远程磁盘缓存。
- 不做增量解析；仍以「编辑后整体重解析」为基线。
- 不引入异步运行时、webview、GPU 侧自定义 shader。
- 不追求 `cargo build` 首次冷构建时间最短；优先运行期体验与可调试性。

## 目标工程结构

### 成员与职责

| 成员 | 职责 | 允许依赖 |
| --- | --- | --- |
| `crates/typors-core` | `Document` / `Block` / `Span` / `ObjectKind`、解析、双向映射、`sample` 生成器。无 GPU、无窗口。 | `ropey`、`pulldown-cmark` |
| `crates/typors-media` | 图片引用解析与来源分类、`data:` 解码、RaTeX 封装（measure / SVG / 错误）、富内容缓存。无 GPUI 类型，可单测。 | `ratex-*`、`base64`；不依赖 core |
| `crates/typors-render` | 主题、`Metrics` 抽象、对象布局、折行、块几何、命中/坐标、块级缓存、布局报告与自检。 | `typors-core`、`gpui-kit`（仅类型/文本系统） |
| `crates/typors-editor` | `MarkdownView`、自绘 `Element`、输入/IME、选区、撤销、富内容资源加载与绘制命令。 | `typors-core`、`typors-render`、`typors-media`、`gpui-kit` |
| `crates/typors-probes` | `--dump-layout` / `--dump-media` / `--dump-cursor` / `--dump-edit` / `--dump-ime` / `--dump-select` / `--dump-newlines` / `--dump-perf` 的模式实现与断言。 | editor / render / core / media |
| `crates/typors-app` | 二进制 `typors`：`main`、窗口、模式分发、文件加载、资源嵌入（`build.rs`、`assets/`）。 | 全部 + `gpui-fps`、`winresource`（build） |
| `crates/typors-spike` | 原 `spike/` 的完整保留：M0 研究记录与独立回归参考。 | `gpui-kit`、`ropey`、`pulldown-cmark`；**不得依赖产品 crate** |

`sample.md`、`docs/`、`packaging/` 留在仓库根。二进制仍叫 `typors`，用 `crates/typors-app` 的 `[[bin]] name = "typors"` 保持 `target/release/typors.exe` 不变；安装脚本改为在仓库根执行 `cargo build --release -p typors-app`，payload 取 `target\release\typors.exe`。

### 依赖方向

```text
typors-core
   ↑
typors-render          typors-media（独立：不依赖 core / gpui）
   ↑                        ↑
   └──── typors-editor ─────┘
              ↑
        typors-probes
              ↑
        typors-app

typors-spike：独立成员，不依赖以上任何 crate
```

- `typors-render` **不**依赖 `typors-media`：它只定义 `MediaMeasures`（对象度量）与 `ObjectMetrics` 结构，由 `typors-editor` 用 `MathEngine + ImageRegistry` 实现并注入。这样 render 的单测可以用 `NoMedia` 跑，无需 RaTeX 或窗口。
- 任何 crate 都不允许反向依赖；`typors-core` 只依赖 `ropey` / `pulldown-cmark`，不得出现 gpui、窗口或图形依赖。用 `cargo tree` 在门禁里检查。

### 根清单与 profile

```toml
# Cargo.toml (virtual workspace)
[workspace]
resolver = "3"
members = ["crates/*"]

[workspace.package]
edition = "2024"
rust-version = "1.98"
version = "0.1.0"
publish = false

[workspace.dependencies]
gpui-kit = "=0.8.0"
gpui-fps = "=0.8.0"
pulldown-cmark = "=0.13.5"
ropey = "=2.0.0-beta.1"
ratex-parser = "=0.1.15"
ratex-layout = "=0.1.15"
ratex-svg = { version = "=0.1.15", features = ["standalone", "embed-fonts"] }
base64 = "=0.22.1"

# 日常开发：本 crate 轻优化 + 依赖全优化。gpui 在 opt-level=0 下滚动/文字
# shaping 会慢到无法判断性能问题，所以依赖单独拉满。
[profile.dev]
opt-level = 1
debug = true
debug-assertions = true
overflow-checks = true
incremental = true
strip = "none"

[profile.dev.package."*"]
opt-level = 2

# 发布：单 crate 优化 + 薄 LTO；符号剥离以缩小安装包。
[profile.release]
opt-level = 3
debug = false
debug-assertions = false
overflow-checks = false
incremental = false
lto = "thin"
codegen-units = 1
panic = "unwind"
strip = "symbols"

[profile.release.package."*"]
opt-level = 3
```

- `panic = "unwind"`：保留 `catch_unwind` 兜住 RaTeX 排版 panic（见 D7），也保留测试/backtrace 语义；不用 `abort`。
- `strip = "symbols"`：安装包优先；代价是崩溃 backtrace 无符号名。若后续接入崩溃上报，改成 `"debuginfo"` 并保留符号表。
- `lto = "thin"`：先在体积与构建时间之间取中；tasks 里会实测 `fat` 与 `thin` 的产物体积，若差异 > 1 MB 再切 `fat`。
- `opt-level = 1`（本 crate）：保留可断点、可逐行调试，又避免 debug 下模板/迭代器慢到不可用。
- 依赖版本集中在 `[workspace.dependencies]`，成员只写 `workspace = true`；`Cargo.lock` 提交，保证可复现。

### 代码质量规则（可执行，不靠口号）

- 每个 crate `#![forbid(unsafe_code)]`；库 crate `#![warn(missing_docs)]`。
- 公共 API 只在 `lib.rs` 显式 `pub use`；模块默认私有。禁止 `pub use crate::internals::*`。
- 库 crate 用 typed error（`MediaError`、`OpenError`），`anyhow` 只允许出现在 `typors-app` / `typors-probes`。
- 生产路径禁止 `unwrap` / `expect` / `panic!` / `todo!` / `unimplemented!`；`clippy::unwrap_used`、`expect_used`、`panic`、`todo`、`dbg_macro` 在库 crate 中 deny，探针与测试按需 `allow`。
- 一个文件只承担一个职责；目标 ≤ 600 行，函数 ≤ 80 行。重构后 `render` / `view` / `probe` / `selection` 必须按职责拆文件夹，而不是把 3 000 行继续堆着。
- I/O 与解码不占 UI 线程：图片走 GPUI asset 异步加载；公式排版是 CPU 计算，按 D10 缓存，超过预算再考虑后台预排版。
- 每个 crate 自带单测；跨 crate 行为用 `typors-probes` 的 `--dump-*` 作为回归证据。测试不得依赖网络，远程图片用本地 `data:` 或 `file://` 替身。

## Decisions

### D1 · workspace 拆分与「先搬后改」

把 `typors/` 拆成 6 个产品 crate + 1 个 spike crate，依赖方向固定为 `core ← render ← editor ← probes ← app`、`media ← editor`。拆分分两步：**第一步只做机械迁移**（文件移动、`pub(crate)` 提升、Cargo 清单），所有探针输出、单元测试、性能基线必须与重构前逐字节/逐项一致；第二步才按职责拆大文件。理由：几何/映射代码没有测试外的安全网，先行为后结构能把「重构引入的漂移」与「新功能 bug」分开。

- 备选：保留单 crate、只加根 workspace → 不满足「member 前缀 typors-」与低耦合目标，`doc` 仍无法脱离 GPU 单测。
- 备选：一次拆到最终形态 → diff 太大，无法判断探针变化来自结构还是逻辑。

### D2 · profile 的生产化配置

见上表。`dev` 与 `release` 的关键差异是 `opt-level`、`lto`、`codegen-units`、`strip`、`debug`。额外把依赖在 dev 下拉到 `opt-level = 2`，否则 gpui/wgpu 的 debug 构建会慢到无法测 H1。发布构建保留 `panic = "unwind"`，因为 D7 的 `catch_unwind` 是公式引擎的最后一道防线。

- 备选：`panic = "abort"` → 体积略小，但公式 panic 会直接终止进程，违背「非法公式不崩溃」的 spec。
- 备选：`codegen-units = 16` → 构建快，但热路径（布局/shaping）性能与体积都更差；发布版用 1，构建时间由 dev 承担。

### D3 · 富内容在文档模型里是「原子对象 span」

`SpanKind` 增加 `Object(ObjectKind)`：

```rust
pub enum ObjectKind {
    Image { destination: String, title: String, alt: String },
    Math { tex: String, display: bool },
}
```

`Span.src` 覆盖完整语法（图片含 `![`、`](...)`、title；公式含 `$`/`$$`），`Span.render` 是 `Block::render` 中**一个** U+FFFC（OBJECT REPLACEMENT CHARACTER，3 字节）。这样：

- `RenderPos` 的无损映射不变：`render_to_src` 仍返回 `span.src.start + offset_in_inline`，因此「每个源码偏移都能往返」的既有测试对对象照样成立。
- 可见投影是多对一：对象内所有源码偏移经 `src_to_render_offset` 折叠到占位符起点；`render_offset_to_src(占位符起点)` 按 canonical 规则返回 `span.src.start`，与 Marker 的语义一致。
- 光标不会停在对象「内部」的渲染位置：任何落入对象所在块的 caret 都让该块进入源码态；渲染态对象只作为选区端点的几何。

- 备选：新增 `BlockKind::Image/Math` → 块级对象可行，但行内公式/行内图片仍需要对象 span，最终要两套模型；统一为 span 更内聚。
- 备选：用零宽 marker 表示对象 → 无法承载尺寸，且违反「可见 span 铺满 render」。

### D4 · 解析：开启 math，图片在 Start/End 之间吞掉内部事件

- `Parser::new_ext(text, Options::ENABLE_MATH)`。已实测 `$5 和 $6` 不会被误判；`\$` 仍是转义文本。开启后既有文档里成对的 `$` 会开始按公式解释，这是本 change 的预期行为，README 与 sample 要说明。
- `Walk` 增加 `image_depth: usize` 与 `pending_image: Option<ImageStart>`：`Tag::Image` 进入时记下 `start`、`dest_url`、`title`，`depth += 1`，内部所有事件（alt 文本、嵌套强调）只累加 alt 字符串，不再生成 covered span；`TagEnd::Image` 时 `depth -= 1`，为 0 时 push 一个 object covered（`src = start..end`，`text = "\u{FFFC}"`）。嵌套在 link 里的图片仍工作，因为 link 的 style frame 在图片外层。
- `Event::InlineMath(s)` / `DisplayMath(s)` 改为 push object covered（range 已含定界符）。
- 解析阶段只存字符串，不做 I/O、不做 base64 解码、不做公式排版：解析仍可在每次编辑后整体重跑。

### D5 · 布局：对象参与流，块仍只有一个行高

- `Flow` 增加 `objects: &dyn MediaMeasures`。`Seg` 变为：
  - `Seg::Text { text, render, span_idx, style, underline }`
  - `Seg::Object { icon: ObjectKind, span_idx, render, metrics: ObjectMetrics }`
- 度量阶段先扫一遍本块对象，得到 `line_height = max(base_line_height, max(object.height))`；对象与文本共享该块的行高。**不引入逐行行高**：逐行行高会牵动 caret、选区、IME 候选框、滚动与 `line_tops`，收益不抵风险。
- 基线：`Metrics` 增加 `ascent(font_size, style)`（`WindowMetrics` 用 `ShapedLine::ascent`；`ApproxMetrics` 用 0.8 em）。块的 `baseline_offset = max(text_ascent, max(object.ascent))`，文本绘制 y = `line_top + baseline_offset - text_ascent`，行内对象 top = `baseline_offset - object.ascent`。对象是行内时按基线对齐，不是简单居中。
- 块级对象：段落可见内容恰好只有一个对象，且该对象是图片或 `display` 公式 → 居中、宽度 = `min(对象宽, content_width)`，该行行高 = 对象高 + `media_margin * 2`（上 12 px / 下 12 px）。`display` 对象即使夹在文本中也强制前后断行，按块级规则摆放；这种混排很少见，接受该块的其它行被同一行高抬高。
- 行内图片高度上限 = `3 × base_line_height`，超过则等比缩小；行内公式不设上限，但若宽度超过 `content_width`，缩放系数 = `content_width / width`。
- 源码态：带对象的块按普通文本行高排版（对象不绘制），块高相应变回文本高度。这是与 Typora 一致的行为：进入图片行就回到 Markdown 语义，离开再恢复。
- `BlockBox` 增加 `objects: Vec<ObjectBox>`：

```rust
pub struct ObjectBox {
    pub span_idx: usize,
    pub kind: ObjectKind,
    pub bounds: Bounds<Pixels>,
    pub baseline: Pixels,
    pub source: Range<usize>,   // 绝对源码范围
}
```

绘制命令在 `prepaint` 由 `ObjectBox` + 资源状态生成；布局层不持有 `Arc<RenderImage>`。

- 备选：逐行行高/浮动对象 → 需要重写 `line_tops`、`blank_line_*`、`caret_x`、`selection_rects`、IME bounds，回归面太大。
- 备选：行内图片固定行高、直接裁剪 → 会裁掉分式/图片内容，违背可见性。

### D6 · 图片：GPUI asset 系统负责字节、解码与缓存

`typors-media` 只做纯逻辑：

```rust
pub enum ImageRef {
    Local(PathBuf),                 // 相对/绝对，按文档目录解析
    Remote(String),                  // 仅 http / https
    Data { mime: String, bytes: Vec<u8> }, // base64 已解码
    Unsupported(String),
}
```

`data:` 的 base64 在 `ImageRegistry` 首次遇到时解码一次并缓存；解析阶段只保留原始字符串，避免每次按键解码 MB 级数据。

- 本地/远程：`Resource::Path(Arc<Path>)` / `Resource::Uri(SharedUri)`，`request_layout` 用 `window.get_asset::<ImgResourceLoader>` 只查缓存拿尺寸；`prepaint` 用 `window.use_asset` 触发/订阅加载并拿 `Arc<RenderImage>`。加载完成时 GPUI 会重绘当前 view，尺寸从占位切到真实值，只重排一次。
- `data:`：`Image::from_bytes(ImageFormat::from_mime_type(...).unwrap_or(guess), bytes)` → `Arc::new(image).use_render_image(window, cx)`；`Image::from_bytes` 的 id 是内容 hash，天然去重。
- 尺寸：`RenderImage::size(0)` 是设备像素；逻辑像素 = `size / window.scale_factor()`。显示宽 = `min(逻辑宽, content_width)`，高按比例；不放大。未知尺寸时占位 `min(content_width, 320) × 180`，虚线框 + alt 文本 + 「加载中…」。
- 失败：文件不存在、HTTP 非 2xx、解码失败、非 http(s) 的 scheme、`data:` mime 非 `image/*` → `Failed(reason)`；占位显示 alt 与原因，不删除源码。
- 绘制：`window.paint_image(bounds, bounds, Corners::default(), image, 0, false)`；v1 只画第 0 帧（GIF 不播放）。
- 备选：自己用 `image` crate + `RenderImage::new` → 多一个直接依赖、要自己做异步与缓存，且失去 GPUI 的 animation/缩略图能力。
- 备选：统一用 `RetainAllImageCache` → 仍要 `&mut Window`，且 `data:` 仍需单独处理；直接用 `use_asset` 更薄。

### D7 · 公式：RaTeX 纯 Rust 排版，SVG 路径交给 GPUI 绘制

`typors-media::MathEngine`（不依赖 GPUI，尺寸用 `f32` 逻辑像素，由 render 层转 `Pixels`）：

```rust
pub struct MathMetrics { pub width_px: f32, pub height_px: f32, pub depth_px: f32 }
pub fn measure(&self, tex: &str, font_size_px: f32) -> Result<MathMetrics, MathError>;
pub fn svg(&self, tex: &str, font_size_px: f32) -> Result<Arc<str>, MathError>;
```

- `measure`：`ratex_parser::parse` → `ratex_layout::layout(&nodes, &LayoutOptions::default())` → 取 `LayoutBox.width/height/depth`（em），乘 `font_size_px`。`padding = 0`，`width = 3.84 em × 20 px = 76.8 px` 这类换算已实测。
- `svg`：`to_display_list` → `ratex_svg::render_to_svg(&list, &SvgOptions { font_size, padding: 0.0, embed_glyphs: true, ..Default::default() })`；断言输出不含 `<text`（含则视为字体解析失败），`<path>` 与 `<image>` 均可被 usvg 解析。
- 绘制：`window.paint_svg(bounds, key, Some(svg.as_bytes()), TransformationMatrix::default(), color, cx)`。`key` 用 `format!("typors-math-{hash:016x}-{size_bits}")`，不是真实路径；因为 `data = Some(..)`，sprite atlas 首次解析后就按 `(path, size)` 命中，不会去读文件。公式颜色跟随正文颜色；v1 是单色 alpha mask，`\color{...}` 的自定义颜色会被忽略（记录为已知限制）。
- 基线：RaTeX 坐标系 y 向下、baseline 在 `y = height`；布局层用 `ascent = height`、`descent = depth` 对齐 D5 的基线模型。
- 失败：`parse` 的 `ParseError` 直接转 `MathError { message, loc }`；`layout`/`to_display_list` 外包 `catch_unwind`（`panic = "unwind"`），捕获后同样转错误。渲染态失败对象退回显示原始 `$...$` 文本 + 错误样式（`theme.math_error` 颜色 + 下划线），仍占一个对象盒；点击进入源码态后可正常编辑。**不允许** panic 冒到事件循环。
- 字体：`\text{中文}` 依赖 `ratex-unicode-font` 在系统字体里找 CJK（本机命中 `C:\Windows\Fonts\NotoSansSC-VF.ttf`）。找不到时字形缺失，按失败降级；不作为硬错误。
- 备选：Typst 排版 → 质量更高但依赖树与编译时间大一个量级，且要把整页排版塞进逐块布局。
- 备选：KaTeX + webview → 违背自绘架构，窗口内嵌 webview 的 IME/焦点会与现有输入通道冲突。
- 备选：`render_single_frame` 光栅化后用 `paint_image` → 支持彩色，但每个公式多一份位图缓存、缩放会糊；v1 用矢量，彩色需求出现再切。

### D8 · 绘制管线：对象作为独立的 prepaint 命令

`PrepaintState` 增加 `objects: Vec<ObjectCmd>`：

```rust
enum ObjectCmd {
    Image { bounds: Bounds<Pixels>, data: Arc<RenderImage> },
    Svg { bounds: Bounds<Pixels>, svg: Arc<str>, color: Rgba },
    Failed { bounds: Bounds<Pixels>, text: String },
}
```

- `shape_frame` 在 cull 之后为每个 `ObjectBox` 解析状态：已加载 → Image/Svg；加载中/失败 → Failed。`Failed` 用 `metrics.shaped` 生成一行占位文本，复用既有 `TextCmd`。
- `paint` 顺序：背景/选区 → 块装饰 → 对象 → 文本 → caret。对象在文本下方，行内对象与文本重叠时文本可读。
- 截断：对象整体与 viewport 相交才生成命令；culled 统计加入对象数，`TYPORS_CULL_LOG` 一并输出。
- 失败绘制：图片失败画虚线框 + alt + 原因；公式失败画源码 + 错误色。失败占位不缓存为成功，状态变化时重试一次（图片）或永久失败（公式解析错误在源码不变时不会变）。

### D9 · 命中、光标与选区

- `DocLayout::hit` 之前先查 `object_at(point)`：点在对象盒内 → 返回 `ObjectHit { block_idx, span_idx, right_half }`。view 把它转成源码偏移 `span.src.start`（左半）或 `span.src.end`（右半），设置 caret 并让该块进入源码态。对象是原子的，内部不产生 caret。
- 文本命中跳过对象盒；`hit` 的既有行为对非对象区域不变。
- 选区：`selection_rects` 增加对象矩形；选区只要与对象源码范围相交，就高亮整个对象盒（与图片/公式的原子语义一致）。跨对象复制仍走源码范围，得到完整 Markdown。
- `caret_x`：源码态按文本度量；渲染态 caret 不会落在对象内（该块已是源码态），因此不需要对象内部 x 映射。`line_tops` / `blank_line_*` 仍按统一行高工作。
- IME：候选框只依赖 `bounds_for_range` + `selected_text_range`。对象块在组合期间已是源码态，UTF-16 坐标走既有路径；无需为对象新增 IME 分支。

### D10 · 缓存与失效

- 图片：GPUI asset 系统是字节/解码的唯一真相；`ImageRegistry` 只缓存「来源 → 状态 + 逻辑尺寸」，状态迁移时 `media_generation += 1`。
- 公式：`MathEngine` 内 `Mutex<HashMap<MathKey, CachedMath>>`，`MathKey = (tex, font_size_bits)`；`CachedMath { metrics, svg: OnceLock<Arc<str>> }`。布局只要 metrics，SVG 在 prepaint 首次可见时才生成；同一公式滚动不重复 parse/layout/serialize。
- 块缓存：`BlockKey` 增加 `media_generation`；图片状态变化只让缓存整体失效一次（下一帧重排），实现简单且可接受。公式不引入 generation，因为 `tex + font_size + 主题字号` 已覆盖。
- 上限：`MathEngine` 最多保留 512 条，超出按插入序淘汰；文档切换（打开新文件）时清空。图片条目随 view 生命周期释放，`cx.drop_image` 只在显式 remove 时调用（v1 不做手动淘汰）。
- 备选：按块版本号只失效受影响块 → 图片加载完成会精确重排一块，但需要块级依赖追踪；v1 先整体失效一次，后续按 profiling 优化。

### D11 · 探针与可验证性

- 新增 `--dump-media`：打开真实窗口、绘制一帧，输出对象表（块号、类型、源码范围、来源、bounds、baseline、状态、绘制命令数），并断言：
  - M1 对象 `src` 落在块内、`render` 占位符长度正确、源码↔渲染往返全量成立；
  - M2 图片 bounds 符合「宽 ≤ 列宽、宽高比 = 内在比、小图不放大」；
  - M3 公式 bounds 与 `MathEngine::measure` 的误差 ≤ 0.5 px，baseline 与文本一致；
  - M4 缺失图片显示 Failed 占位、非法公式显示源码错误态，且进程不崩溃；
  - M5 已加载对象产生 ≥ 1 条绘制命令，culled 对象不产生命令。
- `--dump-layout` 报告增加 `objects` 段；既有 A1–A5 断言保持通过。
- 其余探针不改语义；`--dump-perf` 增加「含 50 公式的大文档」采样，作为预算证据。

### D12 · 文档、打包与 archive 顺序

- README 的 Layout / Build / Running 全部改到 workspace 路径：`cargo build --release -p typors-app`、`cargo run -p typors-app -- sample.md`，产物 `target/release/typors.exe`。
- `packaging/build-installer.ps1` 与 `typors.iss` 中 `typors\Cargo.toml`、`typors\target\release\typors.exe` 改为 workspace 根与 `target\release\typors.exe`；版本从 `crates/typors-app/Cargo.toml`（或 workspace.package）读取。
- `docs/packaging.md` 同步路径；`crates/typors-spike/README` 说明它仍是独立参考、不参与发布。
- archive 顺序：先 `openspec archive m0-hybrid-editor-spike` 让 `editor/hybrid-markdown-editing` 落到主 specs，再归档本 change；否则 MODIFIED delta 无 base 可合并（`validate` 的 INFO 已指出）。这属于发布流程，不阻塞实现。

## Risks / Trade-offs

- [一个 change 同时做重构与功能，diff 大] → Phase 0 只搬不改，探针/测试/性能基线必须与重构前逐项一致后才进入功能阶段；每个 phase 单独可回滚。
- [`window.use_asset` 在 `request_layout` 里调用是否受 GPUI 版本约束] → 当前源码无 `debug_assert_paint/prepaint`；若实测受限，改用 `cx.background_spawn` 自己读文件/解码，`ImageRegistry` 接口不变。
- [`paint_svg` 是单色 alpha mask，`\color` 丢失] → v1 记录为限制；需要彩色时切 `render_single_frame` + `paint_image`（`SvgRenderer` 已公开该 API）。
- [RaTeX 对边角语法/中文 `\text` 的字体回退不完美] → 失败一律降级为源码 + 错误样式；探针覆盖非法命令、矩阵、积分、CJK。
- [公式排版成本] → 按 `(tex, 字号)` 缓存 metrics/SVG；50 公式大文档的首次排版若超 100 ms，改为 idle 预排版（不改变结构）。
- [图片加载前后行高变化导致页面跳动] → 未知尺寸占位固定 16:9；加载完成只重排一次；本地小图通常在同一帧后完成。
- [远程图片无大小上限/超时由 GPUI 默认客户端决定] → 记录风险；v1 只接受 http(s)，后续可换成自带 `HttpClient` 包装做上限。
- [块行高被行内对象抬高，影响 100k 文档的布局缓存] → 对象度量进入块缓存键；不含对象的块零额外成本；`--dump-perf` 回归基线。
- [strip 后崩溃不可读] → 发布前保留一份未 strip 的 `typors.pdb` 归档（打包流程已有 pdb 产物）；需要现场诊断时用未 strip 构建。
- [打包脚本路径失效] → Phase 0 的任务包含脚本改路径并在本机跑一次 `build-installer.ps1 -WhatIf`（或至少 `ISCC` 前的 payload 检查）。

## Migration Plan

1. **Phase 0（结构）**：建根 workspace；把 `typors/` 机械拆成 `typors-core/render/media/editor/probes/app`（先按现有模块分，不拆文件），`spike/` → `crates/typors-spike`；根 profile 与 workspace 依赖；改 packaging 路径。验收：`cargo build --release -p typors-app`、`cargo test --workspace`、全部 `--dump-*` 探针 exit 0，输出与重构前一致。
2. **Phase 1（模型）**：core 增加 `ObjectKind` / `SpanKind::Object`、ENABLE_MATH、图片事件捕获、映射规则与单测。
3. **Phase 2（度量）**：media 的 `ImageRef` / `MathEngine` / 缓存；render 的 `MediaMeasures`、`Seg::Object`、行高/基线、`ObjectBox`、`object_at`、选区矩形；用 `NoMedia`/假引擎跑单测。
4. **Phase 3（绘制）**：editor 的 `ImageRegistry`（asset 加载、data URI）、`ObjectCmd`、`paint_image`/`paint_svg`、失败占位；接入 view。
5. **Phase 4（证据）**：`--dump-media`、`--dump-layout` 扩展、`--dump-perf` 公式样例、`sample.md` + 图片资源、README/docs/packaging 同步。
6. **回滚**：本 change 不迁移用户数据、不改文件格式；回滚到 Phase 0 之前的提交即可。若只回滚功能，保留 workspace 重构（Phase 0 独立提交）。

## Open Questions

- 表格渲染是否紧随其后？本 change 不做，spec 明确仍降级。
- GIF 动画是否需要？v1 只画首帧，等真实需求再开。
- 远程图片是否需要磁盘缓存与大小上限？当前只做内存缓存；若用户有大量网络图片再单开 change。
- `\color{...}` 等彩色公式是否需要？需要时从 `paint_svg` 切到 `render_single_frame` + `paint_image`，对象模型不变。
- 512 条公式缓存上限是否合适？用 `--dump-perf` 与内存观测调，不影响 specs/任务拆分。
