# Design

## Context

动机见 `proposal.md` — Why；行为契约见 `specs/editor/*`。这里只记约束与技术选择。

已核实的本机与依赖事实：

- `gpui-kit 0.8.0`（Apache-2.0）在 crates.io 上，内部精确 pin `gpui = { package = "gpui-pre", version = "=0.3.9" }`，edition 2024，并 re-export GPUI / base / component / assets。
- `gpui-pre 0.3.9` 是 Zed 的 `gpui` 快照（`zed@93d7456`），不是稳定 API；`gpui-fast` 带 `windows-manifest-provided-by-host`，Windows 为一等目标。
- 本机：rustc 1.98.1 msvc、VS 18.3 MSVC x64、148 GB 磁盘，`gpui-pre` 已入 cargo 缓存。
- 约束：**不复用** Typora 的 `lib.asar`、`style/themes/*.css`、图标字体与自带字体。

## Goals / Non-Goals

**Goals:**

- 用一个自绘文本元素回答 H1（单缓冲双投影可行）、H2（Windows IME 可用）、H3（span 映射保真）三个问题。
- 让结论可复现：每条结论都有可重跑的命令与可读数字。
- 让代码可丢弃：spike 不背架构债，不追求抽象。

**Non-Goals:**

- 不做可维护的模块边界——spike 允许单文件 800 行。
- 不做主题系统、字体度量策略、磁盘配置持久化。
- 不做增量解析（每次编辑全量重解析 AST，只要落在 16 ms 预算内即可）。

## Decisions

### D1 · 依赖取 crates.io 的 `gpui-kit 0.8.0`，GPUI 一律走它的 re-export

理由：`gpui-kit` 把 `gpui-pre` 锁在精确快照上，weekly release 不会把应用甩到不兼容的快照（上游为此外加过 CI 检查）。我们把 `gpui` 类型从 `gpui_kit::gpui` 取，**不单独声明 `gpui-pre` 依赖**，避免同 crate 出现两份、类型不互通。

- 备选：git 依赖 main → 最新但不可复现，且 main 随时改快照。
- 备选：直接依赖 `gpui 0.2.2`（crates.io 上的另一条线）→ 与 gpui-kit 的内部快照类型不同，无法混用。

### D2 · 混合编辑由自绘 `Element` 承担，不复用 gpui-kit 的 Editor 或 TextView

`gpui-component` 提供的是分离的两件东西：面向源码的代码编辑器、只读的 Markdown 渲染视图。混合模型的本质是**同一缓冲区有两种投影，且投影边界随光标移动**——它不是二者之一的超集。

```
            单缓冲 (ropey)
                  |
        +---------+---------+
        v                   v
   块级解析 (AST)      光标所在块的源码态
        |                   |
        +--------+----------+
                 v
        自绘 Element 的布局树
        (request_layout / prepaint / paint)
```

- 备选：在 Editor 上叠加渲染层 → 与 Editor 内建行高、光标、选区冲突，且无法把"渲染片段"作为命中区。
- 备选：TextView + 隐藏输入框 → 无法编辑，光标不可见。
- 代价：光标、选区、滚动、IME 全部自己接。这正是 spike 要付的账。

### D3 · 数据模型：rope 单缓冲 + 块级 AST + 块内 span 表

- 缓冲区 `ropey::Rope`（版本对齐 gpui-kit 的 `=2.0.0-beta.1`，避免同 crate 双份）。
- 解析用 `pulldown-cmark`，产出块列表，每块记 `(src_range, kind)`；块内行内片段记 `(src_start, src_end, render_kind)`，`render_kind` 如强调/强强调/行内代码/普通。
- 映射双向：`src_offset -> (block_idx, inline_idx, offset_in_inline)` 与反向。**不变量**：任何映射结果的边界必须落在 UTF-8 字符边界上——这是 H3 的判定核心。
- 重排粒度：块。编辑发生时只重解析，然后只对"受影响块 + 进出块"重新布局；其余块复用上一帧的布局结果。

### D4 · IME 走 gpui 的 `EntityInputHandler`，组合串用**独立缓冲**而非写入rope

原设计把 IME 当作最大未知。侦察后已由源码证据收敛（`gpui-pre 0.3.9` / `gpui-pre-windows 0.3.9`）：

- 通道存在且完整：`EntityInputHandler`（`input.rs:13`，8 个必需方法）、`ElementInputHandler`（`input.rs:128`）、`Window::handle_input`（`window.rs:5391`）。
- Windows 走 **IMM32**（非 TSF）：`WM_IME_STARTCOMPOSITION` / `WM_IME_COMPOSITION`（`gpui-pre-windows/src/events.rs:154/155`）→ `handle_ime_composition_inner`；`GCS_RESULTSTR` → `replace_text_in_range`、`GCS_COMPSTR` → `replace_and_mark_text_in_range`。
- 候选框定位：平台调用 `selected_text_range` + `bounds_for_range` 反查位置，再 `ImmSetCandidateWindow(CFS_CANDIDATEPOS)`，纵向锚在行框中线（`events.rs:634-681`）。
- **关键缺口**：`WM_IME_ENDCOMPOSITION` 未处理，全 crate 对 `unmark_text` / `marked_text_range` 零引用 → 这两个回调在 Windows 上**永不被调用**，组合态清理责任在应用侧。

