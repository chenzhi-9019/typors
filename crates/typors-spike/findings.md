# typors M0 spike · findings

时间盒 2–3 天的可行性 spike。三个假设逐条判定，判定必须能由下面「复现命令栏」里的命令重跑出来。
本文件由任务组 1/2 建立骨架并填写 H3；H1 由任务组 3/4/7 填写，H2 由任务组 6 填写，**不要把占位当结论**。

## 结论栏

| 假设 | 问题 | 判定 | 支撑证据（可重跑） | 记录者 |
| --- | --- | --- | --- | --- |
| **H1** | 同一份文本缓冲区能否同时投影为渲染态与光标行源码态，重排开销是否可接受 | **可用**（渲染态已由自绘 `Element` 真实绘制；10 万字符文档在**视口裁剪 + 块级缓存**下每次重排 p95 **1.4–2.2 ms**、最大 3.9–6.0 ms、**0 次 > 100 ms 停顿**（三次运行）；未优化的同进程基线是 14.1–16.5 ms 中位、p95 16.7–17.8 ms，**24 份样本里有 2–24 份超 16 ms**。release 冷启动到可输入中位 **523–528 ms**（目标 ≤1 s）。无需启用任何退路） | `cargo run --release -- --dump-perf <100k字符.md>`（24 段滚动 + 文末光标 + 打字，输出 min/median/p95/max 与 >100 ms 计数，`result: PASS`）；`cargo run --release -- --dump-layout <100k字符.md>` 的 `timing:`/`culled:` 行；`pwsh -File bench-cold-start.ps1 -Doc <100k字符.md> -Runs 3`；`cargo test --release` **27 passed** | 任务组 3（初步）/ 4 / 7（最终） |
| **H2** | Windows 中文输入法在 gpui 自绘文本元素上是否可用（组合串 / 候选框 / 提交不丢字） | **待人工确认（自动化部分通过）**——合成回调序列 I0–I8 全绿，但「真实 `WM_IME_COMPOSITION` 会不会来」只能由真机打字回答，本轮 agent 无法触发真实输入法；真机验证清单在 H2 **§4**（V1–V6，10 分钟），断点判读方法在 **§3**。不写「可用」 | `cargo run --release -- --dump-ime sample.md`（6 条断言 + `result: PASS`；`--dump-ime <100k字符.md>` 同样 PASS）；`cargo test --release` 27 passed；真机证据待 V1–V6 日志 | 任务组 6（判定）/ 7（汇总） |
| **H3** | 源码偏移与渲染片段的双向映射在 CJK、组合 emoji 与嵌套强调下是否保真 | **可用**（内容偏移逐字节保真；零宽标记处按规范收敛，见下） | `cargo test --release` 的 **27** 个单测全绿；关键是 `doc::tests::src_to_render_roundtrip_is_exact_for_every_offset`（25 687 个偏移）与 `doc::tests::utf8_boundaries_hold_on_cjk_emoji_and_nested_emphasis` | 任务组 1 / 2（判定）/ 7（数字复核） |

## 复现命令栏

全部在 `spike/` 目录下执行。

```powershell
cd spike

# 1.1 依赖形态：gpui-pre 只有一份，cargo build 通过
cargo tree -d                     # 输出中不出现 gpui-pre 的重复版本段
cargo build --release

# 1.2 空窗口 + 读入 .md（进程存活 > 5 s，无 panic 输出）
cargo run --release -- sample.md

# 1.3 / 2.x 单测（H3 的全部证据）
cargo test --release

# 单条重跑（H3 判定核心）
cargo test --release src_to_render_roundtrip_is_exact_for_every_offset
cargo test --release render_offset_to_src_is_canonical
cargo test --release utf8_boundaries_hold_on_cjk_emoji_and_nested_emphasis
cargo test --release block_count_and_ranges_for_six_categories
cargo test --release blocks_and_spans_tile_without_gaps

# 人工核对块/span 表与偏移映射
cargo run --release --bin dump -- sample.md

# 3.x 渲染态：打开窗口（文档常驻渲染态，无光标/无编辑）
cargo run --release -- sample.md

# 3.x 程序化证据：跑一帧真实 paint 后 dump 每块与每个 fragment 的屏幕坐标，
#     并执行 4 条断言（A1 绘制文本 == block.render / A2 零宽 Marker 从未被绘制 /
#     A3 标记零宽且标题无 "#"、无水平位移 / A4 字节↔x 往返）。exit 0 = 全过
cargo run --release -- --dump-layout sample.md

# 3.2 范围外语法（表格 / 公式 / 图片 / 原始 HTML / 任务列表 / 删除线）降级不崩
cargo run --release -- --dump-layout <含这些语法的.md>

# 3.3 / H1 初步：全量重排耗时（dump 输出的 timing 行；不裁剪视口）
cargo run --release -- --dump-layout <100k 字符的.md>

# 7.1 视口裁剪 + 块级缓存 + 帧耗时分布（24 段滚动 + 文末光标 + 打字，含 >100 ms 停顿计数）
#     exit 0 = P1–P9 全过。同时打印 GPUI 自己的 frame trace（gpui-fps 的数据源）
cargo run --release -- --dump-perf sample.md
cargo run --release -- --dump-perf <100k 字符的.md>

# 7.1 窗口模式的真实 gpui-fps HUD（人眼看 FPS / FRAME / P95 / DROP）
cargo run --release -- --fps sample.md

# 7.2 release 冷启动：进程启动 → 日志里 ready 的时间戳，跑 3 次取中位数
pwsh -File bench-cold-start.ps1 -Doc sample.md -Runs 3
pwsh -File bench-cold-start.ps1 -Doc <100k 字符的.md> -Runs 3
```

### 复现用的 10 万字符文档怎么造

`findings` 里所有「100k 字符」的数字都在两份由 `sample.md` 生成的文档上量：

```powershell
cd spike
$s = [System.IO.File]::ReadAllText((Resolve-Path sample.md), [System.Text.Encoding]::UTF8)

# (a) 重复拼接：与组 3/4/6 用的同一份，短片段高度重复，对 shape cache 最友好
$sb = [System.Text.StringBuilder]::new()
while ($sb.Length -lt 100000) { [void]$sb.Append($s) }
[System.IO.File]::WriteAllText("$env:TEMP\typors-bench-100k.md", $sb.ToString(), [System.Text.UTF8Encoding]::new($false))
# -> 195 953 字节 / 100 423 字符 / 6058 块 / 7689 片段

# (b) 每行加唯一后缀：块文本与片段文本全都不重复，是对 shape cache 最坏的情况
$lines = $s -split "`n"; $sb = [System.Text.StringBuilder]::new(); $i = 0
$skip = @('```','---','$$','|','<','!','    ')
while ($sb.Length -lt 100000) {
  $i++
  foreach ($ln in $lines) {
    if ($ln.Length -eq 0) { [void]$sb.Append("`n"); continue }
    $t = $ln.TrimStart(); $k = $false
    foreach ($p in $skip) { if ($t.StartsWith($p)) { $k = $true; break } }
    if ($k) { [void]$sb.Append($ln).Append("`n") } else { [void]$sb.Append($ln).Append(" x$i").Append("`n") }
  }
}
[System.IO.File]::WriteAllText("$env:TEMP\typors-bench-unique-100k.md", $sb.ToString(), [System.Text.UTF8Encoding]::new($false))
# -> 182 697 字节 / 100 287 字符 / 5427 块 / 7236 片段
```

## H1 · 单缓冲双投影与重排开销

**状态：最终判定 = 可用（任务组 3 建立：自绘 `Element` 三阶段可用、块级 measure 的代价、渲染态全量重排的实测数据；任务组 7 收口：视口裁剪 + 块级缓存后 10 万字符文档每次重排 p95 1.6–1.8 ms、无 >100 ms 停顿，冷启动 523 ms。见 §7。）**

填写时必须给出：自绘 `Element` 的 `request_layout` / `prepaint` / `paint` 是否可用、块级 measure 的代码行数与难度、10 万字符文档按住 Ctrl+End 的每次重排耗时（`gpui-fps` 采样）、release 冷启动到可接受输入的时间（三次取中位数）。
判定取值只允许：可用 / 不可用 / 可用但代价高；不可用或代价高时必须写明退路（单列流式 + 近似行高，或退回源码 + 分屏预览）。

> **本节的数字已按组 4/5/6/7 之后的实际行为复核过。** 组 3 原文里的页面高度 790.1、绘制片段数 37 是组 3 当时的实测，现已分别变成 **1080.0** 与 **33**（原因见 §1 下的说明）；§4 的两行表格保留组 3 的原始测量作为历史，并在其后补上组 7 在同一台机器上重测的同口径数字。

### 任务组 3 的记录（3.1 / 3.2 / 3.3）

#### 1 · 自绘 `Element` 三阶段：可用

`Element` **没有** `measure` 方法（`element.rs:52-143`），块高全部由我们自己算再通过 `Style::size.height` 报给 Taffy。实测分工：

| 阶段 | 我们做的事 | 依据 |
| --- | --- | --- |
| `request_layout` | 用 `Window::text_system().shape_line()` 逐片段量宽，算出整篇 `DocLayout`；`style.size.width = relative(1.0)`、`style.size.height = 算出的总高` | `element.rs:74`、`text_system.rs:638` |
| `prepaint` | 把每个已定位片段再 shape 一次得到 `ShapedLine`，连同 origin / 行高存成绘制指令 | `element.rs:84` |
| `paint` | 先 `paint_quad` 画代码块底板 / 引用竖条 / 分隔线，再逐片段 `ShapedLine::paint`，最后把窗口坐标系的 `DocLayout` 发布到 view 上供命中测试 | `window.rs:4684`、`line.rs:108` |

六类块都走「渲染文本」路径。`--dump-layout sample.md` 的实际输出（节选）：

```text
measure: window-text-system
page: origin=(0.0,0.0) size=(1536.0,1080.0) content_width=1472.0
blocks: 26
drawn fragments: 33
[  0] Heading { level: 1 }               src=0..31 bounds=(32.0,32.0,1472.0,42.0) fragments=1
        span  1 bold                   @(32.0,32.0) w=370.1 render=0..28 text="typors 混合编辑器 spike"
