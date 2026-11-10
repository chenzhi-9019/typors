# Proposal

## Why

typors 的目标是用 Rust + gpui-kit 干净复刻 Typora 的写作体验。Typora 1.14.9 的核心不是"Markdown 编辑器"，而是**混合编辑模型**：文档常驻渲染态，光标所在行才还原为 Markdown 源码。gpui-kit 0.8.0 提供的是两件分离的东西——面向源码的代码编辑器、只读的 Markdown 渲染视图——没有这套混合模型。

因此有两个假设若不先验证就进入规划，风险会拖到 M1 才暴露：**（H1）** 同一份文本缓冲区能否同时投影为渲染态与光标行源码态，且重排开销可接受；**（H2）** gpui 自绘文本元素上，Windows 中文输入法是否可用（候选框定位、组合串、提交不丢字）。H2 比 H1 更危险，因为它是别人组件的边界行为，不是我们能重构的东西。

本 change 是一个时间盒 2–3 天的可行性 spike：用最小代码判定这两个假设，及伴生的 **（H3）** span 双向映射在 CJK 与嵌套强调下的保真度。结论决定 typors 的架构走向，而不是决定 typors 有多少功能。

## What Changes

- 为 typors 建立 OpenSpec 规范根（`openspec/`），后续所有 change 以此为家。
- 新增 `spike/` 独立 crate，**不属于最终架构**：一个窗口、单文档、混合渲染编辑的最小实现。
- 仅实现通过验收所需的最小行为：渲染态显示、光标行源码还原、IME 输入、选区往返、撤销重做、滚动性能。
- 产出 `spike/findings.md`：H1/H2/H3 逐个判定为"可用/不可用"，附耗时数字、复现步骤与失败模式。
- **明确不做**（出现即跑偏）：文件树、多标签、主题系统、表格编辑、数学公式、图表、图片、导出、i18n、设置页。
- **法务边界**：不复用 Typora 的 `lib.asar` 代码、`style/themes/*.css`、图标字体与自带字体；只做行为层复刻，素材与实现自出。

## Capabilities

### New Capabilities

- `editor/hybrid-markdown-editing`: 混合编辑内核——文档渲染态、光标行源码还原、源码与渲染片段的双向 span 映射、跨片段选区与撤销重做的一致性，以及重排性能预算。
- `editor/ime-text-input`: 自绘文本元素上的输入法交互——组合串显示、候选框定位到光标、提交与取消不损坏缓冲区。

### Modified Capabilities

（无。typors 当前没有既有 specs，此为本项目首批能力。）

## Impact

- **新增依赖**：`gpui-kit 0.8.0`（Apache-2.0，精确 pin `gpui-pre = 0.3.9`，edition 2024）、`ropey = 2.0.0-beta.1`（与 gpui-kit 对齐，避免同 crate 双份）、`pulldown-cmark`。
- **新增目录**：`openspec/`（规范根）、`spike/`（实验 crate，可随时丢弃）。
- **生产代码**：无。仓库当前为空，此 change 不触及任何既有代码。
- **工具链**：已核实本机 VS 18.3 MSVC x64 + rustc 1.98.1（msvc host）可用；无需额外安装。
- **决策价值**：spike 结论将决定 M1 采用"自绘混合 Element"还是退回"源码 + 分屏预览"，以及公式/图表是否必须走内嵌 webview。
