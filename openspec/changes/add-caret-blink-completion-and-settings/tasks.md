# Tasks

## 1. Typora reference baseline

- [ ] 1.1 Create `docs/reference/typora/` captures of the installed Typora at 1047×689 for blank chrome, open File menu, Preferences (each category), completion popup and unsaved dialog; verify each PNG is a client-area crop, not a desktop capture.
- [ ] 1.2 Add `tools/visual-diff.ps1` that captures typors at the same client size and reports exact background/border mismatches, text bbox deltas and a separate glyph-antialiasing mask; verify it exits non-zero on a deliberately shifted fixture.
- [ ] 1.3 Record the measured token table (header 28, menu 26, page 860/1024/1200, padding 30, body Open Sans 16/1.6, footer 28, colors `#fff/#333/#B5D6FC/#4183C4/#e9e9e9`) in `docs/design/typora-tokens.md` and verify each value against the Typora CSS files and reference crops.
- [ ] 1.4 Add the blank-page fixture assertion: caret x=123.5±0.5, y=58±0.5 at 1047×689, and 1024 px page at 1500 px client width; run it through the harness.

## 2. Theme and page geometry

- [x] 2.1 Split `EditorTheme` into a palette + typography metrics and add `from_settings`; verify light tokens match `#fff/#333/#B5D6FC/#4183C4/#e9e9e9` and dark tokens come from `night.css`.
- [x] 2.2 Apply page `max-width` 860/1024/1200 at the CSS breakpoints, 30 px padding and 100 px bottom padding in `MarkdownView`; verify fixture 1.4 and that existing `--dump-layout` still passes.
- [ ] 2.3 Set body font to `Open Sans` 16 px/1.6 and heading sizes `2/1.8/1.6/1.4/1.2/1rem`; verify heading/code line heights through `--dump-layout` and the visual harness.
- [x] 2.4 Add the 28 px status bar (source toggle left, `N 词` right, 12 px text) and adjust the scroll viewport so the page ends above it; verify no caret clipping and the status fixture.

## 3. Caret blink

- [ ] 3.1 Add `BlinkState` to `MarkdownView` with enabled/interval/generation and a `caret_visible` paint gate; unit-test visible→hidden→visible timing with a fake clock.
- [ ] 3.2 Reset blink to visible from edits, caret moves, selection, mouse, scroll, IME update, focus gain and menu close; test each reset path and that an in-flight timer cannot override a newer generation.
- [ ] 3.3 Start/stop the timer only while focused and not composing; verify no repaint while unfocused and continuous visibility during IME composition.
- [x] 3.4 Add `--dump-blink` with a deterministic timeline (530 ms phases, resets, disabled mode) and run it to exit 0.

## 4. Markdown completion

- [x] 4.1 Add `typors-editor::completion` with the minimal catalog and `suggest` context/ranking rules; unit-test heading/list/fence/math/link/image/table triggers and no-match prose.
- [ ] 4.2 Trigger suggestions on the relevant character edits with a configurable debounce, disabled during IME; unit-test that a newer edit cancels the older debounce.
- [ ] 4.3 Render the Typora-styled popup above/below the caret from `EditorShell`; verify geometry against the completion fixture.
- [ ] 4.4 Implement Up/Down/Enter/Tab/Esc and mouse accept; `accept_completion` replaces the prefix in one undoable edit; test one Ctrl+Z removes the insertion.
- [ ] 4.5 Respect the completion enable switch and delay; disabling closes an open popup; add `--dump-completion` and run it to exit 0.

## 5. Unsaved-changes guard

- [ ] 5.1 Centralize dirty/title state (`*` marker, saved revision, autosave-cleared dirty) in the shell; unit-test title transitions after edit/save/autosave.
- [ ] 5.2 Implement `confirm_close` with Save/Don't Save/Cancel and wire `on_window_should_close` plus File→Exit/Alt+F4; test the three answers and failed-save veto.
- [ ] 5.3 Verify clean documents close silently, autosaved documents close silently, and untitled autosave still prompts; add `--dump-close` with the decision matrix.

## 6. Settings model and persistence