[  2] Paragraph                          src=32..213 bounds=(32.0,84.0,1472.0,26.3) fragments=7
        span  0 plain                  @(32.0,84.0) w=34.0 render=0..6 text="这是"
        span  2 bold                   @(66.0,84.0) w=34.0 render=6..12 text="粗体"
        span  4 plain                  @(100.0,84.0) w=17.0 render=12..15 text="与"
        span  6 italic                 @(117.0,84.0) w=34.0 render=15..21 text="斜体"
        span 10 code                   @(185.0,84.0) w=68.0 render=27..39 text="行内代码"
[  4] Heading { level: 2 }               src=214..258 bounds=(32.0,130.3,1472.0,34.3) fragments=1
        span  1 bold                   @(32.0,130.3) w=379.5 render=0..40 text="二级标题里有 中文 与 emoji 😀"
[  6] ListItem { ordered: false, level: 0 } src=259..300 bounds=(32.0,174.6,1472.0,26.4) fragments=2
        decor text  "•" @ (36.5,174.6)
        span  1 plain                  @(54.0,174.6) w=158.0 render=0..28 text="无序列表第一项，含 "
        span  3 bold                   @(212.0,174.6) w=34.0 render=28..34 text="粗体"
[ 10] ListItem { ordered: true, level: 0 } src=364..389 bounds=(32.0,283.6,1472.0,26.4) fragments=1
        decor text  "1." @ (32.0,283.6)
        span  1 plain                  @(54.0,283.6) w=119.0 render=0..21 text="有序列表第一项"
```

要点：标题块只画 `span 1`（`span 0` 是 `## ` 零宽标记），绘制文本里没有 `#`；粗体/斜体/行内代码各自一个 fragment，`**` 与反引号是零宽 `Marker`，既不绘制也不推进 pen；列表项的项目符号是渲染器的装饰（`decor text`），不是源码里的 `- `。同一份 dump 里的 4 条断言全过：

```text
[ok] A1 painted text == block.render (newlines stripped) — 26 blocks
[ok] A2 no Marker/invisible span painted, fragments slice render exactly — 33 fragments
[ok] A3 marker runs stay zero-width: no '#' in headings, no horizontal shift — 2 headings ...
[ok] A4 caret x <-> hit round-trip and block_at_y self-consistency — 362 probes
result: PASS
```

**组 3 之后这两处数字为什么会变**（组 7 复核，`--dump-layout sample.md` 重跑结果）：

- **页面高度 790.1 → 1080.0**：组 4 给空行 `BlockKind::Gap` 补了行高（`render.rs` 的 `BlockKind::Gap` 分支按 `\n` 个数 × `body_size × line_ratio` 给高），空行从 0 高变成真实行盒，整页因此变高。这是修 bug（否则光标无法停在空行上），不是回归。
- **绘制片段 37 → 33**：`sample.md` 第 23 行改成规范写法（emoji 与闭合 `**` 之间隔空格与正文）后，那一行不再退化成含 `**` 的纯文本；同时 `A4` 探针数从 366 变 362，也是同一批文本变化的结果。

#### 2 · 范围外语法降级（3.2）：不崩溃、以纯文本显示

表格、`$$` 公式、图片、原始 HTML、任务列表、删除线、脚注在 `Options::empty()` 下一律退化为纯文本片段，`paint` 无失败、进程 `exit 0`、4 条断言仍全过。dump 的 `-- out-of-scope syntax as plain text (task 3.2) --` 段直接列出这些片段与屏幕坐标，节选（表格行按段内软换行分成三行）：

```text
  block   2 paragraph    @(32.0,84.0)   "| 列一 | 列二 | 列三 |"
  block   2 paragraph    @(32.0,110.3)  "|---|:---:|---:|"
  block   4 paragraph    @(32.0,199.4)  "行内公式 $E = mc^2$ 与块级公式："
  block   6 paragraph    @(32.0,235.7)  "$$"
  block  10 html (degraded) @(32.0,361.1) "<div class=\"note\">"
  block  11 paragraph    @(32.0,443.9)  "<span>"
```

#### 3 · 块级 measure 的真实代价（3.3）

代码规模（`(Get-Content <file>).Count`；`doc.rs` / `sample.rs` / `bin/dump.rs` 未被改动）：

| 文件 | 行数 | 说明 |
| --- | --- | --- |
| `spike/src/render.rs` | 1259 | 纯几何：`Metrics` 抽象、块级排版、命中测试、dump 与 4 条断言 |
| `spike/src/view.rs` | 292 | `Element` 三阶段 + view + `--dump-layout` 通道 |
| `spike/src/main.rs` | 241 | 窗口模式 / `--dump-layout` 模式 / 报告输出 |
| `spike/src/lib.rs` | 21 | 模块与 re-export |
| **组 3 净增** | **+1708** | 其中 dump 与断言约占 260 行，注释与接口文档约占 200 行 |

难度评价：**中偏上**，且难点不在"画"，在于下面 6 条（全部是实测踩出来的，不是估计）：

1. **`shape_line` 只能单行，这是排版架构的分水岭**（约占 render.rs 的 1/4）。`Block::render` 含段内软换行、围栏代码块还带尾换行，必须先切「逻辑行」再按可用宽度贪心折「视觉行」，最后每个视觉行内的每个片段单独 shape。还要处理"尾换行只是终止符不是新行"（`"fn main() {}\n"` 是 1 行不是 2 行）。
2. **异形块各自有异形定位**：标题靠字号层级 + 上间距；列表项要缩进且**序号得自己数**（`BlockKind::ListItem` 只带 `ordered`/`level`，序号按同层连续计数推出）；围栏代码块是"内容 padding + 底板 quad"，其 bounds 上下各多一圈（`code_pad`）；引用块是竖条 quad + 内容缩进；分隔线只有一条 quad。抽象出「块内统一 flow（left/right/top/font_size/line_height/color/forced style）+ 块级 decoration 列表」之后，新增一类块约 15–25 行。
3. **字节 ↔ x 不能近似**。`LineLayout::x_for_index` 给字节 → x；`index_for_x` 是**向上取整**（第一个位置 ≥ x 的字形），点在 CJK 宽字形左缘 0.5px 处会跳到下一字符。所以 `hit()` 要取上下两个候选边界比距离（`nearest_boundary`）。这条已封装，组 4 不要再自己算。
4. **`Pixels` 的字段是 `pub(crate)`**：`px(1.5).0` 编译不过，要用 `.as_f32()` / `f32::from(px)`；`+`/`-` 可用（derive 了 `Add`/`Sub`），`*` 只有 `Mul<f32>`。另外 `size`/`point`/`px` 是自由函数，局部变量起名 `size` 会把构造函数遮蔽掉（踩过一次）。
5. **`Element::RequestLayoutState` / `PrepaintState` 必须是 `'static`**，而命中测试还要继续用度量：把窗口的 `Arc<WindowTextSystem>` clone 进 `WindowMetrics`（`window.text_system()` 本来就返回 `&Arc<_>`），于是布局对象不带 `Window` 生命周期，`--dump-layout` 在没有窗口时还能用 `ApproxMetrics` 走同一条代码路径。
6. 每帧固定成本：写完布局要 `clone` 一份 `DocLayout` 发布给命中测试（`vec<BlockBox>` + 每块 `render: String`），10 万字符下约 3.5k 块 × 小字符串，属于任务组 7 该优化掉的部分（改成 `Rc<DocLayout>` 或按块版本号缓存）。

#### 4 · H1 初步测量（全量重排，**未**做视口裁剪）

