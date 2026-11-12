# Chart 接口文档

- 模块：`typors` 围栏代码块 `chart` 原生图表
- 版本：1.1.0
- 状态：稳定
- 渲染方式：进程内生成 SVG，无浏览器、无 JavaScript、无网络访问
- 适用对象：文档作者、主题/插件开发者、需要按本接口生成图表的工具作者

本文档是 `chart` 块的接口契约。实现位于 `crates/typors-media/src/chart.rs`，文档中的每个示例都会被测试渲染；解析器接受的每个选项都必须出现在下表中。

## 1. 概要

一个 `chart` 围栏块描述一张图表：第一行是图表类型，其后每行是 `键: 值` 形式的赋值。变量定义数据，`变量.选项` 设置系列样式，保留的全局选项控制标题、尺寸、坐标轴与柱状图布局。

## 2. 语法

```text
chart         = type-line, { blank-line | assignment } ;
type-line     = "pie" | "line" | "bar" | "scatter" ;
assignment    = global-option | series-data | series-option ;
global-option = ("title" | "width" | "height" | "labels" | "stacked" | "horizontal" | "values"), ":", value ;
series-data   = series-name, ":", ( array | number | percentage | label ) ;
series-option = series-name, ".", option-name, ":", value ;
series-name   = ? 不含 "." 的非空文本，区分大小写 ? ;
array         = "[", [ value, { ",", value } ], "]" ;
value         = number | label ;
```

规则：

- 空行被忽略；`#` 不是注释，颜色值中可以直接写 `#`。
- 首个非空行必须是图表类型，否则报错。
- 同一变量可以分多行定义数据和选项。
- 全局选项名优先于同名变量：`stacked: true` 不会创建名为 `stacked` 的系列。
- 未知的 `系列.选项` 是错误；未知的裸键会被当作系列名（这是唯一会把拼写错误变成空系列的地方）。

## 3. 数据类型

### 3.1 数值与百分比

```chart
pie

chrome: 62%
firefox: 18%
safari: 20%
```

- `62%`、`18`、`2024` 都按数值解析。
- 饼图中带 `%` 时以 100 为分母；不足 100 的部分补一个灰色“未定义”扇区，且不显示名称。

### 3.2 数组

```chart
line

a: [1, 2, 3, 4, 5]
b: [2, 4, 3, 5, 4]
```

数组是 `line`、`bar`、`scatter` 的数据点；多个数组按索引对齐，短序列只画自己的点。

### 3.3 标签

```chart
bar

labels: [一月, 二月, 三月]
sales: [12, 18, 9]
```

分类轴标签优先级：全局 `labels` > 第一个定义了 `x`/`labels` 的系列 > 从 1 开始的索引。标签超出槽宽时以 `…` 截断，不会与相邻标签重叠。

## 4. 全局选项

| 选项 | 类型 | 默认 | 范围/取值 | 适用类型 | 说明 |
| --- | --- | --- | --- | --- | --- |
| `title` | 字符串 | 无 | 任意文本 | 全部 | 图表标题，居中显示 |
| `width` | 数值 | 饼图 440，其他 560 | 240–1600 | 全部 | SVG 逻辑像素宽，超出页宽时按比例缩放 |
| `height` | 数值 | 饼图 300，其他 340 | 160–1200 | 全部 | SVG 逻辑像素高 |
| `labels` | 标签数组 | 无 | 任意文本数组 | bar | 分类轴标签 |
| `stacked` | 布尔 | `false` | `true`/`false`/`1`/`0`/`yes` | bar | 系列按类别累加堆叠 |
| `horizontal` | 布尔 | `false` | 同上 | bar | 类别沿 Y 轴、数值沿 X 轴 |
| `values` | 布尔 | `false` | 同上 | bar | 在柱端显示数值 |

示例：

```chart
line

title: 访问量趋势
width: 640
height: 360

visits: [120, 180, 150, 240, 300]
visits.label: 访问量
visits.color: #2E86DE
visits.marker: o
```

## 5. 系列选项

| 选项 | 类型 | 默认 | 适用类型 | 说明 |
| --- | --- | --- | --- | --- |
| `color` | 颜色 | 调色板 | 全部 | 系列颜色，`#RRGGBB` 或 CSS 颜色名 |
| `label` | 字符串 | 变量名 | 全部 | 图例显示名 |
| `marker` | 字符串 | 无 | line/scatter | 每个数据点上方叠加的字符 |
| `line` | 布尔 | scatter 为 `false` | line/scatter | 是否连线 |
| `values` | 布尔 | `false` | bar | 仅该系列显示数值标签 |
| `x` / `labels` | 标签数组 | 无 | line/bar/scatter | 该系列自己的分类标签 |

## 6. 类型

### 6.1 pie

每个变量是一个扇区，标量相加后按比例绘制。默认调色板为 Tableau 10 色，未定义部分固定为 `#B0B0B0`。

