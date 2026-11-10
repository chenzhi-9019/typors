# Tasks

> 顺序按依赖排列：Phase 0 只做工程结构与 profile，行为必须与现状一致；Phase 1–4 才是富内容功能。每个 group 自带该组的测试与文档，不把测试堆到最后。

## 1. Workspace 骨架与 profile

- [x] 1.1 新建根 `Cargo.toml`：virtual workspace、`resolver = "3"`、`members = ["crates/*"]`、`[workspace.package]`（edition 2024 / rust-version 1.98 / version 0.1.0 / publish false）与 `[workspace.dependencies]`（gpui-kit、gpui-fps、pulldown-cmark、ropey 精确 pin）。验证：`cargo metadata --no-deps` 能解析，members 列出 7 个 crate。
- [x] 1.2 建立 `crates/typors-core`、`typors-media`、`typors-render`、`typors-editor`、`typors-probes`、`typors-app`、`typors-spike` 的清单与最小 `lib.rs`/`main.rs`/`build.rs`；删除两个旧清单里的空 `[workspace]` 表（否则成员会被当成独立 workspace 根），包名统一 `typors-*`；`typors-app` 的 `[[bin]] name = "typors"`，`typors-spike` 的 `[[bin]] name = "spike"` 保持不变。验证：`cargo metadata --no-deps` 中 7 个包名均以 `typors-` 开头，`cargo build --workspace` 通过，`target/debug/typors.exe` 与 `target/debug/spike.exe` 存在。
- [x] 1.3 机械迁移现有产品代码：`doc.rs`/`sample.rs` → `typors-core`，`render.rs` → `typors-render`，`view.rs`/`ime.rs`/`selection.rs` → `typors-editor`，`perf.rs`/`probe.rs` → `typors-probes`，`main.rs`/`bin/dump.rs` → `typors-app`；只改 `use` 路径与 `pub(crate)` 可见性，不改逻辑；旧 `typors/` 目录保留到本组验证通过后再删除，`spike/` 移为 `crates/typors-spike`（源文件与测试原样）。验证：`cargo build --release -p typors-app` 成功，产物为 `target/release/typors.exe`。
- [x] 1.4 恢复全部既有测试与探针：`cargo test --workspace`（产品 30 个 + spike 30 个）全绿；依次运行 `--dump-layout`、`--dump-cursor`、`--dump-edit`、`--dump-ime`、`--dump-select`、`--dump-newlines`、`--dump-perf sample.md`，全部 exit 0。验证：把重构前后的探针输出各存一份到 `%TEMP%` 并 diff，除路径/计时行外逐行一致。
- [x] 1.5 配置两套 profile 并实测：`profile.dev`（opt-level 1、debug true、strip none、增量、`profile.dev.package."*"` opt-level 2）与 `profile.release`（opt-level 3、lto thin、codegen-units 1、debug false、strip symbols、panic unwind）。验证：`cargo build` 与 `cargo build --release` 均成功；记录 `target/release/typors.exe` 体积、`--dump-perf` 的 min/median/p95、debug 下窗口可断点；`cargo tree -d` 中 `gpui-pre` 只出现一份。
- [ ] 1.6 workspace 化打包脚本：更新 `packaging/build-installer.ps1` 与 `packaging/typors.iss` 的 crate 目录、payload 路径、版本来源与构建命令（仓库根 `cargo build --release -p typors-app`，payload `target\release\typors.exe`）；同步 `docs/packaging.md` 的路径与命令。验证：`pwsh packaging/build-installer.ps1 -PayloadPath target\release\typors.exe` 能通过路径存在检查并走到 ISCC 前一步；README 的 Build 命令改为 `cargo build --release -p typors-app` 并可原样运行。

## 2. 大文件拆分与质量门禁

