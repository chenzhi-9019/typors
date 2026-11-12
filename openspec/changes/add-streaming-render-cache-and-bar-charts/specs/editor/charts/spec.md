# charts Specification Delta

## Purpose

Defines the `chart` fenced-block contract: the grammar of a chart definition, the chart types and their options, how a bar chart draws grouped, stacked, horizontal and negative data, how errors surface, and the reference documentation that must describe exactly this interface.

## ADDED Requirements

### Requirement: Chart block grammar

A `chart` fenced code block SHALL define one chart. Its first non-empty line SHALL be the chart type. Every later non-empty line SHALL be `key: value`, where a key without a dot defines a series and a key of the form `series.option` sets an option on that series. Blank lines SHALL be ignored and `#` SHALL NOT start a comment.

#### Scenario: Minimal chart

- **WHEN** a block contains `pie` followed by `a: 10%` and `b: 30%`
- **THEN** a pie chart with two sectors is rendered

#### Scenario: Blank lines and colour values

- **WHEN** the block contains blank lines and `a.color: #FF0000`
- **THEN** the blank lines are ignored and the colour is applied rather than treated as a comment

### Requirement: Supported types and type errors

The chart types SHALL be `pie`, `line`, `bar` and `scatter`. Any other first line, or a block with no series, SHALL produce a chart error for that block, and the block SHALL fall back to its Markdown source while keeping its block height.

#### Scenario: Unknown type

- **WHEN** the first line is `donut`
- **THEN** the block shows its source text, does not crash, and the layout height stays reserved

#### Scenario: No data

- **WHEN** the block contains only the type line
- **THEN** the block reports the "no series defined" error

### Requirement: Series data forms

A series value SHALL be an array of numbers, a scalar number, a percentage scalar, or a label string. Arrays SHALL be the data points for `line`, `bar` and `scatter`; scalars SHALL be shares for `pie`. Series SHALL be ordered deterministically by name so colours and legend order do not change between frames.

#### Scenario: Two arrays align by index

- **WHEN** series `a` has three values and series `b` has five
- **THEN** both are drawn, aligned by index, and `b` draws two more points than `a`

#### Scenario: Percentage scalars

- **WHEN** a `pie` has percentages totalling 90%
- **THEN** the missing 10% is drawn as a grey undefined sector with no legend entry

### Requirement: Global options

The reserved global options SHALL be `title`, `width`, `height`, `labels`, `stacked`, `horizontal` and `values`. `width` SHALL clamp to 240–1600 and `height` to 160–1200. The reserved names SHALL take precedence over series of the same name, and the reference documentation SHALL list them as reserved.

#### Scenario: Size is clamped

- **WHEN** a chart sets `width: 4000`
- **THEN** the rendered chart is 1600 logical pixels wide

#### Scenario: Reserved name is not a series

- **WHEN** a `bar` chart sets `stacked: true`
- **THEN** stacking is enabled and no series named `stacked` appears in the legend

### Requirement: Series options