```chart
pie

a: 10%
b: 30%
c: 50%

a.color: #FF0000
a.label: Test
```

### 6.2 line

数组按索引连线，`marker` 在每个点叠加字符；`line: false` 只画点。

```chart
line

a: [1, 2, 3, 4, 5]
b: [5, 4, 3, 2, 1]

a.color: #FF0000
a.label: Test
a.marker: *
```

### 6.3 scatter

默认不连线；`line: true` 连线。

```chart
scatter

a: [1, 2, 2, 4, 5]
b: [1, 4, 3, 8, 7]

a.label: 实验组
b.label: 对照组
```

### 6.4 bar

柱状图面向分类比较，语义如下：

- **分组**：同一类别下，多个系列并排绘制，组内顺序按系列名排序；柱宽 = clamp(组宽/系列数 - 2, 2, 36) 像素。
- **分类轴**：标签来源见 3.3；垂直柱状图显示在 X 轴下方，水平柱状图显示在左侧。
- **零基线**：数值轴始终包含 0；正值向上/向右，负值向下/向左。
- **数值标签**：`values: true`（全局或系列）时显示；柱体在数值方向小于 12 px 时省略。
- **水平**：`horizontal: true` 时类别沿 Y 轴排列，正柱向右、负柱向左。
- **堆叠**：`stacked: true` 时同一类别的系列累加，正值向上、负值向下分别累加。

分组的两个系列：

```chart
bar

q1: [12, 18, 9]
q2: [8, 14, 19]

q1.label: 一季度
q2.label: 二季度
q1.color: #4E79A7
q2.color: #F28E2B
```

分类标签与数值标签：

```chart
bar

labels: [一月, 二月, 三月]
values: true
sales: [12, 18, 9]
sales.color: #59A14F
```

负值与零基线：

```chart
bar

labels: [盈利, 亏损]
delta: [12, -7]
delta.color: #E15759
```

水平柱状图：

```chart
bar

horizontal: true
labels: [北京, 上海, 广州]
visits: [120, 90, 60]
visits.color: #76B7B2
```

堆叠柱状图：

```chart
bar

stacked: true
labels: [Q1, Q2]
online: [30, 40]
retail: [20, 10]
online.label: 线上
retail.label: 线下
```

## 7. 坐标轴与刻度

| 类型 | 数值轴 | 刻度 | 分类轴 |
| --- | --- | --- | --- |
| bar（垂直） | Y 轴，5 个等距刻度 | 含 0；跨零时加粗零线 | X 轴下方，超宽截断 |
| bar（水平） | X 轴，5 个等距刻度 | 同上 | 左侧，最大 80 px，超出截断 |
| line/scatter | Y 轴，5 个等距刻度 | 含 0 | 无分类轴文字 |
| pie | 无 | 无 | 图例 |

刻度值格式：整数不带小数，其余保留 1 位小数。

## 8. 图例与布局

- 图例位于绘图区右上角，每个系列一行，色块 10×10 px。
- 标题位于顶部居中，字体 14 px、600 字重。
- 图表整体使用逻辑像素；`width` 超过页宽时按比例缩放并居中。

## 9. 错误与回退

图表块出错时不会让文档崩溃：块回退为源码文本并保留块高度。

| 错误 | 触发条件 | 提示 |
| --- | --- | --- |
| 类型错误 | 首行不是 pie/line/bar/scatter | `chart: first line must be pie/line/bar/scatter` |
| 无数据 | 只有类型行 | `chart: no series defined` |
| 缺冒号 | 某行没有 `:` | `chart line N: expected key: value` |
| 空键 | 冒号前为空 | `chart line N: empty key` |
| 未知选项 | `系列.未知选项` | `chart line N: unknown option 'x' on series 'name'` |
| 空数据 | 所有系列都没有有限数值 | 空图提示 `no data` |

## 10. 兼容性与变更记录

| 版本 | 变更 |
| --- | --- |
| 1.0.0 | pie/line/bar/scatter 初版；多系列柱重叠绘制；`x`/`labels` 仅用于图例 |
| 1.1.0 | bar 分组并排；`labels` 分类轴；`values` 数值标签；负值零基线；`horizontal`、`stacked`；未知选项报错；本文件重写为接口文档 |

兼容性说明：1.0.0 的多系列 bar 会完全重叠（后画的系列覆盖先画的）。1.1.0 起改为并排分组，这是有意的行为变更。pie/line/scatter 的输出未改变。

## 11. 示例索引

| 示例 | 章节 |
| --- | --- |
| 饼图与百分比补全 | 3.1、6.1 |
| 双折线 | 3.2、6.2 |
| 分类柱状图 | 3.3、6.4 |
| 标题/尺寸/颜色/标记 | 4、6.2 |
| 分组柱、数值标签、负值、水平、堆叠 | 6.4 |
| 散点 | 6.3 |
