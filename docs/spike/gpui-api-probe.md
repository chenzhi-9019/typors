# gpui-pre 0.3.9 API 探测报告（H2 · Windows 输入法前置调研）

> 角色：技术侦察子代理。只做研究与文档，**不含生产代码**。
> 全部判断以源码为最终依据，每条后附 `文件:行号`。凭印象的推论单独标记为「未验证」。

**一句话结论**：H2 判定为 **可行**。`gpui-pre 0.3.9` 的 Windows IME 通道是**完整实现**的（IMM32 + `WM_IME_*` 全套 + `ImmSetCandidateWindow` 候选框定位），不需要我们自己补平台层。真正的断点在**我们自己的 `EntityInputHandler` 状态机**，以及 spec 里「组合期间不写缓冲区」与 gpui 默认语义的一处正面冲突（见 §3.5 / §8.3）。

---

## 0 · 探测环境与可复现性

| 项 | 值 |
| --- | --- |
| rustc / cargo | `rustc 1.98.0 (88d9e12ae)` / `cargo 1.98.0` |
| 依赖 | `gpui-kit = "=0.8.0"`（其内部 `gpui = { package = "gpui-pre", version = "=0.3.9" }`，见 `gpui-kit-0.8.0/Cargo.toml:264-266`） |
| 源码根 | `C:/Users/chenzhi/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/` |
| 探测 crate | `%TEMP%/gpui-probe`（一次性，可删） |
| 本文引用的相对路径 | 相对上面的源码根，如 `gpui-pre-0.3.9/src/input.rs:13` |

复现命令（全部实际执行过）：

```powershell
cd $env:TEMP\gpui-probe
cargo fetch
cargo build
cargo run
```

> 说明：`gpui-kit 0.8.0` 发布包里**没有** `examples/` 目录（`gpui-kit-0.8.0/` 只有 `src/ tests/ Cargo.toml README.md`），所以任务书里说的 `examples/hello_world` 在 crates.io 包里不存在；官方 hello world 是 `gpui-kit-0.8.0/src/lib.rs:19-40` 的 doctest，本文 §6 就是按它 + `gpui-pre` 自带 example 适配出来的。

---

## 1 · 自绘 Element

### 1.1 trait 名与必需方法

trait 名是 **`Element`**，定义在 `gpui-pre-0.3.9/src/element.rs:52`，super-trait 是 `IntoElement`（`element.rs:146`，要求实现 `type Element` + `fn into_element`）。

| 成员 | 签名要点 | 行号 |
| --- | --- | --- |
| `type RequestLayoutState` | 关联类型，`request_layout` 返回、`prepaint`/`paint` 接收 | `element.rs:55` |
| `type PrepaintState` | 关联类型，`prepaint` 返回、`paint` 接收 | `element.rs:59` |
| `fn id` | `fn id(&self) -> Option<ElementId>` | `element.rs:66` |
| `fn source_location` | `fn source_location(&self) -> Option<&'static panic::Location<'static>>` | `element.rs:70` |
| `fn request_layout` | `fn request_layout(&mut self, id: Option<&GlobalElementId>, inspector_id: Option<&InspectorElementId>, window: &mut Window, cx: &mut App) -> (LayoutId, Self::RequestLayoutState)` | `element.rs:74-80` |
| `fn prepaint` | `fn prepaint(&mut self, id, inspector_id, bounds: Bounds<Pixels>, request_layout: &mut Self::RequestLayoutState, window: &mut Window, cx: &mut App) -> Self::PrepaintState` | `element.rs:84-92` |
| `fn paint` | `fn paint(&mut self, id, inspector_id, bounds: Bounds<Pixels>, request_layout: &mut Self::RequestLayoutState, prepaint: &mut Self::PrepaintState, window: &mut Window, cx: &mut App)` | `element.rs:96-105` |
| a11y（选做） | `fn a11y_role` / `write_a11y_info` / `a11y_synthetic_children`（有默认实现） | `element.rs:113 / 121 / 132` |

**没有 `measure` 方法**。`Element` 只有上面这些。需要内在尺寸时用两处 `Window` API：

- `Window::request_layout(style: Style, children: impl IntoIterator<Item = LayoutId>, cx: &mut App) -> LayoutId` —— `window.rs:5209`
- `Window::request_measured_layout<F>(style, measure: F) -> LayoutId`，其中 `F: Fn(Size<Option<Pixels>>, Size<AvailableSpace>, &mut Window, &mut App) -> Size<Pixels>` —— **这就是文字测量的官方钩子**，`window.rs:5238-5241`（文档注释原文：「One place this is used internally is when measuring text.」）

### 1.2 在 paint 里怎么画

| 要画的东西 | API | 行号 |
| --- | --- | --- |
| 文字（已 shape 的单行） | `ShapedLine::paint(origin: Point<Pixels>, line_height: Pixels, align: TextAlign, align_width: Option<Pixels>, window, cx) -> Result<()>` | `text_system/line.rs:108` |
| 文字背景 / 下划线 | `ShapedLine::paint_background(...)` / `ShapedLine::paint_with_underline_handler(...)` | `line.rs:160 / 129` |
| 填充矩形 | `window.paint_quad(quad: PaintQuad)`，quad 由 `fill(bounds, background) -> PaintQuad` 构造 | `window.rs:4684` / `window.rs:7832`（`Window::fill` 关联函数） |
| 矢量路径 | `window.paint_path(path: Path<Pixels>, color)` | `window.rs:4755` |
| 下划线 | `window.paint_underline(...)` | `window.rs:4772` |
| SVG | `window.paint_svg(...)` | `window.rs:4983` |
| 命中区 | `window.insert_hitbox(bounds, HitboxBehavior) -> Hitbox` | `window.rs:5295` |

光标竖条/选区高亮都是 `fill(...)` + `paint_quad`，官方 example 就是这么写的：`gpui-pre-0.3.9/examples/input.rs:512-535`（光标 2px 宽竖条）与 `:522-534`（选区矩形）。

---

## 2 · 文本测量与排版

入口是 `Window::text_system() -> &WindowTextSystem`（类型定义 `text_system.rs:602`）。四个关键函数：