- [ ] 2.1 拆 `typors-core`：`doc/` 下分 `model.rs`（Block/Span/RenderPos）、`parse.rs`（Walk/build_block/infer_marker）、`mapping.rs`（src_to_render/render_to_src/可见投影）、`tests.rs`；`sample.rs` 独立。验证：`cargo test -p typors-core` 全绿，且每个文件 ≤ 600 行。
- [ ] 2.2 拆 `typors-render`：`theme.rs`、`metrics.rs`、`flow.rs`（Seg/折行）、`block.rs`（异形块布局）、`geometry.rs`（hit/caret/line_tops）、`cache.rs`、`layout.rs`、`report.rs`（report/checks）。验证：`cargo test -p typors-render` 全绿，`render.rs` 不再作为单文件存在，每个文件 ≤ 600 行。
- [ ] 2.3 拆 `typors-editor`：`view/`（mod/render）、`element.rs`、`input/`（keyboard/ime/undo）、`selection.rs`。验证：`cargo test -p typors-editor` 全绿，`--dump-ime`/`--dump-select` 仍 exit 0，每个文件 ≤ 600 行。
- [ ] 2.4 拆 `typors-probes`：每个 `--dump-*` 一个模块（layout/media/cursor/edit/ime/select/newline/perf）+ `runner.rs` 分发。验证：`--dump-*` 全部 exit 0，输出与 1.4 保存的基线 diff 一致（新增 media 模式除外）。
- [ ] 2.5 落地质量门禁：各 crate `#![forbid(unsafe_code)]`、库 crate `#![warn(missing_docs)]`；workspace lints 对库 crate deny `clippy::unwrap_used`/`expect_used`/`panic`/`todo`/`unimplemented`/`dbg_macro`；`tools/check.ps1` 串起 `cargo fmt --all --check`、`cargo clippy --workspace --all-targets -- -D warnings`、`cargo test --workspace`。验证：`pwsh tools/check.ps1` 退出码 0；故意引入一个 `unwrap` 到库 crate 时 clippy 报错并阻止通过（临时验证后还原）。
- [ ] 2.6 记录结构基线：在 `docs/` 增加 `architecture.md`，画依赖方向图、列出每个 crate 的职责与禁止依赖、profile 取舍、门禁命令；验证：文档中的命令逐条可运行，且与根 `Cargo.toml` 实际配置一致。

## 3. 文档模型：对象 span（core）

- [x] 3.1 `SpanKind::Object(ObjectKind)` + `ObjectKind::{Image, Math}`；`build_block` 支持带占位符的 covered；`Block::render` 中对象占一个 U+FFFC；`locate`/`RenderPos` 保持无损映射。验证：新增单测覆盖对象样本的每个源码偏移 `src_to_render → render_to_src` 等于输入（含对象内部偏移），且全部落在 UTF-8 边界。
- [x] 3.2 解析开启 `Options::ENABLE_MATH`，`InlineMath`/`DisplayMath` 生成 `ObjectKind::Math`，`src` 含定界符、`tex` 不含。验证：单测断言 `$E=mc^2$`、`$$...$$`、多行 `$$` 的对象范围与 `tex`；`价格 $5 和 $6` 不产生对象；`\$` 保持文本。
- [x] 3.3 图片解析：`image_depth` + `pending_image` 捕获 `Start(Image)` 的 dest/title 与完整 range，内部 alt 文本只累加不生成普通 span，`End(Image)` 生成一个 Image 对象。验证：单测覆盖 `![a](x.png)`、title、`<my image.png>`、alt 内强调、link 内图片、连续两张图片；断言每个图片恰好一个对象 span、alt 正确、range 覆盖完整语法。
- [x] 3.4 扩展 100 份混合样本生成器：加入图片、行内公式、块级公式、缺失图片、非法公式；`out_of_scope_syntax_degrades_without_panic` 改为只覆盖表格/HTML（图片公式不再降级），并新增对象文档的 canonical reverse 测试。验证：`cargo test -p typors-core` 全绿，原有 20k+ 偏移往返断言仍成立。
- [ ] 3.5 为 `typors-core` 的公共 API 写 rustdoc（对象模型、映射语义、canonical 规则），并在 README 的「范围」段落说明图片/公式不再降级。验证：`cargo doc -p typors-core --no-deps` 无 warning；README 段落与 `sample.md` 的实际行为一致。

