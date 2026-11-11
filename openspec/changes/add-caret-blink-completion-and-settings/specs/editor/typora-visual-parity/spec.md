# typora-visual-parity Specification Delta

## Purpose

Defines the measured Typora visual contract that every typors surface must match, and the reference harness that proves it.

## ADDED Requirements

### Requirement: Reference tokens

The UI SHALL use Typora's default light theme tokens: background `#ffffff`, text `#333333`, selection `#B5D6FC`, link `#4183C4`, window border `#e9e9e9`, hover `rgba(229,229,229,0.59)`, muted control text `#777777`. Dark theme SHALL use Typora `night.css` equivalents. No component may hard-code a color outside the token set.

#### Scenario: Light theme token check

- **WHEN** a surface is rendered in light theme
- **THEN** its background is exactly `#ffffff`, body text `#333333`, selection `#B5D6FC`, and separators `#e9e9e9`

### Requirement: Window chrome geometry

The window SHALL use a 28 px header, a menu row whose dropdown opens at `top:26px`, a content area starting at `top:28px`, and a 28 px status bar. The header title SHALL be 12 px with 8 px left padding; menu items SHALL be 13 px with Typora's `3pt 24px 2pt 24px` item padding. The status bar SHALL place outline/source controls left and the word count right, with 12 px text.

#### Scenario: Reference window

- **WHEN** typors is captured at a 1047×689 client area in light theme
- **THEN** header height is 28 px, content begins at y=28, the status bar occupies the bottom 28 px, and the menu dropdown's top edge is y=26

### Requirement: Page and typography

The page SHALL be centered with `max-width: 860px`, `padding: 30px`, `padding-bottom: 100px`, body font `Open Sans` at `16px` with `line-height: 1.6`, and heading sizes `2rem, 1.8rem, 1.6rem, 1.4rem, 1.2rem, 1rem`. At viewport widths ≥1400 and ≥1800 the max width SHALL become 1024 and 1200 respectively. Code SHALL use `0.9rem` of the monospace stack `"Lucida Console", Consolas, "Courier", monospace`.

#### Scenario: Blank page caret position

- **WHEN** an empty document is shown at 1047×689
- **THEN** the first line starts at x=123.5 (±0.5) and y=58 (±0.5), matching Typora's centered 860 px page plus 30 px padding and 28 px header

#### Scenario: Wide viewport

- **WHEN** the client width is 1500 px
- **THEN** the page max width is 1024 px, not 860 px

### Requirement: Surface components

Menu dropdowns, completion popups, the settings surface and the unsaved-changes dialog SHALL use Typora's measured borders, radii, shadows and spacing: 1 px `#e9e9e9` borders, 0 radius for menus/dialogs, `0 15px 30px rgb(0 0 40 / 20%)` for modal content, 13 px menu text, 28 px menu item height, and the same hover fill as Typora.

#### Scenario: Menu item geometry

- **WHEN** a menu dropdown is open
- **THEN** each item is 28 px high with left padding 24 px and the highlighted item uses the hover token

### Requirement: Pixel reference harness

The repository SHALL contain reference captures of Typora and typors at identical client sizes plus a diff tool. For each fixture, background/border pixels SHALL match exactly; text bounding boxes SHALL be within 1 px; and glyph antialiasing differences SHALL be reported separately rather than failing the geometric contract.

#### Scenario: Chrome fixture

- **WHEN** the reference harness compares the chrome fixture
- **THEN** it reports zero geometry mismatches and zero non-antialiasing color mismatches, or lists the exact coordinates that differ

#### Scenario: Settings fixture

- **WHEN** the settings surface is captured at the reference size
- **THEN** the left rail width, row heights, control positions and colors match the Typora settings reference within the same tolerance