计时点：`DocLayout::layout()`（含全部 `shape_line` 量宽）与 `prepaint` 的 shaping 各记一次；同一帧内再跑第二遍 `layout` 得到"字体系统已热"的稳态值（dump 的 `timing: steady-state layout` 行）。

组 3 的原始测量（保留作历史；注意 `sample.md` 的片段数现已变成 33，见 §1）：

| 文档 | 块 / 片段 | 冷 `layout` | 稳态 `layout` | 绘制 shaping | 合计（稳态） |
| --- | --- | --- | --- | --- | --- |
| `sample.md`（841 B） | 26 / 33 | 1.24–1.27 ms | **0.058–0.070 ms** | 0.05–0.07 ms | ≈ 0.13 ms |
| 10 万字符（组 7 的 195 953 B 重复拼接样本） | 6058 / 7689 | 4.62–6.52 ms | **3.77–4.13 ms** | 9.96–10.35 ms | **≈ 14–15 ms** |

组 7 在同一台机器上按同一口径重测（`--dump-layout`，单帧、未裁剪、缓存未命中），并补了第三遍「经缓存」的测量：

| 文档 | 冷 `layout` | 稳态 `layout`（字体系统热、缓存空） | **经缓存 `layout`**（块级 + shape 缓存热） | 绘制 shaping（裁剪后） |
| --- | --- | --- | --- | --- |
| `sample.md` | 1.237 ms | 0.060 ms | **0.019 ms** | 0.051 ms（0 / 33 片段被裁） |
| 100k 重复拼接 | 6.380 ms | 3.902 ms | **2.806 ms** | 0.185 ms（7615 / 7689 片段被裁） |
| 100k 唯一文本 | 6.1 ms | 4.6 ms | **2.3 ms** | 0.09 ms（98.5% 被裁） |

读法：

- 小文档的 1.2 ms 几乎全是字体系统首帧预热（冷 1.24 ms vs 热 0.060 ms），不是块数造成的。
- **组 3「10 万字符不裁剪 ≈ 10–11 ms，仍在 16 ms 内」这个结论在组 7 的样本上不成立**：同一份 10 万字符文档（块数是组 3 的 1.75 倍）不裁剪时是 **14–15 ms**，`--dump-perf` 的 24 份滚动样本里 **2/24 超过 16 ms（重复文本样本）/ 24/24 超过（唯一文本样本）**。也就是说「不做视口裁剪也能过」只在块数较少时侥幸成立，不能当作结论。§7 是这个断言的正式取证。
- shaping 确实占七成（9.6 / 14.1 ms）→ 复用 `ShapedLine` 是最大的一笔，其次是把量宽也接进同一个缓存、再其次是只 shape 视口内的片段。
- 组 3 第 6 条「每帧 clone 一份 `DocLayout` 发布给命中测试」这件事**没有改**：现在仍是 `paint` 里 `request_layout.clone()`。它的代价（100k 文档约 0.5–1 ms）已经被缓存与裁剪的收益盖过，改成 `Rc<DocLayout>` 会动到 `layout()` 的返回类型与 6 个调用点，收益不足以抵消风险（理由见 §7.3「没做的优化」）。

#### 5 · 交给后续组的接口（渲染态几何，`spike/src/render.rs` + `view.rs`）

```rust
// 无 Window 依赖，可无头复用；度量由调用方注入
DocLayout::layout(doc: &Document, theme: &EditorTheme, metrics: Arc<dyn Metrics>,
                  origin: Point<Pixels>, width: Pixels) -> DocLayout
layout.metrics() -> &dyn Metrics
layout.block_bounds(block_idx: usize) -> Option<Bounds<Pixels>>
layout.block_at_y(y: Pixels) / block_at_point(p: Point<Pixels>) -> Option<usize>   // block_idx
layout.hit(p: Point<Pixels>) -> Option<RenderHit>          // { block_idx, render_byte, span_idx }
layout.caret_x(block_idx, render_byte) -> Option<(Pixels, Pixels, Pixels)>  // (x, line_top, line_height)
layout.caret_bounds(block_idx, render_byte) -> Option<Bounds<Pixels>>       // 2px 宽光标矩形

WindowMetrics::new(window: &Window, mono_family: SharedString)   // 真实字形度量（就是被绘制的那个）
WindowMetrics::with_cache(window, mono_family, Arc<ShapeCache>)  // 组 7：与上一帧共用 shaping 缓存
ApproxMetrics                                                     // 无窗口时的确定性近似

// 组 7（任务 7.1）：缓存、裁剪与逐帧成本
DocLayout::layout_mode_marked_cached(.., cache: Option<&mut BlockCache>) -> DocLayout
ShapeCache::{new, stats, len, clear}          // 跨帧 ShapedLine 缓存（命中/未命中计数）
BlockCache::{new, stats, len, clear}          // 块级几何缓存（几何以块顶为 0 存，复用即平移）
Viewport::{UNBOUNDED, on_screen, intersects}  // paint 阶段的视口带（只裁画，不裁几何）

// view 侧
MarkdownView::new(doc) / MarkdownView::with_dump(doc, sink)
MarkdownView::layout() -> Option<&DocLayout>   // 上一帧已 painted 的布局
MarkdownView::perf_frame(metrics, width, viewport_height, scroll, cull) -> PerfSample
MarkdownView::{shape_cache, block_cache, clear_caches, frame_metrics}
```

坐标系约定：`MarkdownView::layout()` 返回的 `DocLayout` **已在窗口坐标**（`paint` 里做过 `translate(bounds.origin)`），组 4 用鼠标点直接喂 `hit()`，不要再加 `bounds.origin`。

#### 6 · 一条与 H3 有关的新事实（不是渲染缺陷，但要知道）

`sample.md` 第 23 行 `嵌套强调：外层**粗体里嵌 *斜体中文* 与外层 emoji 😀**结尾。` 里，**外层的两个 `**` 在 `pulldown-cmark 0.13` 眼里不是 `Strong`，而是 `SpanKind::Text`**（该行的 `block.render` 就含 `**`）。因此渲染态**会**画出这两个 `**`。这不是渲染器把标记画出来了：dump 的 `A1`（绘制文本逐字节等于 `block.render`）与 `A2`（没有任何 `Marker` span 被绘制）都通过。也就是说"视口内不出现 `#` / 反引号 / `- `"在标题、行内代码、列表标记上都成立；**强调分隔符是否出现取决于该行是否真的被解析成强调**。M1 若要"这两个 `**` 也消失"，需要的是解析层加固（或在渲染态之上再加一层 raw-text 遮罩），不在渲染器里解决。

该行现已是规范写法（见 §1 的说明），所以 `sample.md` 上这两个 `**` 不再出现；偏差本身留在 H3 一节的「已知解析器偏差」里，M1 仍需处置。

#### 7 · 任务组 7 的收口（7.1 性能 · 7.2 冷启动）—— H1 的最终取证

##### 7.1 · 做了什么

四处改动，每处都对应组 3/4/6 指出的一条已知开销：

| # | 改动 | 位置 | 组 3/4/6 的原话 |
| --- | --- | --- | --- |
| ① | **`ShapeCache`**：跨帧缓存 `ShapedLine`，键 = (字体 tag, 文本, 字号, 样式, force_bold, 颜色, 下划线)。`layout` 的量宽与 `prepaint` 的 shaping 共用同一个实例（`WindowMetrics::with_cache`），一帧内不再填两个空缓存 | `render.rs`（`ShapeCache` / `WindowMetrics` / `Metrics::tag`） | 「shaping 占七成 → 后续优化重点是"复用上一帧的 `ShapedLine`"」 |
| ② | **块级几何缓存** `BlockCache`：每块的 `spans`/`decors`/高度以**块顶为 0** 存下来，复用是一次平移；失效戳 = (`doc.revision`, 文档长度, 页宽, `origin.x`, 字体 tag, 主题) | `render.rs`（`BlockCache` / `BlockGeometry` / `block_geometry`） | 「每帧固定成本：写完布局要 clone 一份 `DocLayout`……属于任务组 7 该优化掉的部分」 |
| ③ | **视口裁剪**（只在 paint 阶段）：`shape_frame` 只 shape、只发出视口带内（含上下各一屏的余量，`CULL_MARGIN`）的片段与 quad | `view.rs`（`shape_frame` / `prepaint`）、`render.rs`（`Viewport`） | design.md D5「视口裁剪」 |
| ④ | **干掉每帧一次 `doc.text()` 全量拷贝**：光标块的源码切片改为 `doc.rope().slice(block.src)`，只取那几十字节 | `render.rs::source_slice` | 「有光标时每帧一次 `doc.text()` 全量拷贝，裁剪时应一并干掉」 |

##### 7.1 · 效果（`--dump-perf`，同一进程、同一文档、24 段滚动）

下面的每个数字都跑了 **3 遍**，给的是 3 遍的区间（同一台机器上单次运行之间 p95 有 ~1.5 倍的抖动，`max` 抖动更大，所以只报单次值会显得比实际精确）。