## 4. 富内容引擎（typors-media）

- [x] 4.1 `ImageRef` 来源解析：空串、相对/绝对路径（用文档目录解析）、`http(s)://`、`data:image/*;base64,...`、其它 scheme → `Unsupported`；`data:` 的 base64 解码与 mime 白名单。验证：单测覆盖大小写 scheme、Windows 盘符路径、带空格/中文路径、非法 base64、非 image mime；不访问网络。
- [x] 4.2 `MathEngine`：`measure(tex, font_size)` 与 `svg(tex, font_size)`，内部走 `ratex_parser::parse → ratex_layout::layout → to_display_list → ratex_svg::render_to_svg`，`padding = 0`、`embed_glyphs = true`；`parse` 错误与 `layout` panic 都转 `MathError`。验证：单测覆盖 `E=mc^2`、`\frac`、`\sqrt`、`\sum`、`\int`、矩阵、`\text{你好}`、`\badcommand{x}`；断言 SVG 含 `<path` 且不含 `<text`，尺寸在 em×font_size 的 1% 内。
- [x] 4.3 `MathEngine` 缓存：`(tex, font_size_bits)` 为键，metrics 与 SVG 分开缓存；上限 512、插入序淘汰；`svg` 在首次可见前不生成。验证：同一公式调用两次 `measure` 只 parse 一次（用内部计数或测试钩子）；两次 `svg` 返回同一 `Arc` 指针；超过上限后旧条目被淘汰。
- [x] 4.4 度量与错误呈现边界：公式超宽时给出缩放系数；空 `tex`、只有空白、超长 `tex`（如 10 万字符）不 panic、不长时间占用；`MathError` 带可展示的 message。验证：单测覆盖空/超长/NaN 尺寸与错误 Display；release 下 `catch_unwind` 生效（构造一个会 panic 的测试钩子，断言返回 Err 而不是进程退出）。
- [ ] 4.5 为 `typors-media` 写 rustdoc 与 README（支持的公式子集、字体依赖、`\text{中文}` 的系统字体回退、已知限制）。验证：`cargo doc -p typors-media --no-deps` 无 warning；README 命令可运行。

## 5. 排版：对象参与流（typors-render）

- [x] 5.1 `Metrics::ascent`：`WindowMetrics` 用 `ShapedLine::ascent`，`ApproxMetrics` 用 0.8 em；`Flow` 增加 `baseline_offset` 与按块计算的 `line_height`（`max(基础行高, 对象高)`）。验证：单测断言纯文本块的行高/基线不变（与重构前数值一致），含高行内对象的块行高变大且文本 y 随基线下移。
- [x] 5.2 `MediaMeasures` trait + `ObjectMetrics { width, height, ascent, descent }` + `NoMedia` 实现；`Seg::Object` 参与 `logical_lines` 与 `layout_lines` 的折行；`display` 对象前后强制断行并居中；单对象段落按块级媒体摆放。验证：单测覆盖文本夹公式折行、display 独占行、图片段落居中、行内图片超高被 3×行高上限截断缩放。
- [x] 5.3 `BlockBox.objects: Vec<ObjectBox>`，记录 span_idx、bounds、baseline、source、display；`report()` 输出对象段；`layout_is_consistent` 增补对象断言。验证：单测断言对象 bounds 在块内、宽高比正确、baseline 等于文本基线；`--dump-layout` 报告出现对象行且既有 A1–A5 仍 PASS。
- [x] 5.4 命中与选区：`object_at(point)`、`ObjectHit`（左/右半）；`hit` 跳过对象；`selection_rects` 对相交对象输出完整矩形；`caret_x` 在源码态走原路径。验证：单测点击对象左/右半分别得到源码 start/end；跨对象选区得到连续源码范围；对象内部不产生 caret。
- [x] 5.5 缓存失效：`BlockKey` 增加 `media_generation`；图片状态变化使缓存整体失效一次；公式不影响 generation。验证：单测模拟 generation 变化后几何重算、未变化时命中缓存；对象位移后 `block_at_y`/`hit` 自洽。
- [ ] 5.6 为 `typors-render` 写 rustdoc：`MediaMeasures` 契约、行高/基线模型、`ObjectBox` 坐标语义、缓存键约定。验证：`cargo doc -p typors-render --no-deps` 无 warning；`NoMedia` 的文档示例可编译（doctest）。