| 函数 | 签名 | 行号 |
| --- | --- | --- |
| `shape_line` | `fn shape_line(&self, text: SharedString, font_size: Pixels, runs: &[TextRun], force_width: Option<Pixels>) -> ShapedLine` | `text_system.rs:638-644` |
| `shape_text` | `fn shape_text(&self, text: SharedString, font_size: Pixels, runs: &[TextRun], wrap_width: Option<Pixels>, line_clamp: Option<usize>) -> Result<SmallVec<[WrappedLine; 1]>>`（多行 + 自动换行） | `text_system.rs:750-757` |
| `layout_line` | `fn layout_line(&self, text: &str, font_size: Pixels, runs: &[TextRun], force_width: Option<Pixels>) -> Arc<LineLayout>`（只要度量不要绘制时用） | `text_system.rs:886-892` |
| `shape_line_by_hash` | 同上但用内容 hash 做缓存键，避免为命中缓存而拼 `SharedString` | `text_system.rs:689-697` |

`TextRun` 结构：`text_system.rs:1228`（字段 `len, font, color, background_color, underline, strikethrough`，`len` 是 **UTF-8 字节数**，必须覆盖整行）。`UnderlineStyle` 在 `style.rs:828`。

**度量与命中**（`ShapedLine` deref 到 `LineLayout`，`line.rs:42-46`）：

| 需求 | API | 行号 |
| --- | --- | --- |
| 行宽 | `ShapedLine::width() -> Pixels` | `line.rs:88` |
| 行高 / 基线 | `LineLayout.ascent: Pixels`、`LineLayout.descent: Pixels` | `text_system/line_layout.rs:25 / 27` |
| 默认行高 | `Window::line_height() -> Pixels` | `window.rs:3167` |
| 字节偏移 → x | `LineLayout::x_for_index(index: usize) -> Pixels` | `line_layout.rs:108` |
| x → 字节偏移 | `LineLayout::index_for_x(x: Pixels) -> Option<usize>` / `closest_index_for_x(x) -> usize` | `line_layout.rs:61 / 78` |
| 切分单行 | `ShapedLine::split_at(byte_index)` | `line.rs:184+` |

### 2.1 「按块排版多个行内片段」应该用哪个

结论：**一个片段一次 `shape_line`，再用 `width()` 累加推进 pen**，不要试图用一次 `shape_line` 塞多个不同样式的片段去算各自位置。理由：`shape_line` 返回的 `ShapedLine` 只暴露整行的 `width()` 与「整行字节偏移 → x」，没有暴露「run 边界 → x」；跨 run 的片段起点只能靠前序片段宽度累加得到。

- 探测骨架 §6 就是这么实现的（`prepaint` 里 `for fragment { shape_line(...); pen += line.width() }`，`paint` 里同样的 pen 推进逐个 `line.paint(...)`），**已实际编译 + 运行通过**。
- 需要自动换行的段落级排版走 `shape_text(...) -> SmallVec<WrappedLine>`（`text_system.rs:750`），`WrappedLine` 结构在 `line.rs:443`，`paint` 在 `line.rs:460`。
- 同一行内混排不同字号：`force_width` 参数用于强制宽度（例如制表位/对齐），不是混排手段；混排靠多个 `TextRun`（同一 `font_size`、不同 `font`）。跨字号混排是否需要额外基线对齐，**未验证**（属 design.md 的 Open Question）。

> ⚠️ 已知限制：`shape_line` **只能处理单行**，文本里含换行符会 `debug_assert` 失败（`text_system.rs:645-648`）。块级排版必须自己按行切分。

---

## 3 · 输入法（H2 核心）

### 3.1 三个对象如何串起来

```text
你的 View (实现 EntityInputHandler, input.rs:13)
   ↓  ElementInputHandler::new(bounds, entity)        input.rs:128 / 137
   ↓  window.handle_input(&focus_handle, handler, cx) window.rs:5391  （只能在 paint 阶段调用）
   ↓  PlatformInputHandler                             platform.rs （含 unmark_text: 2066）
   ↓  PlatformWindow::update_ime_position(bounds)      platform.rs:1350
Windows: WM_IME_COMPOSITION → ImmGetCompositionStringW + ImmSetCandidateWindow (events.rs:155/713/654)
```

**两个 trait 都存在**，都完整：
- `EntityInputHandler`：`input.rs:13`（我们实现，13 个方法，见 3.2）
- `InputHandler`（平台侧 canonical 抽象）：`platform.rs:2240`
- `ElementInputHandler<V>`：`input.rs:128`，`InputHandler` 实现 `input.rs:145`

### 3.2 `EntityInputHandler` 全部方法（完整签名 + 行号）

**必需实现**（无默认体）：

```rust
// input.rs:15-21
fn text_for_range(
    &mut self,
    range: Range<usize>,                       // UTF-16 码元
    adjusted_range: &mut Option<Range<usize>>,
    window: &mut Window,
    cx: &mut Context<Self>,
) -> Option<String>;

// input.rs:24-29
fn selected_text_range(
    &mut self,
    ignore_disabled_input: bool,
    window: &mut Window,
    cx: &mut Context<Self>,
) -> Option<UTF16Selection>;               // platform.rs:2227

// input.rs:32-36
fn marked_text_range(&self, window: &mut Window, cx: &mut Context<Self>) -> Option<Range<usize>>;

// input.rs:39
fn unmark_text(&mut self, window: &mut Window, cx: &mut Context<Self>);

// input.rs:49-55
fn replace_text_in_range(
    &mut self,
    range: Option<Range<usize>>,
    text: &str,
    window: &mut Window,
    cx: &mut Context<Self>,
);

// input.rs:58-65
fn replace_and_mark_text_in_range(
    &mut self,
    range: Option<Range<usize>>,
    new_text: &str,
    new_selected_range: Option<Range<usize>>,
    window: &mut Window,
    cx: &mut Context<Self>,
);

// input.rs:68-74  ← 候选框锚定的唯一数据来源
fn bounds_for_range(
    &mut self,
    range_utf16: Range<usize>,
    element_bounds: Bounds<Pixels>,
    window: &mut Window,
    cx: &mut Context<Self>,
) -> Option<Bounds<Pixels>>;

// input.rs:77-82
fn character_index_for_point(
    &mut self,
    point: crate::Point<Pixels>,
    window: &mut Window,
    cx: &mut Context<Self>,
) -> Option<usize>;
```

