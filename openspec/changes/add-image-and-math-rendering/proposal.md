# Proposal

## Why

typors 目前把图片与数学公式当作「范围外语法」降级为纯文本：`$$E = mc^2$$` 和 `$x^2$` 原样显示，`![示例图](example.png)` 只显示 alt 文本。用户实测确认这两类内容不可读，而它们恰是技术写作最常用的元素，缺失会直接否掉「Typora 式写作体验」这一产品前提。

同时，此前「Rust 侧无成熟 LaTeX 等价物、大概要走内嵌 webview」的判断已被推翻：`gpui-pre 0.3.9` 已提供 `Window::paint_image` / `paint_svg`、异步资源加载与 SVG 渲染器；RaTeX 0.1.15 提供纯 Rust、离线、KaTeX 兼容的排版，并能输出自包含矢量路径。两项能力都已在本机编译验证（见 design.md 的调研记录）。

## What Changes

- **图片渲染**：解析 `![alt](src "title")`，在渲染态绘制真实图片。来源支持本地相对/绝对路径、`http(s)://` URL、`data:` URI；按内在宽高比等比缩放，宽度不超过正文列宽，默认不放大；居中显示。
- **公式渲染**：启用 `pulldown-cmark` 的 math 扩展，行内 `$...$` 与块级 `$$...$$` 由 RaTeX 排版，输出矢量路径经 GPUI 绘制；块级公式独占一行并居中。
- **对象化投影**：渲染态用原子占位符（U+FFFC）表示图片/公式；光标进入所在块时整块仍还原为 Markdown 源码；渲染偏移与源码偏移之间保持可逆，跨对象选区复制得到原始 Markdown。
- **布局**：行内对象参与折行、基线与行高计算（必要时抬高该块行高）；块级对象独占行、按内容宽度约束；页面高度把对象高度计入。
- **异步加载与降级**：图片走 GPUI 资源系统异步加载，加载中/失败显示占位（alt + 原因），不阻塞输入、不崩溃；公式解析失败回退为源码样式并给出可见错误标记。
- **缓存**：图片按来源、公式按（TeX 源 + 字号 + 主题）缓存解码/排版/SVG，避免每帧重解析；图片元数据解析与公式排版的缓存参与块级布局缓存键。
- **文档与探针**：`sample.md` 增加本地图片与行内/块级公式；新增 `--dump-media` 探针（对象几何、绘制命令、源码↔渲染往返、失败降级），并扩展 `--dump-layout` 报告。
- **工程结构（前置重构，不改变外部行为）**：把现有 `typors/` 与 `spike/` 收敛为根 workspace，成员全部位于 `crates/`、包名以 `typors-` 为前缀；按「文档模型 / 富内容引擎 / 排版 / 编辑器 / 探针 / 应用」拆分，依赖保持单向；根 `Cargo.toml` 统一 edition、依赖版本与 `dev` / `release` 两套 profile（opt-level、LTO、codegen-units、debug、strip）。该重构只改变工程组织，不新增 spec delta。
- **明确不做**：表格、原始 HTML、脚注、图表、导出、图片拖放/粘贴、公式可视化编辑器、远程图片磁盘缓存、GIF 动画、图片缩放语法（如 `{width=50%}`）。

## Capabilities

### New Capabilities

- `editor/rich-content-rendering`: 图片与数学公式从 Markdown 源码到屏幕像素的完整契约——来源解析、内在尺寸、布局与折行、原子对象与光标/选区映射、异步加载、失败降级、缓存与性能预算。

### Modified Capabilities

- `editor/hybrid-markdown-editing`: 「渲染态文档显示」的降级场景不再包含图片与数学公式（仅保留表格、图表、原始 HTML 等），并明确图片/公式改由 `editor/rich-content-rendering` 承担。

## Impact

- **工程根**：新增根 `Cargo.toml`（virtual workspace，`resolver = "3"`，`members = ["crates/*"]`），统一 `workspace.package` / `workspace.dependencies` 与 `profile.dev` / `profile.release`；`typors/` 与 `spike/` 目录被 `crates/typors-*` 取代。可执行文件仍为 `target/release/typors.exe`（`typors-app` 的 `[[bin]] name = "typors"`）。
- **代码**：`crates/typors-core`（doc 模型/解析/映射/sample）、`crates/typors-media`（图片引用与 RaTeX 引擎）、`crates/typors-render`（排版/几何/主题/度量）、`crates/typors-editor`（Element、输入、IME、选区）、`crates/typors-probes`（`--dump-*`）、`crates/typors-app`（窗口与 modes、资源嵌入）、`crates/typors-spike`（原 spike，保留为独立回归参考）；图片与公式引擎的实现落在 `typors-media` 与 `typors-editor`。
- **依赖**：新增 `ratex-parser` / `ratex-layout` / `ratex-svg = 0.1.15`（`ratex-svg` 启用 `standalone` + `embed-fonts`，字符转路径，自带 KaTeX 字体约 0.5 MB），`base64 = 0.22`（`data:` URI 解码；已在依赖树中）。不新增异步运行时、不引入 webview。`image` crate 不直接声明——解码走 GPUI 资源系统。所有版本集中在 `[workspace.dependencies]`。
- **产物体积**：预计增加 2–4 MB（RaTeX 字体 + 排版代码）；release 构建与启动预算需复测。
- **质量门禁**：`cargo fmt --all --check`、`cargo clippy --workspace --all-targets -- -D warnings`、`cargo test --workspace`、`cargo tree -d`（GPUI 只能有一份）、全部 `--dump-*` 探针 exit 0；重构前后探针输出与性能基线必须一致。
- **风险**：`gpui-pre` 是快照 API（`paint_svg` / `use_asset` / `paint_image` 的调用时机）；RaTeX 对 `\text{中文}` 依赖系统字体回退；远程图片加载失败与超时行为需实测；大模块拆分期间容易出现可见行为漂移，必须逐 crate 对照探针。
- **兼容性**：重构不改变外部行为；未包含图片/公式的文档行为与性能不变。`sample.md` 的「范围外语法」说明与 README 需同步更新；打包脚本若引用 `typors/target/...` 需改为 workspace `target/`。