**决策：组合串不入 rope。** 维护 `marked_text: String` + `marked_anchor: usize`，布局时把 rope 片段与该 marked 片段合成为统一的 `Vec<Fragment>` 一起 `shape_line`，`bounds_for_range` 在该合成坐标系里算 x，提交时才写 rope。

- 理由一：规格要求"组合期间 SHALL NOT 写入缓冲区"，rope 不动即天然满足，无需靠 undo 回滚。
- 理由二：平台层不通知组合结束，若沿用 gpui 官方的"先写入再标记"（`examples/input.rs:337-366`、`gpui-base/src/input/base/state.rs` 的 `ime_marked_range`），组合态残留必须靠应用侧猜测清理，容易重复插入或留下残留下划线。
- 代价：`text_for_range` / `selected_text_range` / `bounds_for_range` / `character_index_for_point` 全部要在合成坐标系里工作，比官方实现多一层映射。这是有意付的账——事后改会牵动整个布局层。

另两条实现期必须遵守的实测事实：

- **`handle_input` 只能在 `paint` 里调**（`window.rs:5391` 带 `debug_assert_paint()`），且仅在焦点就绪时注册 handler；漏掉它 → 全平台收不到任何字符（Windows 上字符走 `WM_CHAR` → `replace_text_in_range`，不经过 KeyDown）。
- **UTF-16 / UTF-8 双坐标系**：`EntityInputHandler` 一律 UTF-16 码元，而 `ropey`、`pulldown-cmark`、`TextRun::len` 一律 UTF-8 字节。CJK 与 emoji 下必须显式换算，这是最容易错位的一处。

### D5 · 性能策略：视口裁剪 + 块级缓存，用 `gpui-fps` 量

10 万字符文档不做全文布局：只布局视口内块并前后各留一屏余量。行高先取统一值（不做逐行字体度量），把"可变块高"留到 M2。测量面板直接用 `gpui-fps`（gpui-kit 自带），采样点记在 `findings.md`。

### D6 · spike 的物理形状

`spike/` 一个独立 crate（自己的 `Cargo.toml`，不进最终 workspace），产物 `spike/findings.md`。通过则归档 change、把 findings 交给 M1 提案；不通过则连同退路一起归档，spike 可整目录删除。

## Risks / Trade-offs

- [gpui 的文本布局围绕整体 StyledText 设计，块级异形布局要手写 measure] → spike 只做 6 类块，行高统一；若 measure 复杂度爆炸，判定 H1 为"可行但代价高"，在 findings 里给出量化代价而不是硬扛。
- [Windows IME 在 gpui-pre 快照上的支持深度未知] → 已由源码侦察消除平台层疑点（IMM32 通道、候选框回调均在）。风险下移为我们自己的 marked 状态机（见 D4）与「真机验证」：**agent 无法模拟输入法打字，S3/6.1–6.4 的真人验收必须由用户完成**，因此实现组要以可判定的运行时日志（组合开始/提交/清理各打一条带时间戳与偏移的记录）为自动化证据，再交人工确认。
- [纯 TSF-only 输入法可能不经 IMM32] → 未验证；若用户常用输入法属此类，作为已知限制记录，不阻塞 spike。
- [退路设计] → H2 失败：组合期间强制整行源码态（Typora 在输入法期间同样整行还原），最坏退"源码 + 分屏预览"；H1 失败：退单列流式 + 近似行高，或退分屏。退路一旦启用，M1 的方案要重写，所以必须先证。
- [`gpui-pre` 快照 API 漂移] → 精确 pin `=0.3.9`，同步固定 rustc 1.98.1；升级作为独立动作，不在 spike 内做。
- [首次编译时间长（GPUI 全家桶可能十几分钟）] → 一次性成本，接受；不为此改动依赖形态。
- [法务] → 只做行为复刻；界面素材、主题 CSS、字体自出。验收时对照的是"行为一致"而非"素材一致"。

## Migration Plan

1. spike 通过 → `openspec archive` 归档本 change，`specs/editor/*` 进入主 specs；`findings.md` 作为 M1 提案的技术输入。
2. spike 部分通过 → 同样归档，但在 `findings.md` 明确记录退路与它对 M1 架构的影响。
3. 回滚：删 `spike/` 目录即可，无生产代码受影响（仓库此前为空）。

## Open Questions

- ~~是否需要自定义字体度量以支持行内混排字号（标题行内嵌行内代码）~~ → 留给 M1，不改本轮 specs。
- 编辑时全量重解析是否始终落在预算内——spike 会给数字；若超预算，M1 再议增量解析，不影响本 change 的规格。

## 已确认的范围决策（M0 期间与用户敲定）

- **表格与 LaTeX 公式按原计划排在 M2/M3**，不作为 M1 内容。当前规格要求的是"以还原后的纯文本降级显示、不崩溃、不中断"（`specs/editor/hybrid-markdown-editing` 的「未支持语法降级显示」场景 + 任务 3.2），因此真机上看到 `| 列一 | 列二 |` 与 `$$E = mc^2$$` 原样显示是**符合规格的既定行为，不是缺陷**。之所以单列一条，是因为它两次被当作 bug 上报；M1 开始前请先读这一条。
- 相关代价估算（供 M1 排期参考）：表格需要列宽/对齐/单元格内编辑；公式需要 LaTeX 排版引擎，Rust 侧无成熟等价物，大概率要走内嵌 webview（`gpui-kit` 有 `gpui-webview`）。