**有默认实现（可不写）**：`paste`（`input.rs:42`，默认调 `replace_text_in_range`）、`set_selected_text_range`（`input.rs:85`）、`text_length_utf16`（`input.rs:94`，默认 `None`）、`accepts_text_input`（`input.rs:103`，默认 `true`）、`text_input_configuration`（`input.rs:108`）、`text_input_editable_range`（`input.rs:117`，默认 `None`）。

### 3.3 Windows 平台层如何把 IME 事件送进来

**crate/模块**：`gpui-pre-windows-0.3.9`，实现全部在 `gpui-pre-windows-0.3.9/src/events.rs`。走的是 **IMM32**（`ImmGetContext` / `ImmGetCompositionStringW` / `ImmSetCandidateWindow`），不是 TSF。

| 环节 | 实现 | 行号 |
| --- | --- | --- |
| 消息分发 | `WM_IME_STARTCOMPOSITION => self.handle_ime_position(handle)`；`WM_IME_COMPOSITION => self.handle_ime_composition(handle, lparam)` | `events.rs:154 / 155` |
| 组合字符串 | `handle_ime_composition` → `handle_ime_composition_inner(ctx, lparam)` | `events.rs:713 / 718` |
| 提交（结果串） | `lparam & GCS_RESULTSTR` → `input_handler.replace_text_in_range(None, &result)` | `events.rs:728-734` |
| 组合中（预编辑串） | `lparam & GCS_COMPSTR` → `replace_and_mark_text_in_range(None, &comp, caret_pos)`，`caret_pos` 来自 `GCS_CURSORPOS` | `events.rs:735-754`（行 739 `retrieve_composition_cursor_position`，定义在 `1713`） |
| 空组合（日文 IME 的 lparam==0） | `replace_text_in_range(None, "")` | `events.rs:720-726` |
| **候选框位置来源** | `retrieve_caret_position()` = `selected_text_range(false)` → `bounds_for_range(caret_range)` → 转成物理像素 `POINT` | **`events.rs:634-645`** |
| **候选框落位** | `ImmSetCandidateWindow(CFS_CANDIDATEPOS, ptCurrentPos=caret)` + `ImmSetCompositionWindow(CFS_POINT, ptCurrentPos=caret)` | **`events.rs:654-681`** |
| 组合起始即校正位置 | `WM_IME_STARTCOMPOSITION` → `handle_ime_position` → `update_ime_position` | `events.rs:647-652` |
| IME 使能/禁用 | `update_ime_enabled`：`ImmAssociateContextEx(IACE_DEFAULT / 0)`，禁用时 `ImmNotifyIME(NI_COMPOSITIONSTR, CPS_COMPLETE)` 先提交 | `events.rs:683-711` |
| gpui 侧主动刷新 IME 位置 | `Window::invalidate_character_coordinates()` → `selected_bounds` → `platform_window.update_ime_position(bounds)` | `window.rs:6653-6662`；平台接口 `platform.rs:1350` |
| IME 上下文生命周期 | `ImeContext::get` / `Deref` / `Drop(ImmReleaseContext)` | `events.rs:1460-1488` |

候选框纵向位置公式：`y = origin.y * scale + (height * scale) / 2`（`events.rs:641-642`），即**锚在行框中线**，不是行底。

`update_ime_enabled` 的输入是 `input_handler.query_accepts_text_input()`（`events.rs:684-686`），对应我们实现的 `accepts_text_input`（`input.rs:103`）。**这意味着「有没有焦点 + 是否接受文本输入」直接决定 IME 是否被启用**：不接受文本输入时 `ImmAssociateContextEx(..., 0)` 会把 IME 上下文摘掉。

### 3.4 缺口与未实现项（明确列出）

| 项 | 事实 | 证据 |
| --- | --- | --- |
| `WM_IME_ENDCOMPOSITION` | **未处理**。整个 `gpui-pre-windows-0.3.9` 里 `WM_IME_*` 只有 2 处命中，没有 ENDCOMPOSITION / NOTIFY，落到 `DefWindowProcW` | `events.rs:154-155`，`:165-171`（默认分支） |
| `unmark_text` 在 Windows 上**从不被调用** | 全 crate 搜 `unmark` 零命中；`marked_text_range` 零命中；唯一与 IME 回调相关的调用是 `bounds_for_range` | `gpui-pre-windows-0.3.9/src` 全局 grep；命中仅 `events.rs:637` |
| `marked_text_range` 在 Windows 上**从不被查询** | 同上 | 同上 |
| 组合串下划线样式 | 平台读了 `GCS_COMPATTR`（`events.rs:1717-1724`）但**没有把属性传给 `EntityInputHandler`**；下划线要我们在 `TextRun::underline` 里自己声明 | `events.rs:1717`；`examples/input.rs:476-501` |
| 纯 TSF-only 输入法 | 用的是 IMM32 兼容层。微软拼音/搜狗等 IMM32 兼容输入法应可用；是否有只走 TSF 的输入法不工作，**未验证** | `events.rs:1467 / 1691 / 1670` |

**这不是「缺失」，而是「由应用负责」**：平台把组合串原样交给 `replace_and_mark_text_in_range`，把清除组合态的责任留给我们（因为 ENDCOMPOSITION 不转发）。实现要点：在 `replace_text_in_range`（提交/清空）里必须清除自己的 marked 状态，否则组合串会残留。官方 example 的做法是 `.take()`：`examples/input.rs:333`、`:356-358`。

### 3.5 三个判定点：平台层有没有支撑

| 判定点 | 平台层 | 真正风险 |
| --- | --- | --- |
| 组合串能显示 | **有**：`WM_IME_COMPOSITION`+`GCS_COMPSTR` → `replace_and_mark_text_in_range`（`events.rs:735-754`） | 我们的实现要正确画 marked 段（用 `TextRun::underline`，参考 `examples/input.rs:476-501`） |
| 候选框锚定光标 | **有**：`bounds_for_range` → `ImmSetCandidateWindow(CFS_CANDIDATEPOS)`（`events.rs:634-681`） | `bounds_for_range` 必须返回**组合串起点**的 x；这要求 marked 串参与我们的布局坐标系 |
| 提交不丢字 | **有**：`GCS_RESULTSTR` → `replace_text_in_range(None, result)`（`events.rs:728-734`）；无 handler 时字符/结果被**静默丢弃**（`with_input_handler` 在无 handler 时返回 `None`，`events.rs:1437-1441`） | 组合串若已写入 rope 且提交时又按 `None` range 替换，会重复插入；必须靠 marked 状态机消解 |