Each series SHALL accept `color` (`#RRGGBB` or a CSS colour name), `label` (legend text, defaulting to the series name), `marker` (a character drawn at each point), `line` (boolean, whether `scatter` joins points), and `x`/`labels` (the series' own category labels). An unknown `series.option` SHALL be a chart error naming the option.

#### Scenario: Colour and label

- **WHEN** a series sets `a.color: #FF0000` and `a.label: 测试`
- **THEN** the series is red in the plot and the legend shows 测试

#### Scenario: Unknown option

- **WHEN** a series sets `a.colour: #FF0000`
- **THEN** the block reports an error naming `colour` and falls back to source

### Requirement: Bar series draw side by side

In a `bar` chart, series that share a category index SHALL be drawn side by side within the category slot instead of overlapping. Group order SHALL follow series order, group width SHALL leave a visible gap between categories, and every series SHALL remain individually visible and colour-distinguishable.

#### Scenario: Two series in one category

- **WHEN** `q1: [12, 18, 9]` and `q2: [8, 14, 19]` are drawn as `bar`
- **THEN** each category shows two adjacent bars of different colours, neither covering the other

#### Scenario: Many series

- **WHEN** a bar chart has more series than fit comfortably in a slot
- **THEN** bars narrow to a documented minimum width and remain separated rather than merging

### Requirement: Bar category axis labels

A `bar` chart SHALL render category labels under the x axis: the global `labels` option when present, otherwise the labels of the first series (in series order) that defines `x` or `labels`, otherwise the 1-based category index. Labels that exceed the slot SHALL be truncated with an ellipsis rather than overlapping their neighbour.

#### Scenario: Explicit labels

- **WHEN** a bar chart sets `labels: [一月, 二月, 三月]`
- **THEN** the three categories are labelled 一月, 二月 and 三月 under the axis

#### Scenario: No labels

- **WHEN** a bar chart defines values but no labels
- **THEN** the categories are labelled 1, 2, 3

#### Scenario: Long labels

- **WHEN** a category label is wider than its slot
- **THEN** it is shortened with an ellipsis and adjacent labels do not collide

### Requirement: Bar value range and zero baseline

A `bar` chart's value axis SHALL always include zero, and bars SHALL grow from the zero baseline: positive values upward, negative values downward. Ticks SHALL be computed from the full value range including negatives.

#### Scenario: Negative value

- **WHEN** a bar series contains `-5`
- **THEN** that bar is drawn below the zero line with its correct magnitude and the axis shows negative ticks

#### Scenario: All positive values

- **WHEN** every value is positive
- **THEN** the zero baseline is the bottom of the plot area and bars start there

### Requirement: Bar value labels

When `values: true` is set globally, or on a series, the numeric value SHALL be printed at the end of each affected bar, above a vertical bar and to the right of a horizontal one, formatted with a documented rule and omitted when it would not fit inside the plot area.

#### Scenario: Labels on top of bars

- **WHEN** a bar chart sets `values: true`
- **THEN** each bar shows its value and the label does not overlap the neighbouring bar

#### Scenario: Labels disabled

- **WHEN** `values` is absent or `false`
- **THEN** no value text is drawn

### Requirement: Horizontal and stacked bars

A `bar` chart SHALL support `horizontal: true`, drawing categories down the y axis with bars extending right for positive values and left for negative ones, and `stacked: true`, drawing series cumulatively so each category's total is the sum of its series, with negative values stacked downward from zero.

#### Scenario: Horizontal bars

- **WHEN** `horizontal: true` is set
- **THEN** categories run down the left axis and bar lengths encode the values

#### Scenario: Stacked bars

- **WHEN** `stacked: true` is set for two series
- **THEN** the second series starts where the first ends and the category total is the sum of both

#### Scenario: Stacked with negatives

- **WHEN** a stacked bar has a negative value
- **THEN** negative segments stack downward from zero and positive segments upward

### Requirement: Chart rendering stays native and bounded

Charts SHALL be rendered in-process to SVG with no browser, JavaScript or network access, SHALL respect the configured `width` and `height`, and SHALL be scaled to fit the page width without changing their aspect ratio. Chart rendering failure SHALL NOT abort the document layout.

#### Scenario: Oversized chart

- **WHEN** a chart's width exceeds the page width
- **THEN** it is scaled down proportionally to fit and stays centered

#### Scenario: Render failure

- **WHEN** a chart cannot be rendered
- **THEN** the rest of the document still lays out and the block shows its source

### Requirement: Reference documentation is the interface contract

`docs/chart.md` SHALL be a reference document, not a tutorial: it SHALL document the grammar, every type, every reserved global option and every series option with type, default and range, the bar semantics above, axis/tick/legend rules, the documented error messages, and a versioned change log. The document SHALL describe exactly the interface the parser accepts, and a check SHALL fail when a documented example does not parse or an implemented option is undocumented.

#### Scenario: Every documented example renders

- **WHEN** the chart documentation check extracts the fenced `chart` examples from `docs/chart.md` and renders them
- **THEN** every example parses and renders without error

#### Scenario: Undocumented option

- **WHEN** the parser accepts a reserved option that the reference does not list
- **THEN** the documentation check fails and names the option

#### Scenario: Out-of-date bar section

- **WHEN** bar grouping, labels, stacking or horizontal behaviour changes
- **THEN** the reference is updated in the same change and its change log records the version