- [x] 6.1 Create `crates/typors-settings` with the versioned serde schema, Typora defaults and validation ranges; unit-test defaults, round-trip, out-of-range errors and missing fields via serde defaults.
- [ ] 6.2 Implement atomic load/save at `%APPDATA%\typors\settings.json`, `.bak` recovery for corrupt JSON and a non-blocking recovery warning; test corrupt/partial/unwritable files.
- [ ] 6.3 Implement settings application: theme/font/line/page width, autosave/prompt/restore, completion/blink/pairing, shortcuts and language; unit-test that applying a settings snapshot changes the expected theme and view fields.
- [ ] 6.4 Implement shortcut capture/conflict validation and the zh/en language lookup; unit-test duplicate rejection and menu string switching.
- [ ] 6.5 Add `--dump-settings` covering defaults, validation errors, persistence and corrupt recovery; run it to exit 0.

## 7. Preferences UI

- [ ] 7.1 Add `PreferencesState` + modal overlay to the shell with the 240 px rail, four categories and Typora colors/spacing; verify the settings fixture and Esc/cancel behavior.
- [ ] 7.2 Implement the 外观 pane (theme, font family, size, line height, page width) with live preview and validation; verify changes apply before 确定 and cancel reverts.
- [ ] 7.3 Implement 文件与保存 and 编辑 panes (autosave, prompt, restore, default dir; completion delay, blink interval, pairing); verify each control persists and validates.
- [ ] 7.4 Implement 快捷键与语言 pane (reference table, rebinding, conflict message, language selection); verify shortcut and language fixtures.
- [ ] 7.5 Implement 恢复默认值 with confirmation, 取消/确定 semantics and focus return; verify all fields reset and the editor caret is unchanged.

## 8. Integration, probes and docs

- [ ] 8.1 Extend the visual harness to menus, preferences, completion and save dialog; run all fixtures and attach the report to the change.
- [ ] 8.2 Run `cargo test --workspace` and every `--dump-*` (including `blink`, `completion`, `settings`, `close`); all must exit 0.
- [ ] 8.3 Update README and `docs/` with Typora tokens, settings path/schema, shortcut/language behavior, blink/completion behavior and the visual-diff workflow; verify the documented commands run as written.
- [ ] 8.4 Build release, run a real-window checklist (blink, completion keyboard/mouse, close X/Alt+F4, every settings category, theme/language switch, restart persistence) and capture the final screenshots.

## Workflow follow-up

- Review the artifacts before implementation; this proposal intentionally stops at planning.
- After implementation and review, archive the change and re-run `openspec validate --all --strict`.

## 9. 追加：原生图表、目录与无控制台窗口

- [x] 9.1 实现 ```chart 围栏的原生 SVG 图表（pie/line/bar/scatter），支持 Matplotlib 风格的 `series: values` 与 `series.option: value`；`cargo test -p typors-media chart` 通过。
- [x] 9.2 在 `sample.md` 加入饼图与折线图，实机渲染确认；`--dump-media` 识别 chart 对象并 exit 0。
- [x] 9.3 实现 `[toc]` 自动目录：按标题层级生成缩进列表，保留源码块；`cargo test -p typors-core toc` 与实机截图通过。
- [x] 9.4 写 `docs/chart.md`，覆盖图表类型、变量/选项、全局选项、示例和限制。
- [x] 9.5 将 `typors.exe` 改为 Windows GUI 子系统，双击/启动不出现控制台；`dumpbin -headers` 显示 subsystem 2。

## 10. 性能与 DLL 拆分

- [x] 10.1 状态栏词数按 `Document::revision` 缓存，避免每帧复制全文；`--dump-perf` 记录布局耗时。
- [x] 10.2 将 Merman/全部图表族移入独立 `typors_mermaid.dll`（C ABI，`libloading` 懒加载）；runtime DLL 从 39 MB 降到 16 MB（release）。
- [x] 10.3 NSIS 安装器与卸载器包含/删除 `typors_mermaid.dll`；安装到临时目录后 `--dump-media` 能加载 Mermaid。
- [ ] 10.4 首帧 prepaint 仍约 55 ms，需要继续做 paint 分段计时与按需资源加载；当前只完成了 DLL/内存拆分，未证明交互帧流畅。