---

## 4 · 键盘与文本输入

### 4.1 键盘事件链路（重要反直觉）

**Windows 上普通字符根本不走 KeyDown，全部走 `WM_CHAR` → `InputHandler::replace_text_in_range`**：

```rust
// gpui-pre-windows-0.3.9/src/events.rs:153, 461-468
WM_CHAR => self.handle_char_msg(wparam),
fn handle_char_msg(&self, wparam: WPARAM) -> Option<isize> {
    let input = self.parse_char_message(wparam)?;
    self.with_input_handler(|input_handler| {
        input_handler.replace_text_in_range(None, &input);   // ← 字符直接进 input handler
    });
    Some(0)
}
```

所以：**`paint` 里不调 `window.handle_input(...)`，应用在 Windows 上一个字符都收不到**（`with_input_handler` 无 handler 直接 `?` 返回 `None`，`events.rs:1437-1441`）。`WM_CHAR` 会过滤控制字符（`events.rs:1400-1402`）。

### 4.2 Action / KeyBinding 机制

| 步 | API | 行号 |
| --- | --- | --- |
| 声明 action | `actions!(my_module, [Backspace, Left, ...])` 宏 | `examples/input.rs:19-37` |
| 构造绑定 | `KeyBinding::new<A: Action>(keystrokes: &str, action: A, context: Option<&str>) -> Self` | `keymap/binding.rs:33` |
| 注册绑定 | `App::bind_keys(&mut self, bindings: impl IntoIterator<Item = KeyBinding>)` | `app.rs:2606` |
| 声明上下文 | `InteractiveElement::key_context<C, E>(self, key_context: C) -> Self`（作用在 Div 上） | `elements/div.rs:830` |
| 挂监听 | `InteractiveElement::on_action<A: Action>(&mut self, listener: impl Fn(&A, &mut Window, &mut App))` | `elements/div.rs:458` |
| 裸键监听 | `InteractiveElement::on_key_down` / `on_key_up` | `elements/div.rs:496 / 528` |
| 焦点 | `InteractiveElement::track_focus(&mut self, &FocusHandle)` | `elements/div.rs:788` |

### 4.3 `Element::dispatch_event` 存在吗？

**不存在**。`Element` trait（`element.rs:52-143`）没有 `dispatch_event`、没有 `on_key_*`、没有事件钩子。整个 crate 里 `dispatch_event` 只有一处，是 App 层的 `Window::dispatch_event(&mut self, event: PlatformInput, cx: &mut App) -> DispatchEventResult`（`window.rs:5593`）。

自绘 Element 接事件的两种正规姿势：

1. **推荐**：把自绘元素挂在 `div()` 上，用 `div().key_context(...).track_focus(...).on_action(...)` 接键盘，自绘 Element 只负责画。官方 example 正是这样：`examples/input.rs:590-622`（`div()` + `.key_context("TextInput")` + 6 个 `on_action` + `.child(TextElement { .. })`）。
2. 在 `paint` 里 `window.insert_hitbox(bounds, behavior)`（`window.rs:5295`）自己接鼠标命中。

> 注意 `handle_input` 有 `self.invalidator.debug_assert_paint()`（`window.rs:5397`），**只能在 paint 阶段调用**；在 `render` 或 `prepaint` 里调用会在 debug 构建下断言失败。

---

## 5 · 剪贴板与撤销

### 5.1 剪贴板：内置

| API | 行号 |
| --- | --- |
| `App::read_from_clipboard(&self) -> Option<ClipboardItem>` | `app.rs:1722` |
| `App::read_from_clipboard_async(...)` | `app.rs:1733` |
| `App::write_to_clipboard(&self, item: ClipboardItem)` | `app.rs:1750` |
| `ClipboardItem` / `::new_string(String)` / `::text() -> Option<String>` | `platform.rs:2969 / 3021 / 3055` |
| `EntityInputHandler::paste` 默认实现（走 `replace_text_in_range`） | `input.rs:42-46` |

> 实测注意：探测程序里 `write_to_clipboard("probe")` 紧跟 `read_from_clipboard()` 同一 tick 返回了 `None`（运行输出 `clipboard roundtrip: None`）。可能是 Windows 剪贴板写入异步/需消息泵推进。**该现象未深究，标记为未验证**；实现剪贴板功能时不要假设同步写后可立即读回。

### 5.2 撤销：gpui 核心没有，gpui-base 有

- `gpui-pre-0.3.9/src` 全量 grep `UndoHistory` / `UndoManager` / `undo_history`：**0 命中**。gpui 核心不提供撤销。
- `gpui-base 0.8.0`（`gpui-kit` 的必需依赖，通过 `gpui_kit::base` 可达，`gpui-kit-0.8.0/src/lib.rs:130`）提供两套：
  - `gpui-base-0.8.0/src/undo_history.rs:5` `pub struct UndoHistory<T>`，`push`（`:67`）、`undo`（`:90`）、`redo`（`:102`）；
  - `gpui-base-0.8.0/src/input/base/undo_manager.rs:76` `impl UndoManager`（面向文本编辑的事务式撤销）。

**建议**：混合编辑器的撤销单元是「一次编辑操作」（可能跨 projection 切换），而不是「一个字符」，所以直接复用 `UndoHistory<T>` 包一层自己的 `EditOp` 枚举比套用 `UndoManager` 更合适。这是设计判断，非源码结论。

---

## 6 · 最小可跑骨架（已实际编译 + 运行）

### 6.1 编译与运行记录

```powershell
PS> cd $env:TEMP\gpui-probe
PS> cargo build
#   Finished `dev` profile [unoptimized + debuginfo] target(s) in 1m 28s   ← 首次（含全部依赖）
#   Finished `dev` profile [unoptimized + debuginfo] target(s) in 11.98s  ← 改 main.rs 后
PS> cargo run
#      Running `target\debug\gpui-probe.exe`
#   clipboard roundtrip: None
#   exit=0    （窗口正常打开，3 秒后自行 quit）
```