**100k 重复拼接样本（195 953 B / 6058 块 / 7689 片段 / 页高 241 410.6 px）**

| 序列 | 中位 | p95 | 最大 | >16 ms | >100 ms |
| --- | --- | --- | --- | --- | --- |
| `baseline`（不裁剪、每步清缓存 = 组 3/4 的代码） | 14.065 ms | 16.731 ms | 16.871 ms | **2 / 24** | 0 |
| `cached`（①②，仍不裁剪） | 11.414 ms | 12.090 ms | 12.952 ms | 0 / 24 | 0 |
| **`shipped`（①②③，窗口实际跑的）** | **1.00–1.13 ms** | **1.36–2.22 ms** | **3.89–4.80 ms** | **0 / 24** | **0** |

分项（中位）：`layout` 4.459 → 0.963 ms，`shaping` 9.594 → **0.034** ms，裁剪掉 **98.7%**（182 074 / 184 536）的片段 shaping。

**100k 唯一文本样本（182 697 B / 5427 块 / 7236 片段 / 页高 213 560.0 px）—— 对缓存最坏的情况**

| 序列 | 中位 | p95 | 最大 | >16 ms | >100 ms |
| --- | --- | --- | --- | --- | --- |
| `baseline` | 16.507 ms | 17.819 ms | 22.024 ms | **24 / 24** | 0 |
| `cached` | 11.729 ms | 12.788 ms | 13.652 ms | 0 / 24 | 0 |
| **`shipped`** | **0.97–1.13 ms** | **1.58–1.59 ms** | **5.79–5.89 ms** | **0 / 24** | **0** |

分项（中位）：`layout` 5.466 → 0.880 ms，`shaping` 10.973 → **0.081** ms，裁剪掉 98.5%。这一份里 shape cache 在整个扫描过程中有 3606 次未命中（重复样本只有 68 次），所以「唯一文本」这一列不是被内容重复美化的数字。

**文末光标（Ctrl+End 产出的那一帧）与打字**（3 遍的区间）：

| 场景 | 重复样本 p95 | 唯一文本 p95 | >100 ms |
| --- | --- | --- | --- |
| 光标跳到文末，连续 24 帧 | **1.08–1.14 ms** | **1.03–1.20 ms** | 0 |
| 文末每敲一个字符（编辑 + 重排 + shaping） | **3.84–3.93 ms** | **3.83–4.46 ms** | 0 |

##### 7.1 · 验收断言（`--dump-perf`，exit 0 = 全过）

| # | 断言 | 结果 |
| --- | --- | --- |
| P1 | 10 万字符文档滚动全程，每次重排 p95 < 16 ms 且无 >100 ms 停顿 | ok（重复样本 1.36–2.22 ms / 唯一文本 1.58 ms p95，0 次停顿） |
| P2 | 光标跳到文末（Ctrl+End）同上 | ok（1.08–1.20 ms p95） |
| P3 | 文末每敲一个字符，编辑 + 重排 + shaping 的 p95 < 16 ms | ok（3.83–4.46 ms） |
| P4 | 未优化基线可在同进程复现（给出前后对比，而不是一句"变快了"） | ok（14.1× / 16.8–17.3×） |
| P5 | **缓存与裁剪不改变几何**：24 个滚动停靠点的 warm layout 与 cold layout 逐块指纹相同 | ok |
| P6 | 经缓存的布局仍通过 A1–A5 | ok |
| P7 | 裁剪不移动光标：culled 与 uncropped 布局给同一光标矩形 | ok |
| P8 | 裁剪不改变选区几何：highlight 与 `seam_gaps` 与未裁剪布局逐矩形一致 | ok |
| P9 | 探针不污染文档：结束时缓冲区逐字节等于开始时 | ok |

`layout.blocks` **始终是完整文档几何**，裁剪只发生在 `prepaint`/`paint`。所以 `block_idx` 直接下标（`display_byte_for_src` / `src_for_display_byte` / `caret_x` / `line_tops` / `selection::display_ranges` / `highlight_rects` / 各探针）一行没动，也不需要占位 `BlockBox` 或索引表 —— 这是组 5 那条警告的最省事解法。`seam_gaps` 与新加的 P8 一起证明「裁剪不造出假缝」。

**裁剪带是从「真的滚动过的窗口」上验证的，不是算出来的。** 写这段代码的 agent 看不见窗口、也滚不动鼠标，所以留了两个环境变量当探针：`TYPORS_SCROLL=<px>` 在第一帧把页面滚到该文档偏移，`TYPORS_CULL_LOG=1` 每帧打印裁剪带。实测（`TYPORS_SCROLL=120000 TYPORS_CULL_LOG=1 spike.exe <100k字符.md>`）：

```text
cull: element_top -120000.0, viewport 1095.0, band 118905.0..122190.0, shaped 104, culled 7585
```

`bounds.origin.y` 确实是滚动偏移的相反数（-120000），带是 `120000 ∓ (1095 + 一屏余量 1095)`，一屏只 shape 104 个片段、裁掉 7585 个 —— 也就是说「滚到文末时屏幕上仍有字」这一条是被观测到的，而不是被假设的。

##### 7.1 · 关于 `gpui-fps`

- **真正接进来的部分**：`gpui-fps 0.8.0` 是开启 `gpui/profiler` 特性的那个 crate（它自己的 `Cargo.toml` 里就这么写：*"`gpui/profiler` belongs here … cargo unifies features across the graph"*）。`--dump-perf` 因此能读 `gpui::profiler::FrameTimingCollector` —— **正是 `gpui-fps` 面板显示的同一份 frame trace** —— 并把它打印成报告的 E 段。实测输出（重复样本一次运行）：

  ```text
  -- E. GPUI frame trace (the source `gpui-fps` reads) --
     draw  967.469 ms = request_layout    6.731 + prepaint    0.235 + paint  960.452 ms  (invalidations 1)
     draw    4.588 ms = request_layout    2.730 + prepaint    0.091 + paint    1.719 ms  (invalidations 2)
     draw    3.786 ms = request_layout    2.288 + prepaint    0.139 + paint    1.333 ms  (invalidations 2)
     draw    5.415 ms = request_layout    3.254 + prepaint    0.149 + paint    1.901 ms  (invalidations 3)
  ```

  第二行起是真实帧：`request_layout` 2.3–3.3 ms + `prepaint` 0.14 ms + `paint` 1.3–1.9 ms，与 P1–P3 的墙钟值同量级，可以互证。第一行 967 ms 的 `paint` 就是跑 `--dump-perf` 自己那一次扫描（同步、单帧内完成），说明探针的开销落在探针那一帧里，没有被算进被测的帧。
- **窗口模式的 HUD**：`cargo run --release -- --fps sample.md` 用 `gpui_fps::fps_monitor(window, cx)` 把真面板画在页面上（FPS / FRAME / P95 / DROP / GPU / CPU / MEM）。这是 `gpui-fps` 公开 API 的正常用法。
- **做不到的部分，如实说**：`FpsMonitor` 没有任何读取统计值的公开方法（只有 `new` / `capacity` / `frame_budget` / `show_resources` / `resource_interval` 这几个构造器），而 HUD 需要一个可见窗口才能量。所以 `--dump-perf` **不能**从面板里取数，它取的是面板的数据源（`FrameTimingCollector`）。面板本身只在 `--fps` 窗口模式下提供人眼读数。

##### 7.2 · release 冷启动（进程启动 → `ready`）

窗口模式在输入就绪时打一条 `ready` 日志，并在 `main()` 第一条指令打一条 `boot`；两条都带绝对墙钟毫秒（`ime.rs::stamp`），所以计时是「父进程在 spawn 前的时刻 → 子进程日志里的 `ready` 时刻」，不依赖轮询精度。`bench-cold-start.ps1` 跑 3 次取中位数：

| 文档 | 三次 (ms) | 中位 | 其中 `main()` 之前 | `main()` + 开窗 |
| --- | --- | --- | --- | --- |
| **100k 唯一文本（182 697 B）** | 523 / 528 / 538 | **528 ms** | 136–139 ms | 387–415 ms |
| `sample.md` | 466 / 481 / 486 | **481 ms** | ~135 ms | ~330 ms |

目标 ≤ 1 s，中位 528 ms，**通过**。冷/热差异很小（538 vs 523/528 ms），因为二进制与文档在三次运行之间都在系统文件缓存里 —— 真正的「冷盘」冷启动没有测到（见 U3）。注意这量的是 `target/release/spike.exe` 本体，不是 `cargo run`（后者会多出 cargo 自身的检查开销，那不属于应用的冷启动）。

##### 7.3 · 没做的优化，以及为什么

