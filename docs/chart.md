# Chart 语法

typors 用围栏代码块 ```` ```chart ```` 渲染原生图表。语法按 Matplotlib 的习惯：第一行是图表类型，后面用 `变量: 值` 定义数据，用 `变量.选项: 值` 配置样式与图例。图表完全在进程内画成 SVG，不依赖浏览器、JavaScript 或 Node。

## 最小示例

````markdown
```chart
pie

a: 10%
b: 30%
c: 50%

a.color: #FF0000
a.label: Test
```
````

这会画一个饼图：`a` 为红色且图例显示 `Test`，`b`/`c` 使用默认配色；因为三者合计 90%，剩余 10% 会自动画成灰色，且不显示名称。

## 通用结构

```chart
<类型>

<变量>: <值>
<变量>.<选项>: <值>

<全局选项>: <值>
```

- 类型必须是 `pie`、`line`、`bar`、`scatter` 之一。
- 空行会被忽略；`#` 不是注释，颜色值里可以包含 `#`。
- 变量名区分大小写；同一变量可以分多行定义数据和选项。
- 全局选项：`title`、`width`、`height`。

## 数据类型

### 标量

```chart
pie

chrome: 62%
firefox: 18%
safari: 20%
```

`62%`、`18`、`2024` 都按数值处理。饼图把标量当作份额；带 `%` 时总和以 100 为分母，不足 100 的部分补灰色“未定义”扇区。

### 数组

```chart
line

a: [1, 2, 3, 4, 5]
b: [2, 4, 3, 5, 4]
```

数组是 `line`、`bar`、`scatter` 的数据点。多个数组按索引对齐；长度可以不同，短序列只画自己的点。

### 标签

```chart
bar

sales: [12, 18, 9]
sales.x: [一月, 二月, 三月]
```

`x` 或 `labels` 给每个点提供分类标签；当前版本用于图例/数据提示，不会改变坐标轴文字。

## 变量选项

| 选项 | 适用 | 说明 | 示例 |
| --- | --- | --- | --- |
| `color` | 全部 | 系列颜色，`#RRGGBB` 或 CSS 颜色名 | `a.color: #FF0000` |
| `label` | 全部 | 图例显示名；省略时用变量名 | `a.label: 测试` |
| `marker` | line/scatter | 数据点标记字符 | `a.marker: *` |
| `line` | line/scatter | `true` 时画连线，`false` 只画点 | `a.line: false` |
| `x` / `labels` | line/bar/scatter | 点的标签数组 | `a.x: [A, B, C]` |

## 全局选项

| 选项 | 默认 | 范围 | 说明 |
| --- | --- | --- | --- |
| `title` | 无 | 字符串 | 图表标题，居中显示 |
| `width` | 饼图 440，其他 560 | 240–1600 | SVG 宽（逻辑像素） |
| `height` | 饼图 300，其他 340 | 160–1200 | SVG 高（逻辑像素） |

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

## 类型详解

### pie

每个变量是一个扇区；标量相加后按比例绘制。默认调色板为 Tableau 10 色，未定义部分固定为 `#B0B0B0`。图例只显示有 `label` 或变量名的扇区；灰色“未定义”不显示名称。

```chart
pie

a: 10%
b: 30%
c: 50%

a.color: #FF0000
a.label: Test
```

### line

数组按索引连线，`marker` 在每个点叠加一个字符标记；`line: false` 可只显示点。

```chart
line

a: [1, 2, 3, 4, 5]
b: [5, 4, 3, 2, 1]

a.color: #FF0000
a.label: Test
a.marker: *
```

### bar

数组按索引画柱；柱宽由点数自动分配，适合分类对比。

```chart
bar

q1: [12, 18, 9]
q2: [8, 14, 19]

q1.label: 一季度
q2.label: 二季度
q1.color: #4E79A7
q2.color: #F28E2B
```

### scatter

每个点画成圆点；默认不画连线，设置 `line: true` 可连线。

```chart
scatter

a: [1, 2, 2, 4, 5]
b: [1, 4, 3, 8, 7]

a.label: 实验组
b.label: 对照组
```

## 与 Markdown 的配合

- 图表是块级原子对象：光标进入该块时显示源码，离开后显示图表，和 Mermaid/公式一致。
- 图表失败时回退为源码文本并保留块高度，不会让文档崩溃。
- 图表支持 `title`、图例和颜色，但不支持坐标系变换、双 Y 轴、堆叠、误差线等 Matplotlib 高级功能。
- 图表 SVG 使用逻辑像素；`width`/`height` 过大的图会在页宽内按比例缩放。

## 自动目录 `[toc]`

在单独一行写 `[toc]`：

```markdown
[toc]

# 第一章
## 1.1 小节
```

渲染时会自动替换为当前文档所有标题的缩进列表，标题层级对应缩进；`[toc]` 本身仍是一个可编辑的块，光标进入时显示源码。目录不包含 `[toc]` 自身，也不包含代码块里的 `#`。