**结果：编译通过 `exit=0`，运行 `exit=0`。** 该文件覆盖了本文所有关键 API：自绘 `Element` 三阶段、`shape_line` 多片段推进、`paint_quad`、`handle_input` + `ElementInputHandler`、`EntityInputHandler` 全部必需方法、`actions!` + `KeyBinding` + `key_context` + `on_action`、`read_from_clipboard` / `write_to_clipboard`。

> 首次编译 1 分 28 秒。**edition 必须为 2024**：`Element::paint` 里 `if ... && let Some(..)` 的 let-chain 在 edition 2021 下报 `error: let chains are only allowed in Rust 2024 or later`（实测）。

### 6.2 `Cargo.toml`

```toml
[package]
name = "gpui-probe"
version = "0.1.0"
edition = "2024"

[dependencies]
gpui-kit = "=0.8.0"
```

> 只有 `gpui-kit` 一个依赖。GPUI 类型从它的 re-export 取（`gpui-kit-0.8.0/src/lib.rs:101` `pub use ::gpui::*;`，`:117` `pub use crate as gpui;`），**不要**再单独声明 `gpui`/`gpui-pre` 依赖——design.md D1 的判断正确，且有源码支撑。

### 6.3 `src/main.rs`（完整，逐字取自已编译通过的探测 crate）