| 没做 | 原因 |
| --- | --- |
| 把 `paint` 里那份 `request_layout.clone()` 换成 `Rc<DocLayout>` | 这是组 3 第 6 条点名的开销（100k 文档约 0.5–1 ms）。但它要改 `MarkdownView::layout()` 的返回类型和 6 个调用点（probe/selection/ime 都在读 `&DocLayout`），而收益已经被 ①③ 盖过（总数从 14 ms 降到 1 ms 量级）。属于 M1 的性能余量，不是 M0 的判据 |
| 打字时只重排受影响的块 | 一次按键会 `apply_edit` → 全文重解析 → `revision` 自增，`BlockCache` 的失效戳因此整体失效，所以「每敲一个字」仍是全量重排（3.8–4.5 ms）。要做到按键只排一块，得让 `Document` 给出「哪些块变了」（块级 dirty 标记），那是数据层改动，不在组 7 的文件边界内。当前 3.8–4.5 ms 已在预算内 |
| 分块/异步 shaping（后台线程预排下一屏） | 现在单帧最坏 6.0 ms（首次访问一屏新区域），没有必须异步的必要；引入线程会把 `ShapedLine` 的所有权跨线程搬，风险大于收益 |
| 近似行高 / 单列流式（design D5 的退路） | **不需要启用**：裁剪 + 缓存的实测已经远在预算内，退路只会牺牲保真度 |
| 真机 `gpui-fps` 面板读数 | 面板没有公开读数 API（见上），只能人眼看；`--dump-perf` 用它的数据源代替 |

##### 7.3 · 文件与规模（组 7）

`[System.IO.File]::ReadAllLines(f).Count`：`src/perf.rs` **704**（新，`--dump-perf` 探针 + 分布统计 + P1–P9 断言）、`src/render.rs` 1918 → **2607**（`ShapeCache`、`BlockCache`、`BlockGeometry`/`block_geometry` 抽取、`Viewport`、`source_slice` 与 5 条新单测）、`src/view.rs` 1868 → **2167**（`shape_frame`、`prepaint` 裁剪、`perf_frame`、`--fps` HUD、光标/高亮按视口过滤）、`src/main.rs` 578 → **696**（`--dump-perf` / `--fps`、`boot` 日志、frame trace 输出）、`src/lib.rs` 38 → **43**；`bench-cold-start.ps1` **122**（新）。`src/doc.rs`、`src/sample.rs`、`sample.md`、`src/ime.rs`、`src/selection.rs`、`src/probe.rs` **未改动**。


## H2 · Windows 中文输入法

**判定：待人工确认（自动化部分通过）。** 下面这一节能证明的是：**如果** Windows 的 IMM32 回调真的交到 `EntityInputHandler`，那么组合串显示、候选框锚点、提交、取消、组合期间源态保持这五条规矩都成立，且逐条有可重跑的断言。它**不能**证明「微软拼音按下去、`WM_IME_COMPOSITION` 真的会来」——那只能由用户在真机上打一遍字来回答，本轮 agent 无法触发真实输入法。因此本判定的措辞固定为**待人工确认**，不写「可用」；`结论栏` 的 H2 行归任务组 7.3 汇总填写。

写法与 Tasks 6.5 的对应关系：判定在上、断点判读方法在 §3、真机步骤在 §4、退路在 §6。

> 组 7 汇总：`结论栏` 的 H2 行已按本节填写为 **待人工确认（自动化部分通过）**，并指向本节 §4 的 V1–V6 清单。**没有**改写成「可用」——本轮没有真机输入法日志。

### 1 · 自动化证据：`--dump-ime` 重放平台的真实调用顺序

```powershell
cd spike
# H2 的自动化部分（6.1–6.4）：退出码 0 = 6 条断言全过
cargo run --release -- --dump-ime sample.md
# 10 万字符文档上的同一组断言（顺带给出组合更新的重排耗时）
cargo run --release -- --dump-ime <100k 字符的.md>
# 单测：组合缓冲的坐标换算
cargo test --release
```

`--dump-ime` 只在**真的画出过一帧之后**运行（`EntityInputHandler` 的每个方法都要 `&mut Window`，而窗口只存在于帧内），所以它断言的是用户眼睛能看到的那份几何，不是第二套实现。它按 `gpui-pre-windows/src/events.rs` 的顺序合成事件：

```text
GCS_COMPSTR  -> replace_and_mark_text_in_range(None, "nihao", Some(0..5))
GCS_COMPSTR  -> replace_and_mark_text_in_range(None, "nih",   None)
GCS_RESULTSTR-> replace_text_in_range(None, "你好世界")            （提交）
lparam == 0  -> replace_text_in_range(None, "")                    （取消 / Esc）
WM_CHAR      -> replace_text_in_range(None, "abc ")                （提交后紧接 ASCII）
```

`sample.md` 上的实际输出（节选，`--dump-ime`）：

```text
-- I1/I2 composition start and update.
   1 start nihao          compstr=nihao sel=0..5         marked="nihao"@35 sel=0..5 rope#0/841B caret=35
   relayout per update      0.063 ms (full document)
   composite layout         block 2 Source display_byte=3 painted="这nihao是**粗体**与*斜体*以及`行内代码`混排的一段正文。…"
   selected_text_range       utf16=28..28 composite=Some(40..40) -> bounds_for_range (94.7,110.3 2.0x26.3)
   2 update nih           compstr=nih sel=none           marked="nih"@35 rope#0/841B caret=35
-- I3/I6 commit, CJK geometry and element_bounds.
   3 commit 你好世界          result=你好世界 range=none         marked=none rope#1/853B caret=47
   CJK range bounds         (49.0,110.3 70.0x26.3) measured cjk=68.0 ascii4=39.7
   element_bounds ignored   (49.0,110.3 70.0x26.3) vs (49.0,110.3 70.0x26.3)
-- I7 the caret block stays in the source projection while composing.
   projection @ after update heading Some(Source) source blocks [4] painted "## biaotilan二级标题里有 中文 与 emoji 😀"
   heading anchor           bounds (150.2,183.0 2.0x34.3) line height 34.3
-- I8 the candidate anchor follows a list item's indent.
   list item anchor         block 6 content_left=54.0 bounds (89.2,253.6 2.0x26.3) line height 26.3

-- checks --
  [ok] I1/I2 composition is displayed without touching the buffer, update replaces it — rope unchanged at revision 0, composition painted at the caret and underlined
  [ok] I3/I6 commit inserts once at the anchor, CJK width is measured, element_bounds ignored — one edit, caret behind the four characters, block still the only source block
  [ok] I4/I5 cancel leaves the buffer byte-identical, ASCII after commit appends cleanly — cancel: same bytes, same revision, caret restored; 'abc ' appended once
  [ok] I7 a rendered heading block flips to source when composing and keeps it until the caret leaves — one source block at every step; markers painted; back to rendered once the caret left
  [ok] I8 the candidate anchor follows an indented block's content edge — anchor x sits at the list content edge with the full line height
  [ok] I0 the probe restores the document and leaves no composition behind — 841 bytes, 4 edits
result: PASS
```

逐条对应 Tasks 6.1–6.4：

| 任务 | 断言（`--dump-ime`） | 断的是什么 |
| --- | --- | --- |
| 6.1 组合串显示且不写缓冲区 | I1/I2：`mark` 之后 `rope` 的**字节数与 `revision` 都不变**、`marked == "nihao"`、组合串作为**独立下划线片段**出现在 `display_byte` 处；更新成 `"nih"` 时是替换而不是追加（`painted` 里不出现 `nihao`/`nihan`） | 架构 D4：组合串不进 rope |
| 6.2 候选框锚定光标 | I1/I3/I6/I7/I8：`selected_text_range` → `bounds_for_range` 得到的矩形 x 落在该偏移的**片段边缘**（与 `Flow` 的 pen 推进独立互证），高度等于该块行高（正文 26.3 / `Heading{2}` 34.3），并断言**忽略 `element_bounds`**（传一个 origin=(411,733) 的矩形，结果不变）、CJK 四字宽度 70.0 ≈ 实测 advance 68.0 且是 ASCII 的 ~1.8 倍、列表项锚点跟着 `content_left=54.0` 走 | UTF-16 ↔ 合成坐标 ↔ 显示坐标三层换算 |
| 6.3 提交与取消 | I3：提交 4 字 = **恰好 1 次编辑**（`revision +1`）、光标紧随 `anchor+12`、`marked` 已清空（**没有依赖 `unmark_text`**）、该块仍是 source 而 25 个兄弟块仍是 rendered；I4：取消后**字节一致且 `revision` 也没变**（连一次空编辑都不记）、光标复原；I5：紧随的 `"abc "` 得到 `你好世界abc `，无丢字无重复 | 组合态清理责任在应用侧 |
| 6.4 组合期间源态保持 | I7：光标进标题前该段落已回到 rendered；组合 start/update/commit 三步都断言「正好 1 个 source 块 = 该标题」且 `## ` 仍在画；提交后仍是 source；光标移走后回到 rendered | 组合期间不得重投影 |
| 6.5 判定与断点 | 本节 | — |