## 6. 绘制与资源（typors-editor）

- [x] 6.1 `ImageRegistry`：按来源缓存状态（Pending/Ready/Failed）与逻辑尺寸；本地/远程走 `window.get_asset::<ImgResourceLoader>`（布局查尺寸）与 `window.use_asset`（prepaint 订阅/取图）；`size / window.scale_factor()` 转逻辑像素。验证：单测纯逻辑（来源→Resource 映射、尺寸换算、上限）；集成用 `--dump-media` 验证本地图片加载后尺寸正确。
- [x] 6.2 `data:` URI：base64 解码 → `Image::from_bytes(ImageFormat, bytes)` → `Arc::new(image).use_render_image(window, cx)`；mime 不在白名单或解码失败 → Failed；缓存按内容 hash。验证：单测覆盖 png/jpeg 小样本 data URI 的字节解码；`--dump-media` 用内嵌 1×1 PNG 验证 Ready 与绘制命令。
- [x] 6.3 `ObjectCmd` + `shape_frame` 扩展：可见对象生成 Image/Svg/Failed 命令，图片 `paint_image`、公式 `paint_svg`，失败占位复用文本命令；cull 统计加入对象。验证：单测断言 viewport 外对象不生成命令、加载中生成 Failed、已加载生成 Image/Svg；`TYPORS_CULL_LOG=1` 输出对象计数。
- [x] 6.4 view 接线：`MarkdownView::new` 接收文档路径（`Option<PathBuf>` 作为相对路径基准）；view 持有 `ImageRegistry`、`MathEngine`、`media_generation`；`request_layout` 查尺寸、`prepaint` 取图并生成命令；点击对象走 `ObjectHit` → 源码 caret → 该块源码态。验证：手动打开 sample.md 点击图片左/右半，块切换为源码态且 caret 落在 `![`/`)` 边界；`--dump-cursor` 增加对象点击用例并通过。
- [x] 6.5 回归 IME 与选区：对象块进入源码态后 IME 组合串/候选框仍定位正确；跨对象选区/撤销重做不损坏 buffer。验证：`--dump-ime`、`--dump-select`、`--dump-newlines` exit 0；手动在图片块输入中文并撤销一次。
- [ ] 6.6 为 `typors-editor` 写 rustdoc（`MarkdownView` 生命周期、`ImageRegistry` 的 get/use 两阶段约定、`ObjectCmd` 的坐标语义、失败占位策略），并在 README 的 Layout 表加一行说明 editor 的职责。验证：`cargo doc -p typors-editor --no-deps` 无 warning；文档中的对象加载时序与实现一致。

## 7. 探针、样本与性能证据

- [x] 7.1 实现 `--dump-media`：打开窗口绘制一帧，输出对象表并断言 M1–M5（range/占位/往返、图片尺寸、公式尺寸与基线、失败降级、绘制命令与 cull）。验证：`cargo run --release -p typors-app -- --dump-media sample.md` exit 0，输出含 `[ok] M1`…`[ok] M5`。
- [x] 7.2 扩展 `--dump-layout`：报告增加 `objects` 段与每条对象的 bounds/baseline/source；A1–A5 断言保持不变。验证：`cargo run --release -p typors-app -- --dump-layout sample.md` exit 0，diff 中仅新增对象段与预期行高变化。
- [ ] 7.3 性能证据：`--dump-perf` 增加「10 万字符 + 50 公式」样本，断言首帧 ≤ 100 ms、采样帧 ≤ 16 ms、无 >100 ms 停顿；同时复跑无公式基线与重构前对比。验证：命令 exit 0；结果写进 `docs/architecture.md` 或本 change 的实现记录。
- [x] 7.4 样本与资源：`sample.md` 增加本地 `assets/sample-picture.png`（用 `tools/make-icon.ps1 -PreviewPath` 生成并提交）、`$E=mc^2$`、`$$\frac{a}{b}$$`、一个缺失图片路径、一个非法公式；README 的探针表加入 `--dump-media`。验证：运行窗口目视图片/公式/占位均正确；`--dump-media sample.md` 覆盖四种对象状态。
- [ ] 7.5 集成验收：`pwsh tools/check.ps1`、`cargo test --workspace`、全部 `--dump-*`、`cargo build --release -p typors-app`、`packaging/build-installer.ps1` 的 payload 检查一次通过；记录 release exe 体积与冷启动时间（≤1 s）。验证：输出归档到 `docs/architecture.md`；任一步失败回到对应 group 修复，不跳过。