```rust
// API probe for gpui-pre 0.3.9 via gpui-kit 0.8.0.
// Purpose: prove the exact API surface a self-drawn hybrid editor needs:
//   custom Element + shape_line + paint_quad + ElementInputHandler + EntityInputHandler
//   + actions/keybindings + clipboard.
use std::{ops::Range, time::Duration};

use gpui_kit::gpui::InspectorElementId;
use gpui_kit::*;

actions!(probe, [Backspace, Quit]);

// ---------------------------------------------------------------- state

struct DocView {
    focus_handle: FocusHandle,
    text: String,
    selected_range: Range<usize>,
    marked_range: Option<Range<usize>>,
    last_lines: Vec<ShapedLine>,
    last_bounds: Option<Bounds<Pixels>>,
}

impl DocView {
    fn new(cx: &mut Context<Self>) -> Self {
        Self {
            focus_handle: cx.focus_handle(),
            text: "hello 混合 editor".to_string(),
            selected_range: 6..6,
            marked_range: None,
            last_lines: Vec::new(),
            last_bounds: None,
        }
    }

    fn backspace(&mut self, _: &Backspace, _window: &mut Window, cx: &mut Context<Self>) {
        if self.selected_range.is_empty() && self.selected_range.start > 0 {
            self.text.remove(self.selected_range.start - 1);
            self.selected_range = self.selected_range.start - 1..self.selected_range.start - 1;
        }
        cx.notify();
    }

    fn quit(&mut self, _: &Quit, _window: &mut Window, cx: &mut Context<Self>) {
        cx.quit();
    }

    fn cursor_offset(&self) -> usize {
        if self.selected_range.is_empty() {
            self.selected_range.start
        } else {
            self.selected_range.end
        }
    }
}

impl Focusable for DocView {
    fn focus_handle(&self, _cx: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

// ------------------------------------------------- EntityInputHandler (the H2 channel)

impl EntityInputHandler for DocView {
    fn text_for_range(
        &mut self,
        range_utf16: Range<usize>,
        adjusted_range: &mut Option<Range<usize>>,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<String> {
        let start = utf16_to_byte(&self.text, range_utf16.start);
        let end = utf16_to_byte(&self.text, range_utf16.end);
        adjusted_range.replace(byte_to_utf16(&self.text, start)..byte_to_utf16(&self.text, end));
        Some(self.text[start..end].to_string())
    }

    fn selected_text_range(
        &mut self,
        _ignore_disabled_input: bool,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<UTF16Selection> {
        Some(UTF16Selection {
            range: byte_to_utf16(&self.text, self.selected_range.start)
                ..byte_to_utf16(&self.text, self.selected_range.end),
            reversed: false,
        })
    }

    fn marked_text_range(
        &self,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<Range<usize>> {
        self.marked_range.as_ref().map(|r| {
            byte_to_utf16(&self.text, r.start)..byte_to_utf16(&self.text, r.end)
        })
    }

    fn unmark_text(&mut self, _window: &mut Window, _cx: &mut Context<Self>) {
        self.marked_range = None;
    }

    fn replace_text_in_range(
        &mut self,
        range_utf16: Option<Range<usize>>,
        text: &str,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let range = range_utf16
            .map(|r| utf16_to_byte(&self.text, r.start)..utf16_to_byte(&self.text, r.end))
            .or_else(|| self.marked_range.clone())
            .unwrap_or_else(|| self.selected_range.clone());
        self.text.replace_range(range.clone(), text);
        let end = range.start + text.len();
        self.selected_range = end..end;
        self.marked_range = None;
        cx.notify();
    }

    fn replace_and_mark_text_in_range(
        &mut self,
        range_utf16: Option<Range<usize>>,
        new_text: &str,
        new_selected_range_utf16: Option<Range<usize>>,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let range = range_utf16
            .map(|r| utf16_to_byte(&self.text, r.start)..utf16_to_byte(&self.text, r.end))
            .or_else(|| self.marked_range.clone())
            .unwrap_or_else(|| self.selected_range.clone());
        self.text.replace_range(range.clone(), new_text);
        let end = range.start + new_text.len();
        self.marked_range = (!new_text.is_empty()).then_some(range.start..end);
        self.selected_range = match new_selected_range_utf16 {
            Some(r) => {
                range.start + utf16_to_byte(new_text, r.start)
                    ..range.start + utf16_to_byte(new_text, r.end)
            }
            None => end..end,
        };
        cx.notify();
    }

    fn bounds_for_range(
        &mut self,
        range_utf16: Range<usize>,
        element_bounds: Bounds<Pixels>,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<Bounds<Pixels>> {
        let start = utf16_to_byte(&self.text, range_utf16.start);
        let end = utf16_to_byte(&self.text, range_utf16.end);
        let (x0, x1) = self.x_for_byte_offsets(start, end)?;
        Some(Bounds::from_corners(
            point(element_bounds.left() + x0, element_bounds.top()),
            point(element_bounds.left() + x1, element_bounds.bottom()),
        ))
    }

    fn character_index_for_point(
        &mut self,
        point: Point<Pixels>,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<usize> {
        let bounds = self.last_bounds?;
        let local = bounds.localize(&point)?;
        let mut x = px(0.);
        for line in &self.last_lines {
            if local.x >= x && local.x <= x + line.width() {
                let idx = line.index_for_x(local.x - x)?;
                return Some(byte_to_utf16(&self.text, idx.min(self.text.len())));
            }
            x += line.width();
        }
        Some(byte_to_utf16(&self.text, self.text.len()))
    }
}

impl DocView {
    /// x ranges of a byte range, summed across fragment lines.
    fn x_for_byte_offsets(&self, start: usize, end: usize) -> Option<(Pixels, Pixels)> {
        if self.last_lines.is_empty() {
            return None;
        }
        let mut pen = px(0.);
        let mut xs = [None, None];
        let mut consumed = 0usize;
        for (i, line) in self.last_lines.iter().enumerate() {
            let len = line.text.len();
            if xs[0].is_none() && start <= consumed + len {
                xs[0] = Some(pen + line.x_for_index(start - consumed));
            }
            if xs[1].is_none() && end <= consumed + len {
                xs[1] = Some(pen + line.x_for_index(end - consumed));
            }
            pen += line.width();
            consumed += len;
            if i + 1 == self.last_lines.len() {
                xs[0] = xs[0].or(Some(pen));
                xs[1] = xs[1].or(Some(pen));
            }
        }
        Some((xs[0]?, xs[1]?))
    }
}

fn byte_to_utf16(text: &str, byte: usize) -> usize {
    text[..byte.min(text.len())].encode_utf16().count()
}

fn utf16_to_byte(text: &str, utf16: usize) -> usize {
    let mut count = 0;
    for (byte, ch) in text.char_indices() {
        if count >= utf16 {
            return byte;
        }
        count += ch.len_utf16();
    }
    text.len()
}

// ------------------------------------------------------------- custom Element

/// One inline fragment inside a block: what the hybrid editor paints per line.
struct Fragment {
    text: SharedString,
    color: Hsla,
}

struct BlockElement {
    view: Entity<DocView>,
    fragments: Vec<Fragment>,
}

struct PrepaintState {
    lines: Vec<ShapedLine>,
    background: PaintQuad,
    cursor: Option<PaintQuad>,
}

impl IntoElement for BlockElement {
    type Element = Self;
    fn into_element(self) -> Self::Element {
        self
    }
}

impl Element for BlockElement {
    type RequestLayoutState = ();
    type PrepaintState = PrepaintState;

    fn id(&self) -> Option<ElementId> {
        Some(ElementId::Name("probe-block".into()))
    }

    fn source_location(&self) -> Option<&'static std::panic::Location<'static>> {
        None
    }

    fn request_layout(
        &mut self,
        _id: Option<&GlobalElementId>,
        _inspector_id: Option<&InspectorElementId>,
        window: &mut Window,
        cx: &mut App,
    ) -> (LayoutId, Self::RequestLayoutState) {
        let mut style = Style::default();
        style.size.width = relative(1.0).into();
        style.size.height = window.line_height().into();
        (window.request_layout(style, [], cx), ())
    }

    fn prepaint(
        &mut self,
        _id: Option<&GlobalElementId>,
        _inspector_id: Option<&InspectorElementId>,
        bounds: Bounds<Pixels>,
        _request_layout: &mut Self::RequestLayoutState,
        window: &mut Window,
        cx: &mut App,
    ) -> Self::PrepaintState {
        let view = self.view.read(cx);
        let marked = view.marked_range.clone();
        let selection = view.selected_range.clone();
        let cursor = view.cursor_offset();
        let font_size = window.text_style().font_size.to_pixels(window.rem_size());

        let mut lines = Vec::new();
        let mut pen = px(0.);
        let mut consumed = 0usize;

        for fragment in &self.fragments {
            let len = fragment.text.len();
            let run = TextRun {
                len,
                font: window.text_style().font(),
                color: fragment.color,
                background_color: None,
                underline: None,
                strikethrough: None,
            };
            let runs = match marked.clone() {
                Some(m)
                    if m.start < consumed + len && m.end > consumed =>
                {
                    let local = (m.start.saturating_sub(consumed)).min(len)
                        ..(m.end - consumed).min(len);
                    vec![
                        TextRun { len: local.start, ..run.clone() },
                        TextRun {
                            len: local.end - local.start,
                            underline: Some(UnderlineStyle {
                                color: Some(run.color),
                                thickness: px(1.),
                                wavy: false,
                            }),
                            ..run.clone()
                        },
                        TextRun { len: len - local.end, ..run.clone() },
                    ]
                }
                _ => vec![run],
            };
            let runs: Vec<TextRun> = runs.into_iter().filter(|r| r.len > 0).collect();
            let line =
                window
                    .text_system()
                    .shape_line(fragment.text.clone(), font_size, &runs, None);
            lines.push(line);
            pen += lines.last().unwrap().width();
            consumed += len;
        }

        let caret_x = {
            let mut x = px(0.);
            let mut acc = 0usize;
            for line in &lines {
                if cursor <= acc + line.text.len() {
                    x += line.x_for_index(cursor - acc);
                    break;
                }
                x += line.width();
                acc += line.text.len();
            }
            x
        };

        let cursor_quad = (selection == (cursor..cursor)).then(|| {
            fill(
                Bounds::new(
                    point(bounds.left() + caret_x, bounds.top()),
                    size(px(2.), window.line_height()),
                ),
                blue(),
            )
        });

        PrepaintState {
            lines,
            background: fill(bounds, white()),
            cursor: cursor_quad,
        }
    }

    fn paint(
        &mut self,
        _id: Option<&GlobalElementId>,
        _inspector_id: Option<&InspectorElementId>,
        bounds: Bounds<Pixels>,
        _request_layout: &mut Self::RequestLayoutState,
        prepaint: &mut Self::PrepaintState,
        window: &mut Window,
        cx: &mut App,
    ) {
        let focus_handle = self.view.read(cx).focus_handle.clone();
        window.handle_input(
            &focus_handle,
            ElementInputHandler::new(bounds, self.view.clone()),
            cx,
        );

        window.paint_quad(prepaint.background.clone());

        let mut pen = px(0.);
        for line in &prepaint.lines {
            line.paint(
                point(bounds.left() + pen, bounds.top()),
                window.line_height(),
                TextAlign::Left,
                None,
                window,
                cx,
            )
            .unwrap();
            pen += line.width();
        }

        if focus_handle.is_focused(window)
            && let Some(cursor) = prepaint.cursor.take()
        {
            window.paint_quad(cursor);
        }

        let lines = std::mem::take(&mut prepaint.lines);
        self.view.update(cx, |view, _cx| {
            view.last_lines = lines;
            view.last_bounds = Some(bounds);
        });
    }
}

// ------------------------------------------------------------------- render

impl Render for DocView {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let text = self.text.clone();
        let marked = self.marked_range.clone();
        let fragments = vec![
            Fragment { text: text.clone().into(), color: rgb(0x222222).into() },
            Fragment { text: "  [tail]".to_string().into(), color: rgb(0x888888).into() },
        ];
        let _ = marked;

        div()
            .flex()
            .flex_col()
            .key_context("Probe")
            .track_focus(&self.focus_handle)
            .cursor(CursorStyle::IBeam)
            .on_action(cx.listener(Self::backspace))
            .on_action(cx.listener(Self::quit))
            .bg(rgb(0xf5f5f5))
            .text_size(px(20.))
            .line_height(px(30.))
            .p(px(24.))
            .child(BlockElement { view: cx.entity(), fragments })
    }
}

fn main() {
    gpui_kit::application().run(|cx: &mut App| {
        gpui_kit::init(cx);
        cx.bind_keys([
            KeyBinding::new("backspace", Backspace, Some("Probe")),
            KeyBinding::new("ctrl-q", Quit, None),
        ]);
        gpui_kit::open_window(WindowOptions::default(), cx, |_window, cx| {
            cx.new(DocView::new)
        })
        .expect("failed to open window");

        // clipboard round trip: clipboard lives on App, not on the element.
        cx.write_to_clipboard(ClipboardItem::new_string("probe".to_string()));
        let pasted = cx.read_from_clipboard().and_then(|item| item.text());
        println!("clipboard roundtrip: {pasted:?}");

        cx.spawn(async move |cx| {
            cx.background_executor().timer(Duration::from_secs(3)).await;
            cx.update(|cx| cx.quit());
        })
        .detach();
    });
}
```