`cargo test --release`：当时 **17 passed**（原 13 条 + `ime` 模块 4 条坐标换算单测；组 7 收口时全 crate 是 **27** 条），`--dump-layout` / `--dump-cursor` / `--dump-edit` 仍分别 5 / 11 / 4 条断言全过（未削弱、未删除任何 A1–A5 / C1–C6 / E1–E4 断言）。

### 2 · 本轮修掉的一个真实缺陷（属 6.2，不属真机问题）

组 4 的 `bounds_for_range` 用 `Bounds::from_corners(start, end.top_right())` 算矩形，而 `top_right()` 的 y 就是**行顶**——两端在同一行时返回的矩形**高度恒为 0**。平台拿它算候选窗纵向位置：`y = origin.y * scale + (height * scale) / 2`（`gpui-pre-windows/src/events.rs:641-642`，锚在**行框中线**），高度 0 就等于把候选窗锚到行顶，**整体高半行**。修复为保留行框高度（`size(width, caret_line_height)`）。这就是 6.2 要找的那类「坐标算错 → 候选框偏移」，只是它在源码里而不是在输入法里。

### 3 · 真机日志与三条断点的判读方法

每个 IME 回调入口都往 **stderr** 打一行：时间戳、方法名、入参，以及事后的 `marked` / `rope` / `caret` 摘要（`--dump-ime` 与窗口模式都生效；`TYPORS_IME_LOG=off` 可关）。

```text
ime t=+3.412s wall=1792903133179ms mark           compstr="nihao" sel=Some(0..5) range=None anchor=35 marked=none rope#0/841B caret=35 -> marked="nihao"@35 sel=0..5 rope#0/841B caret=35
ime t=+3.415s wall=1792903133182ms selected_range utf16=28..28 marked="nihao"@35 sel=0..5 rope#0/841B caret=35
ime t=+3.416s wall=1792903133183ms bounds         range=28..28 composite=40..40 -> (94.7,110.3 2.0x26.3)
ime t=+4.020s wall=1792903133787ms commit         result="你好世界" range=None was_marked=true src=35..35 marked="nih"@35 rope#0/841B caret=35 -> marked=none rope#1/853B caret=47
ime t=+5.100s wall=1792903134867ms cancel         result="" range=None was_marked=true marked="nihao"@47 sel=0..5 rope#1/853B caret=47 -> marked=none rope#1/853B caret=47
```

窗口模式启动时会先打一条 `ready`，所以「一行 `mark` 都没有」是可读的结论，而不是空日志。

| 断了哪一环 | 日志特征 | 说明 |
| --- | --- | --- |
| **组合事件没来** | 只有 `ready`，打拼音时**没有 `mark` 行** | 平台没把 `WM_IME_COMPOSITION` 交进来（IME 未启用 / 纯 TSF 输入法 / `handle_input` 未注册）。这不是我们的状态机问题，先查焦点与输入法类型 |
| **候选框位置不对** | `bounds` 行的 `x,y` 与实际候选窗位置不符 | `bounds` 是平台 `ImmSetCandidateWindow` 用的那个矩形（窗口坐标）。`y` 应落在光标所在行的**中线附近**；若 y 比行顶高半行、或 x 在行首/窗口左上角，就是坐标换算错。同时看 `composite=` 是否指向组合串内部 |
| **提交丢字 / 重复** | `commit` 行的 `result` 与之后的 `rope#N/MB` 对不上；或 `rope` 字节增量 ≠ `result` 字节数；或出现两条 `commit` | `rope#` 是 `Document::revision`、`/MB` 是字节数。一次提交应当恰好 +1 revision、字节数 + `result.len()`。`was_marked=true` 而 `rope` 没动 = 组合串被吞；`rope` +了两次 = 重复插入 |

还有一栏值得顺手看：`mark` 行的 `anchor=` 在**整个组合期间必须不变**（rope 不动，锚点就不该动），`caret=` 也应当一直是同一个值。

### 4 · 人工验证步骤 V1–V6（留给用户，10 分钟）

前置：Windows + 微软拼音（或搜狗）设为当前输入法；`cargo run --release -- sample.md` 打开窗口；stderr 重定向到文件便于回读（PowerShell：`cargo run --release -- sample.md 2> ime.log`）。

| # | 操作 | 期望 | 日志里核对 |
| --- | --- | --- | --- |
| V1 | 点进正文段落中段，打 `nihao`，先**不要**选字 | 光标处显示 `nihao` 且带下划线；窗口标题不用管，文档里不出现 `nihao` | 有 `mark compstr="nihao"`；`rope#` 与 `MB` **不变** |
| V2 | 继续打/退格，让组合串变成 `nih` | 组合串原地变短，不叠加成 `nihan` | 第二条 `mark compstr="nih"`，`anchor=` 与第一条相同 |
| V3 | 选「你好世界」提交 | 段落里出现四个字，光标紧随其后，下划线消失 | 一条 `commit result="你好世界" was_marked=true`，`rope#` +1、`MB` +12，`caret=anchor+12` |
| V4 | 紧接打 `abc` 加空格 | 得到 `你好世界abc `，不丢字不重复 | 四条 `commit result="a"/"b"/"c"/" "`（`WM_CHAR` 逐字），`rope` 每次 +1 |
| V5 | 再打 `nihao`，按 **Esc** | 组合串消失，文档内容与 V5 之前逐字节一致 | 一条 `mark` 后跟一条 `cancel result=""`，且 `cancel` 前后 `rope#` / `MB` **完全相同** |
| V6 | 把光标放进「二级标题」，打 `biaoti` 并提交 | 该标题立刻显示为 `## 二级标题…` 源码态，组合期间不抖动，提交后仍是源码态，光标移走后恢复渲染态 | `mark`/`commit` 之间 `selected_range` 的 `bounds` 高度是标题行高、位置在标题行内 |

断点判读：V1 就没有 `mark` → 断在「组合事件」；V1/V2 有 `mark` 但候选窗跑到别处 → 断在「候选框位置」，把 `bounds` 行的值与截图一起交回来；V3/V4 的 `commit` 与 `rope` 对不上 → 断在「提交丢字」；V5 有 `rope` 变化 → 断在「取消」；V6 组合期间标题跳回渲染态 → 断在「组合期间源态保持」。

### 5 · 真机未覆盖的部分（诚实边界）

- 真实 `WM_IME_COMPOSITION` / `WM_IME_ENDCOMPOSITION` 的行为、候选窗的**视觉**位置，本轮无人工确认（U1 仍未消除）。
- 纯 TSF-only 输入法是否走 IMM32 兼容层，仍未验证（U4）。
- 剪贴板往返：见下面的 U2 —— 本环境拿不到 Win32 剪贴板，这一项是 `not exercised`，不是通过。
- `WM_IME_ENDCOMPOSITION` 未处理这一事实只影响「组合被系统单独结束时」的清理；我们的模型把清理放在提交/取消路径上，因此不依赖它——但若真机上出现「提交前系统先结束组合、随后才发 `GCS_RESULTSTR`」，日志会表现为一条 `mark` + 一条 `commit`（`was_marked=true`），仍能正确提交。
- 组合期间若用户用方向键移动光标，组合锚点不会跟着走（Windows 通常会先结束组合）；日志会如实显示 `anchor` 与 `caret` 分离。

### 6 · 退路（按代价排序，尚不需要启用）

1. **组合期间强制整行源码态**：本轮已顺带满足——光标块本来就是 source，组合串被拼进该块的源码切片里画。若真机上出现重排抖动，退路是「组合期间冻结该块几何（只在提交后重排一次）」，代价是组合串不能自动换行。
2. **组合串不入 rope**：本轮已采用（design D4），并且是规格「组合期间 SHALL NOT 写入缓冲区」的唯一实现方式；不需要回滚。
3. **最坏退源码 + 分屏预览**：仅当 V1 完全收不到 `mark` 且确认是平台层问题时才考虑。

### 7 · 交给 M1 / 任务组 7 的接口与代价

坐标三层的换算封装在 `src/ime.rs`（`MarkedText`）+ `DocLayout::{display_byte_for_composite, caret_bounds_composite}`：

```rust
// 组合态（不进 rope）
MarkedText { text, anchor, selection }            // 组合串 / 落在哪个源码字节 / IME 光标
MarkedText::{splice, to_source, to_composite, composite_caret, composite_range}
// 布局：把组合串拼进光标块的源码投影（唯一注入点）
DocLayout::layout_mode_marked(doc, theme, metrics, origin, width, mode, Some(&marked))
layout.display_byte_for_composite(&doc, composite_byte) -> (block_idx, display_byte)
layout.caret_bounds_composite(&doc, composite_byte) -> Option<Bounds<Pixels>>   // 候选窗锚点
// view
MarkdownView::{marked, marked_text, layout_now}
view.layout_now(metrics, origin, width)           // 当前投影 + 当前组合串
```

代价与坑（都已在代码里注释）：

