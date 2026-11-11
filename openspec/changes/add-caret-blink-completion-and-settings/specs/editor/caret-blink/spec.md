# caret-blink Specification Delta

## Purpose

Makes the insertion caret behave like Typora's blinking caret instead of a permanently painted bar.

## ADDED Requirements

### Requirement: Blink cadence

While the editor is focused and not composing, the caret SHALL alternate visible and hidden with a 530 ms visible phase and a 530 ms hidden phase. The first frame after focus or a caret reset SHALL show the caret. The cadence SHALL not change the caret's position, size or document scroll.

#### Scenario: Caret blinks at rest

- **WHEN** the editor has focus and no input occurs for 1.2 s
- **THEN** the caret is visible for approximately 530 ms, hidden for approximately 530 ms, and visible again, without moving

#### Scenario: No layout movement

- **WHEN** the caret toggles visibility
- **THEN** text, scroll offset and page height are unchanged

### Requirement: Reset on activity

Any caret movement, text input, selection change, click, scroll, menu open/close, IME composition update or window refocus SHALL reset the blink phase to visible and restart the timer. The caret SHALL remain continuously visible during an active IME composition.

#### Scenario: Typing resets the phase

- **WHEN** the caret is in its hidden phase and the user types a character
- **THEN** the caret becomes visible immediately and the hidden phase restarts from that edit

#### Scenario: Composition keeps the caret visible

- **WHEN** an IME composition is active
- **THEN** the caret does not disappear until the composition commits or cancels

### Requirement: Focus and settings

The caret SHALL be hidden when the window is unfocused and SHALL resume blinking at the same document offset when focus returns. Settings SHALL allow blinking to be disabled (caret always visible while focused) and the interval to be changed; changes apply without restart.

#### Scenario: Disable blinking

- **WHEN** the user turns off “光标闪烁” in settings
- **THEN** the focused caret stays visible continuously

#### Scenario: Unfocused window

- **WHEN** the window loses focus during the visible phase
- **THEN** no caret is painted, and refocusing paints it at the unchanged offset