## 8. 文档与发布配置收尾

- [ ] 8.1 更新 README 的 Layout/Build/Running/Probes/Dependencies 段落到 workspace 结构，说明 `typors-spike` 的地位与 `crates/typors-*` 的职责；同步 `docs/packaging.md` 与 `docs/spike/gpui-api-probe.md` 中失效的路径。验证：从 README 第一条命令开始逐条执行，全部可用。
- [ ] 8.2 记录 profile 决策：在 `docs/architecture.md` 写明 dev/release 的 opt-level、LTO、strip、panic 选择与实测体积/启动数据；说明 strip 对崩溃诊断的影响与替代方案。验证：文档数值与实际 `Get-Item target/release/typors.exe`、`--dump-perf` 输出一致。
- [ ] 8.3 检查「无坏味道」清单：每 crate 依赖方向、文件行数、公共 API 数量、`unwrap/expect` 生产路径零命中、无 `todo!`、无 `dbg!`、测试不依赖网络；把检查命令加入 `tools/check.ps1`。验证：`pwsh tools/check.ps1` 全绿；人为破坏一条规则能被脚本拦截（验证后还原）。

## 9. 用户追加（2026-11-11，超出原 42 项）

- [x] 9.1 删除线：启用 `ENABLE_STRIKETHROUGH`，`Style.strike` 与绘制横线。验证：`sample.md` 的 `~~已废弃~~` 在渲染态显示横线；`cargo test --workspace` 全绿。
- [x] 9.2 GFM 表格基础渲染：`ENABLE_TABLES`、`BlockKind::Table` + `TableData` 单元格范围、网格线与行/列布局。验证：`--dump-layout sample.md` 的 `[ 23] Table` 有 4 个单元格片段、bounds 非零、A1–A5 PASS。
- [x] 9.3 行内 HTML 安全子集：`<b>/<strong>/<i>/<em>/<code>/<s>/<a>/<br>` 映射到样式或换行，未知标签不破坏解析。验证：`sample.md` 的 `<b>粗体</b>`、`<i>斜体</i>`、`<br>` 正确显示；单元测试保持全绿。
- [x] 9.4 输入抖动：滚动锚定，同一光标块 top 变化时补偿 scroll offset。验证：`cargo build` 通过；`--dump-edit`/`--dump-select` exit 0；实机输入待用户复核。
- [ ] 9.5 mermaid：需要 webview 或原生 flowchart 子集，尚未实现。阻塞点：与 GPUI 自绘架构冲突，需确认方案。
- [ ] 9.6 块级 HTML：当前 `<div>` 等块级标签仍按纯文本/代码块降级；行内子集已完成，块级渲染需要 HTML/CSS 布局能力。

## Workflow follow-up

- 先归档 M0：`openspec archive m0-hybrid-editor-spike`，让 `editor/hybrid-markdown-editing` 进入主 specs；否则本 change 的 MODIFIED delta 在归档时没有 base（`validate` 已给出 INFO）。
- 再归档本 change：`openspec archive add-image-and-math-rendering`，并确认 `openspec/specs/editor/rich-content-rendering/spec.md` 与 `editor/hybrid-markdown-editing/spec.md` 内容正确。
- 归档后复跑 `openspec validate --all --strict`，并检查 `openspec/specs/` 中没有重复的旧 requirement。