- `paint` 里的 `window.handle_input` 是唯一入口，`request_layout`/`paint`/`sync_layout`/`seed_layout` 现在共用 `layout_now`，否则会出现「一帧有组合串、下一帧没有」。
- `bounds_for_range` 必须**忽略 `element_bounds`**（布局已是窗口坐标，加回去就双重偏移），并且**必须返回带行高的矩形**（见 §2）。
- 提交路径必须自己清 `marked`：Windows 的 `unmark_text` / `marked_text_range` 从不被调用。
- 组合期间所有 `EntityInputHandler` 方法都在「合成坐标」里回答（rope + 组合串），因此 `text_for_range` 返回的文本会**包含**未提交的组合串；这是有意的，与「不写缓冲区」不冲突。

**给组 7（H1）的一条数据**：组合更新走的是与按键同一条全量重排路径——`sample.md` 0.063 ms，10 万字符（3120 块）**1.79–1.92 ms**（仅 `layout`，不含 `prepaint` 的 shaping；叠上 H1 记录的 7–8 ms shaping 后≈一次按键的成本）。也就是说拼音每敲一个字母的代价与敲一个字符等价，`H1` 的视口裁剪（7.1）同样会把它降下来，不需要为 IME 单独优化。

**组 7 回填**：这条判断成立。`--dump-perf` 的「文末每敲一个字符」序列走的就是同一条路径（`apply_edit` → 全量重解析 → 全量重排 → 裁剪 shaping），在 100k 文档上是 p95 **3.8–4.5 ms**（两次样本各 3 遍）、0 次 >100 ms，无需为 IME 加任何特殊优化。另外 7.1 顺带把「有光标时每帧一次 `doc.text()` 全量拷贝」换成了只读光标块那几十字节（`render.rs::source_slice`），这条对 IME 路径同样是净收益 —— 组合期间每帧都要用到那个源码切片。

**文件与规模**（`[System.IO.File]::ReadAllLines(f).Count`）：新增 `src/ime.rs` 1176 行（组合缓冲 + 日志 + `--dump-ime` 的 I0–I8 断言 + 4 条单测）；改动 `src/render.rs` 1736 → 1914（组合串拼接与合成坐标）、`src/view.rs` 1149 → 1328（`EntityInputHandler` 重写、`layout_now`、`--dump-ime` 的帧内挂点）、`src/main.rs` 396 → 462（`--dump-ime` 模式）、`src/lib.rs` 29 → 34（模块与导出）；`src/probe.rs` / `src/doc.rs` / `src/sample.rs` / `sample.md` 未改动。

## H3 · 源码与渲染片段的双向映射

**判定：可用。** 在 CJK、组合 emoji（ZWJ 家庭、肤色、组合重音 U+0301、区域旗帜）、粗体内嵌斜体、行内代码、围栏代码块、有序/无序列表、以及范围外语法（表格 / 公式 / 图片 / 原始 HTML）上，100 份混合样本的**每一个**字节偏移都完成了「源码 → 渲染 → 源码」往返，逐条相等，且结果始终落在 UTF-8 字符边界上。

### 数据模型（`spike/src/doc.rs`）

- 缓冲区 `ropey::Rope` 单份；解析用 `pulldown-cmark 0.13`（`Options::empty()`，即表格/脚注/删除线不启用，范围外语法降级为纯文本）。
- `Block { kind, src, render, spans }`：`src` 是文档绝对字节区间（行对齐），`render` 是剥掉标记后的文本，`spans` 是块内片段表。
- **块与 span 都是「无缝铺满」的**：`blocks` 依次覆盖 `[0, len_bytes())`，`spans` 依次覆盖整块。因此任意源码字节都属于唯一一个块和唯一一个 span，不需要「找不到就 fallback」的模糊路径，这是往返可判定的前提。
- 三类 span：`Text`（可见，且与源码切片逐字节相同）、`Marker(kind)`（零宽，如 `## `、`**`、`` ` ``、`> `、围栏行、列表标记）、`Degraded`（可见但与源码不等长：转义、硬换行、实体、原始 HTML）。
- 嵌套强调不靠嵌套区间表达，而是**扁平化 + 样式栈**：`外层**粗体里嵌 *斜体* 外层**` 被铺成 `Marker(**) Text(bold) Marker(*) Text(bold+italic) Marker(*) Text(bold) Marker(**)`。这样行内片段永不重叠，渲染器一个 span 一个 style 就够。

### 双向映射的语义（这是 H3 判定的核心）

- `src_to_render(src) -> RenderPos { block_idx, inline_idx, offset_in_inline }`、`render_to_src(&RenderPos) -> Option<usize>`：**无损对**。`offset_in_inline` 可见 span 内与渲染文本偏移同值（因为可见 span 与源码逐字节相同），零宽标记内则是标记自身内的局部偏移（标记占 0 渲染宽度，但它在源码里确有字节，于是这些字节有唯一归属）。因此任意边界偏移往返严格相等。
- `src_to_render_offset(src) -> (block_idx, render_byte)`、`render_offset_to_src(block_idx, render_byte) -> src`：**可见投影对**，用于「画光标」和「点击命中」。零宽标记在渲染侧没有自己的位置，必然多对一，故约定：正向收敛到标记所在的渲染位置，反向取**最小**的源码偏移（即标记起始）作为规范位。测试 `render_offset_to_src_is_canonical` 逐样本穷举全部渲染字节，与「最小值定义」严格对齐。

结论的准确表述是：**内容偏移逐字节保真；零宽标记处多对一，但收敛规则确定、可测、不跨片段**——这正是混合编辑器需要的语义（标记字节永远落在当前块上，而当前块在光标进入时本来就是源码态，走的是恒等映射）。

### 边界不变量与验证

| 不变量 | 验证测试 |
| --- | --- |
| 块、span 无缝铺满，渲染文本 == 可见 span 拼接 | `doc::tests::blocks_and_spans_tile_without_gaps` |
| 100 份混合样本的每个字节偏移往返相等（本次共 **25 687** 个源码偏移，含每份样本文末偏移） | `doc::tests::src_to_render_roundtrip_is_exact_for_every_offset` |
| 可见投影反向是规范最小源码偏移 | `doc::tests::render_offset_to_src_is_canonical` |
| CJK / 组合 emoji / 嵌套强调下所有映射落在字符边界 | `doc::tests::utf8_boundaries_hold_on_cjk_emoji_and_nested_emphasis` |
| 六类块（标题 / 粗体 / 斜体 / 行内代码 / 围栏代码块 / 列表）块数与源区间 | `doc::tests::block_count_and_ranges_for_six_categories` |
| 范围外语法（表格 / 公式 / 图片 / 原始 HTML）降级不崩、映射仍全覆盖 | `doc::tests::out_of_scope_syntax_degrades_without_panic` |
| 单缓冲编辑后重解析，映射仍成立 | `doc::tests::edit_reparses_and_keeps_mapping` |
| 退化文档（空文档、只有标记）不 panic | `doc::tests::empty_documents_are_safe` |

单测共 9 条（含样本生成器的自检）——`doc` 模块自己的 9 条；全 crate 当时合计 13 条，组 7 收口时是 **27** 条（见本节末）。`cargo test --release` 与 `cargo test` 均全绿。

### 给后续组的调用接口（`use spike::{Document, ...}`）

```rust
// 文档：单缓冲 + 块表，编辑后自动重解析
Document::parse(&str) -> Document
doc.text() -> String                       // 从 rope 取回全文
doc.len_bytes() -> usize                   // 字节长度（不是字符数）
doc.blocks() -> &[Block]
doc.revision() -> u64                      // 每次 apply_edit 自增，供重排缓存失效
doc.apply_edit(src: Range<usize>, new_text: &str)   // 改 rope + 重解析全量
doc.is_char_boundary(byte) -> bool
doc.snap_floor(byte) -> usize / doc.snap_ceil(byte) -> usize
doc.block_at_src(src: usize) -> Option<usize>

// 无损对（任意偏移可逆）
doc.src_to_render(src: usize) -> RenderPos               // { block_idx, inline_idx, offset_in_inline }
doc.render_to_src(&RenderPos) -> Option<usize>

// 可见投影对（画光标 / 点击命中；零宽标记多对一，反向取规范最小偏移）
doc.src_to_render_offset(src: usize) -> Option<(block_idx, render_byte)>
doc.render_offset_to_src(block_idx: usize, render_byte: usize) -> Option<usize>
```

```rust
struct Block { kind: BlockKind, src: Range<usize>, render: String, spans: Vec<Span> }
enum BlockKind { Heading { level: u8 }, Paragraph, ListItem { ordered: bool, level: usize },
                 FencedCode { info: String }, IndentedCode, BlockQuote, Rule, Html, Gap }
struct Span { src: Range<usize>, render: Range<usize>, kind: SpanKind, style: Style }
enum SpanKind { Text, Marker(MarkerKind), Degraded }
struct Style { bold: bool, italic: bool, code: bool, link: bool }
enum MarkerKind { HeadingHash, ListBullet, TaskBox, QuoteMarker, EmphasisDelim,
                  InlineCodeDelim, CodeFence, LinkSyntax, LineBreak, Other }