---

## 7 · H2 风险结论

### 7.1 判定

**可行，且不需要我们自己补平台层。** 依据：§3.3 的 6 个环节（消息分发 / 组合串 / 结果串 / 空组合 / 候选框位置 / 候选框落位）在 `gpui-pre-windows-0.3.9` 中逐条存在，全部走 IMM32，没有 `todo!()` 或空实现。

### 7.2 最可能的断点（按概率排序）

1. **我们自己的 marked 状态机**（不是平台）。平台只在 `WM_IME_COMPOSITION` 触发 `replace_and_mark_text_in_range`，之后**不再通知结束**（`WM_IME_ENDCOMPOSITION` 未处理，§3.4）。若我们把组合串写进 rope 并在提交时按 `None` 再替换一次，就会出现重复插入/残留下划线。
2. **候选框位置漂移**。它只依赖 `bounds_for_range(selected_text_range())` 一个返回值（`events.rs:634-645`）。在「混合投影」下光标可能在渲染态行内的任意 inline span 中间，`bounds_for_range` 必须把 UTF-16 偏移正确映射到「那一行 + 那段片段的 x」。任何一段映射错，候选框就会跳到行首或上一位置。**这是 §1/§2 的布局坐标系与 §3 的 UTF-16 坐标系必须严格对齐的地方。**
3. **组合期间切换投影**。spec 要求「组合期间光标块保持源码态」（`specs/editor/ime-text-input/spec.md` 最后一条 Requirement）。投影一切换，元素 bounds 与行布局都变，而 IME 组合仍在进行 → 需要「进入组合即冻结该块投影」的显式状态位，否则候选框和组合串位置会跳动。
4. （低概率）纯 TSF-only 输入法。IMM32 兼容层下微软拼音/搜狗应可用；**未在真机 IME 上验证**。

### 7.3 需要真机验证的最小清单（交给实现组）

| # | 验证项 | 观测点 | 挂掉的退路 |
| --- | --- | --- | --- |
| V1 | 输入 `nihao` 时组合串出现在光标处 | `replace_and_mark_text_in_range` 被调用、`new_text` 正确 | 组合期间强制整行源码态（spec 已要求） |
| V2 | 候选框在光标下方 | `bounds_for_range` 返回值 vs 实际候选窗位置 | 组合期间把整块投影成单行纯文本，让 bounds 计算退化 |
| V3 | 提交「你好世界」不丢字不重复 | `replace_text_in_range(None, "你好世界")` 一次调用、rope 只多 4 字 | 提交时用 marked range 替换而非 `None` |
| V4 | 提交后紧跟 `abc` + 空格 | `WM_CHAR` 三次进 handler，无丢失 | —（此环最稳） |
| V5 | Esc 取消组合 | 收到 `lparam == 0` 的组合（`events.rs:720-726`）或空 `GCS_COMPSTR`；rope 不变 | 自己在 `unmark_text` 里回滚临时缓冲 |
| V6 | 渲染态标题中启动组合 | 块投影切换到源码态且不抖动 | 退分屏预览 |

### 7.4 如果 H2 断掉的替代方案

1. **组合期间强制整行源码态**（spec 已内置要求）。最便宜的补丁，与 Typora 行为一致；代价是组合那一下有 1 次重排。
2. **组合串走独立缓冲，不进 rope**。把 `marked_text: String` 放在 view 上，布局时把「正文 rope 片段 + marked 串片段」拼成临时 `Vec<Fragment>` 再 shape。`replace_and_mark_text_in_range` 只更新 `marked_text`，`replace_text_in_range` 才写 rope。**这条同时解决 spec 的「组合期间不写入缓冲区」硬要求**（见 §8.3），建议实现组一开始就这么做。
3. 最坏：退「源码 + 分屏预览」，混合 Element 只做只读渲染。

---

## 8 · 实现组最容易踩的 3 个坑

### 8.1 `paint` 里忘了 `window.handle_input` = 全平台无输入

Windows 的字符、IME 组合、粘贴**全部**走 `InputHandler`（`WM_CHAR` → `replace_text_in_range`，`events.rs:461-468`；`WM_IME_COMPOSITION` → 同链路）。没有 handler 时 `with_input_handler` 的 `?` 直接返回 `None`，事件被吞（`events.rs:1437-1441`）。而且 `handle_input` 有 `debug_assert_paint`（`window.rs:5397`），只能在 `paint` 里调；`focus_handle.is_focused(window)` 为假时它**静默不注册**（`window.rs:5399-5404`）。