```
`BlockKind::Gap` 是「不属于任何 Markdown 块」的行（空行、游离文本），它存在是为了让 `blocks` 严格铺满全文；渲染器可以直接跳过它或原样输出。渲染时按 `span.render` 从 `block.render` 切片、按 `span.style` 选字形即可；`Marker` 的 `render` 是空区间，天然不画。

### 失败样本

本轮**没有**出现往返失败或半字符切割的样本。以下两处是已知的、已被显式建模而不是失败的行为，M1 需要知道：

1. **零宽标记偏移不可逆投影**：例如 `这是**粗体**文本` 里的第 3 个字节（`**` 的第二个 `*`）在可见投影里与 `粗体` 的第一个字节共用同一个渲染位置。无损对能还原它，可见投影不能——这是「渲染侧本就没有这个位置」的必然结果，不是实现缺陷。
2. **`Degraded` span 内部偏移按 clamp 处理**：转义（`\*`）、硬换行（行尾两空格 / `\`）、HTML 实体这类「源码与渲染文本不等长」的片段，其内部偏移在可见投影里被夹到片段渲染长度内。它们数量极少（混合样本里只来自转义与硬换行），且无损对仍然精确。M1 若要把光标停在转义字符中间，需要给 `Degraded` 补一张逐字符对齐表。

### 已知解析器偏差（交 M1）：闭合 `**` / `*` 紧邻 emoji

**现象**：`outer **bold with *italic* inside** end.` 解析正确；但把闭合 `**` 的左邻字符换成 emoji 时，外层 strong 不成立，两个 `**` 作为普通文本留在 `block.render` 里，渲染态因此会画出它们。组 3 在 H1 一节的 §6 记录的是同一件事的现象层证据（他们的 `A1`/`A2` 断言证明这不是渲染器绘制零宽 `Marker` 造成的）。

**5 组对照（实测，取 `Document::parse(...).blocks()[0].render`）**：

| 输入 | 外层 strong | 实测 render |
| --- | --- | --- |
| `外层**粗体内容**结尾。` | 成立 | `外层粗体内容结尾。` |
| `外层**粗体里嵌 *斜体* 尾巴**结尾。` | 成立 | `外层粗体里嵌 斜体 尾巴结尾。` |
| `外层**粗体里嵌 *斜体中文* 与外层**结尾。` | 成立 | `外层粗体里嵌 斜体中文 与外层结尾。` |
| `外层**粗体里嵌 *斜体* 与 emoji 😀**结尾。` | **不成立** | `外层**粗体里嵌 斜体 与 emoji 😀**结尾。`（残留 `**`） |
| `outer **bold with *italic* inside** end.` | 成立 | `outer bold with italic inside end.` |
| 规避写法：`… 与外层 emoji 😀 的粗体**结尾。` | 成立 | 与第 2/3 行同类，无残留 |

唯一相关变量是**闭合 `**` 的左邻是否为 emoji**；与嵌套、CJK、块类型都无关。

**根因**：`pulldown-cmark 0.13.5` 在判定分隔符 run 的 right-flanking 属性时，把 emoji（Unicode 类别 `So`，符号-其他）算进了标点。CommonMark 的标点定义是 Unicode `P*`（标点）类，`So` 不在其中，所以 `😀**` 本应可以闭合外层 strong。**这是解析器的库级偏差，不是 H3 映射的缺陷**：同一份偏差样本上，`src_to_render` / `render_to_src` 往返仍逐字节成立，内层 `*斜体*` 的分隔符仍是零宽 `Marker`（回归测试同时断言了这两点）。它也不是渲染器缺陷：零宽 `Marker` 从不被绘制，`**` 之所以可见，是因为它们在 AST 里就是 `SpanKind::Text`。

**影响面**：闭合 `**` / `*` 的左邻是 emoji 或类似 `So`/`Sk` 类符号（😀 ✅ ☺ 等）时，外层强调不出、标记字符会画进渲染态。开标签后紧跟 emoji 不受影响；emoji 不与分隔符相邻时也不受影响。`sample.md` 第 23 行已改成规范写法（emoji 与闭合 `**` 之间隔一个空格与正文），渲染态不再残留标记。

**M1 的三个候选处置**：

1. **换解析器**（comrak / markdown-rs / tree-sitter-markdown）：一次性解决，但要重做本 spike 的「未覆盖区间即标记」适配层——`doc.rs` 现在依赖 pulldown-cmark 的事件字节区间。
2. **给 `pulldown-cmark` 打补丁**（vendor 一份，把 flanking 的标点判定改成只看 `P*`）：改动最小、语义最准，代价是维护 fork 并跟随版本升级。
3. **自写 flanking 判定**：在解析前把「`**` 前是 `So`」的输入重写成等价规范写法。代价最大：喂给解析器的字节与被映射的缓冲区不再是同一份，映射层要额外补偿，不推荐。

对应回归测试：`doc::tests::known_deviation_emoji_after_closing_strong_keeps_markers`。它断言的是**当前已知的错误行为**（`render` 里仍含 `**`）+ 往返成立 + 内层 Marker 零宽 + 规避写法无残留；M1 修掉偏差后这条测试会失败，届时把断言改成修正后的期望即可（测试名与注释都写明了这一点，别让它拦住修复）。

### 实现代价

- 代码规模（`[System.IO.File]::ReadAllLines(f).Count`）：`src/doc.rs` 1156 行（实现 760 + 测试 396，含 1 条已知偏差回归测试）、`src/sample.rs` 180 行、`src/lib.rs` 21 行、`src/bin/dump.rs` 54 行；组 1/2 交付合计 1411 行（`src/main.rs` / `render.rs` / `view.rs` 归任务组 3、4）。全部是纯数据层，无 UI 依赖，单测毫秒级。组 7 收口时 `src/lib.rs` 已是 43 行（多了 `perf` 模块与导出），H3 的 25 687 个往返偏移与 9 条 `doc` 单测**未改动**。
- 难度评价：**中**。真正的工作量不在「算区间」，而在两件事——用「未覆盖区间即标记」的扫描把 pulldown-cmark 不给标记位置的缺口（`#`、`**`、反引号、列表标记、围栏行）还原成零宽 span；以及给块/span 定下「无缝铺满」的强不变量，否则往返立刻变成需要 fallback 的模糊判断。

单测总数现在是 **27**：H3 的 9 条（组 1/2）+ 组 4 的 3 条 + 组 5 的 5 条 + 组 6 的 4 条 + 组 7 的 5 条（块级缓存命中即等几何 / 编辑使缓存整体失效 / 几何以块顶为 0 存 / 视口带的推导 / `source_slice` 只读一块）+ 1 条样本生成器自检。

---

## 未覆盖与开放事项（U1–U4）

判定栏里凡是「没测到」的，一律记在这里，不写成通过。

| # | 未覆盖项 | 现状 | 谁来解决 |
| --- | --- | --- | --- |
| **U1** | 真实 `WM_IME_COMPOSITION` / `WM_IME_ENDCOMPOSITION` 会不会到达 `EntityInputHandler`，候选窗的视觉位置是否真的贴着光标 | 只有合成回调序列（I0–I8）通过。需要人工按 **H2 §4** 的 V1–V6 打一遍字并回读 `ime.log`；断点判读表在 **H2 §3**。**这正是 H2 判定写「待人工确认」的原因** | 用户（10 分钟） |
| **U2** | 平台剪贴板的**真实往返**（Ctrl+C 写入系统剪贴板、再从别的进程粘贴回来） | **not exercised（不是通过）**。组 5 实测：本环境（agent 会话启动的进程）拿不到 Win32 剪贴板 —— 写后**同 tick** 读回 `None`，**跨 tick** 读回仍是 `None`，连 PowerShell 的 `Set-Clipboard` 也失败。`--dump-select` 的 S5 / S5b 因此都记成 `not exercised`，而**应用内**的复制/粘贴幂等是靠 `MarkdownView::clipboard` 影子副本验证的（走 `paste_text`，不碰系统剪贴板），那部分是 passed。结论：**「复制得到连续源码文本」成立**（`selection::selection_text` 是连续源码字节，单测 + 探针断言）；**「与外部程序交换剪贴板」未验证** | 用户（真机 `cargo run --release -- sample.md` 里 Ctrl+C / Ctrl+V 试一次） |
| **U3** | 真正的「冷盘」冷启动 | 7.2 的 523–538 ms 是**文件系统缓存已热**的三次运行（二进制与 182 KB 文档都在系统缓存里），不是开机后第一次启动。要测冷盘得先 `Clear-RecycleBin`/清系统文件缓存（需管理员），本轮做不到。真实冷盘只会更慢 | 用户（或接受 528 ms 作为偏乐观的估计） |
| **U4** | 纯 TSF-only 输入法是否走 IMM32 兼容层 | 未验证 | 用户（H2 §4 的 V1 换一个输入法再跑一遍） |