### 8.2 UTF-16 vs UTF-8 两套坐标系

- `EntityInputHandler` 的所有区间参数都是 **UTF-16 码元**（`input.rs` 全文 + `platform.rs:2244` 注释）；
- `TextRun::len`、`ShapedLine` 的 `x_for_index`/`len()` 都是 **UTF-8 字节**（`text_system.rs:1228`；`line.rs:78-82`）；
- ropey / pulldown-cmark 的 span 也是 **UTF-8 字节**（design.md D3 的 span 表）；
- 中文 3 字节 / 1 UTF-16，emoji 4 字节 / 2 UTF-16。**两套换算必须双向、且落在字符边界上**，否则组合期光标与候选框会错位。参考实现：`examples/input.rs:285-287`、`:399-400`（`range_from_utf16` / `range_to_utf16`）。

### 8.3 ⚠️ spec 与 gpui API 的正面冲突：「组合期间不写缓冲区」

这是本次侦察**最重要的发现**，必须在实现前拍板：

- spec（`openspec/changes/m0-hybrid-editor-spike/specs/editor/ime-text-input/spec.md`）要求：组合期间的字符「SHALL NOT 写入文档缓冲区」。
- gpui 的参考实现（`examples/input.rs:337-366`）语义相反：`replace_and_mark_text_in_range` **直接把 `new_text` 写进 `content`**，再用 `marked_range` 记下这段是「暂定的」，等 `replace_text_in_range` 提交时才 `.take()` 掉标记。`gpui-base` 的生产实现同样如此（`gpui-base-0.8.0/src/input/base/state.rs:4303` `impl ... EntityInputHandler`，`ime_marked_range` 字段 + `:4337-4340` `unmark_text` 清标记并 `commit_transaction`）。
- 平台层假设也是这个语义：`bounds_for_range` 与 `selected_text_range` 都基于「组合串已经在文档里」的坐标系（`events.rs:634-645`）。

**可行的消解方式**（建议写进 spike 的 findings）：
- 维护 `marked_text: String` + `marked_anchor: usize`（**不进 rope**），`EntityInputHandler` 的三个方法只操作它；
- 自己的布局函数把「rope 片段 + marked 片段」合成 `Vec<Fragment>` 一起 shape（§6 的 `BlockElement.fragments` 天然支持）；
- `bounds_for_range` 在「rope 坐标 + marked 串偏移」的合成坐标系里算 x；
- 提交时 `replace_text_in_range` 把 marked 串一次写进 rope 并清空 marked。

**代价**：`text_for_range` / `character_index_for_point` 都要在合成坐标系里回答（不能直接用 rope 下标）。可接受的复杂度，但**必须在 spike 第一版就按这个模型写**，事后改会造成布局层大改。

---

## 附录 A · 未验证项清单（不要当成结论用）

| # | 未验证内容 | 说明 |
| --- | --- | --- |
| U1 | 真机微软拼音/搜狗的实际行为 | 本轮只做源码级 + 编译运行验证，**没有真人用输入法打字** |
| U2 | 剪贴板同 tick write→read 返回 `None` | 观测到现象，未查原因（`clipboard roundtrip: None`） |
| U3 | 跨字号 inline 混排的基线对齐 | 未做实验；属 design.md Open Question |
| U4 | 纯 TSF-only 输入法是否工作 | IMM32 路径推断，未验证 |
| U5 | 启用 `gpui-fast` feature 后的 Windows 后端 | `gpui-kit` 的 `gpui-fast` 是可选替换后端（`gpui-kit-0.8.0/Cargo.toml:275-279`）。**默认关闭**（`default = ["component", "assets"]`），本文结论只对默认（gpui-pre 后端）成立；若打开该 feature，IME 走另一套实现，**未探测** |

## 附录 B · 源码路径索引（crate 相对路径）

| 主题 | 路径 |
| --- | --- |
| Element trait | `gpui-pre-0.3.9/src/element.rs:52-143` |
| EntityInputHandler | `gpui-pre-0.3.9/src/input.rs:13-124` |
| ElementInputHandler | `gpui-pre-0.3.9/src/input.rs:128+` |
| InputHandler（平台抽象） | `gpui-pre-0.3.9/src/platform.rs:2240-2408` |
| Window::handle_input | `gpui-pre-0.3.9/src/window.rs:5391` |
| Window::invalidate_character_coordinates | `gpui-pre-0.3.9/src/window.rs:6653` |
| Window::paint_quad / fill | `gpui-pre-0.3.9/src/window.rs:4684 / 7832` |
| Window::request_layout / request_measured_layout | `gpui-pre-0.3.9/src/window.rs:5209 / 5238` |
| shape_line / shape_text / layout_line | `gpui-pre-0.3.9/src/text_system.rs:638 / 750 / 886` |
| ShapedLine / WrappedLine | `gpui-pre-0.3.9/src/text_system/line.rs:43 / 443` |
| LineLayout（ascent/descent/x_for_index/index_for_x） | `gpui-pre-0.3.9/src/text_system/line_layout.rs:19 / 25 / 27 / 108 / 61 / 78` |
| 官方 input example（IME 参考实现） | `gpui-pre-0.3.9/examples/input.rs`（IME impl 277-402；Element 404-586；render 588-624） |
| 官方 editor example | `gpui-pre-0.3.9/examples/view_example/example_editor.rs:239+` |
| Windows IME 平台层 | `gpui-pre-windows-0.3.9/src/events.rs:154-155, 461-468, 634-762, 1437-1441, 1460-1488, 1660-1730` |
| gpui-base 生产版输入实现 | `gpui-base-0.8.0/src/input/base/state.rs:4303+`；`src/input/base/element.rs:3132`（`handle_input` 调用） |
| 撤销 | `gpui-base-0.8.0/src/undo_history.rs:5 / 67 / 90 / 102`；`src/input/base/undo_manager.rs:76` |
| gpui-kit pin 与 re-export | `gpui-kit-0.8.0/Cargo.toml:264-266`；`src/lib.rs:101 / 117 / 130 / 181 / 200 / 207` |
