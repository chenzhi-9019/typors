//! Window shell: a fixed menu row above the editor, plus file state and command
//! dispatch. The editor itself stays unaware of menus, dialogs and save paths.

use std::path::PathBuf;

use gpui_kit::gpui::prelude::FluentBuilder;
use gpui_kit::gpui::prelude::*;
use gpui_kit::gpui::prelude::{InteractiveElement as _, StatefulInteractiveElement as _};
use gpui_kit::gpui::*;
use typors_core::Document;
use typors_editor::view::MarkdownView;
use typors_render::EditorTheme;
use windows_sys::Win32::UI::WindowsAndMessaging::{
    IDNO, IDYES, MB_ICONINFORMATION, MB_ICONWARNING, MB_OK, MB_YESNOCANCEL, MessageBoxW,
};

use crate::file_dialog;
use crate::menu::{MenuCommand, MenuState};

const PREFERENCES_RAIL: usize = 240;

#[derive(Clone, Debug)]
struct PreferencesState {
    open: bool,
    category: usize,
    draft: typors_settings::Settings,
    error: Option<String>,
}

impl PreferencesState {
    fn new(settings: typors_settings::Settings) -> Self {
        Self {
            open: false,
            category: 0,
            draft: settings,
            error: None,
        }
    }

    fn open(&mut self, settings: &typors_settings::Settings) {
        self.open = true;
        self.category = 0;
        self.draft = settings.clone();
        self.error = None;
    }

    fn cancel(&mut self) {
        self.open = false;
        self.error = None;
    }

    fn step_size(&mut self, delta: f32) {
        self.draft.appearance.font_size =
            (self.draft.appearance.font_size + delta).clamp(12.0, 24.0);
    }

    fn step_line(&mut self, delta: f32) {
        self.draft.appearance.line_height =
            ((self.draft.appearance.line_height + delta) * 10.0).round() / 10.0;
        self.draft.appearance.line_height = self.draft.appearance.line_height.clamp(1.2, 2.0);
    }

    fn step_width(&mut self, delta: f32) {
        self.draft.appearance.page_width =
            (self.draft.appearance.page_width + delta).clamp(600.0, 1200.0);
    }

    fn step_delay(&mut self, delta: i32) {
        self.draft.editing.completion_delay_ms =
            (self.draft.editing.completion_delay_ms as i32 + delta).clamp(0, 500) as u32;
    }

    fn step_blink(&mut self, delta: i32) {
        self.draft.editing.blink_interval_ms =
            (self.draft.editing.blink_interval_ms as i32 + delta).clamp(300, 1200) as u32;
    }
}

pub struct EditorShell {
    editor: Entity<MarkdownView>,
    editor_focus: FocusHandle,
    menu_focus: FocusHandle,
    menu: MenuState,
    path: Option<PathBuf>,
    saved_revision: u64,
    theme: EditorTheme,
    settings: typors_settings::Settings,
    preferences: PreferencesState,
    startup_warning: Option<String>,
    last_key: Option<(String, std::time::Instant)>,
    last_title: String,
    last_autosave: std::time::Instant,
    word_count_cache: (u64, usize),
}

impl EditorShell {
    pub fn new(
        editor: Entity<MarkdownView>,
        path: Option<PathBuf>,
        settings: typors_settings::Settings,
        startup_warning: Option<String>,
        cx: &mut Context<Self>,
    ) -> Self {
        let editor_focus = editor.read(cx).focus_handle().clone();
        let saved_revision = editor.read(cx).doc().revision();
        let theme = EditorTheme::from_settings(&settings);
        editor.update(cx, |view, cx| view.apply_settings(&settings, cx));
        Self {
            editor,
            editor_focus,
            menu_focus: cx.focus_handle(),
            menu: MenuState::new(),
            path,
            saved_revision,
            theme,
            preferences: PreferencesState::new(settings.clone()),
            settings,
            startup_warning,
            last_key: None,
            last_title: "typors".to_string(),
            last_autosave: std::time::Instant::now(),
            word_count_cache: (u64::MAX, 0),
        }
    }

    fn title(&self) -> String {
        match &self.path {
            Some(path) => format!(
                "{} - typors",
                path.file_name().unwrap_or_default().to_string_lossy()
            ),
            None => "typors".to_string(),
        }
    }

    fn display_title(&self, cx: &App) -> String {
        let mut title = self.title();
        if self.dirty(cx) {
            title.push('*');
        }
        title
    }

    /// Typora-style close guard shared by File→Exit, the window X and Alt+F4.
    pub fn confirm_close(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> bool {
        if !self.dirty(cx) || !self.settings.files.prompt_on_close {
            return true;
        }
        let text = format!("“{}”有未保存的更改。是否保存？", self.title());
        match discard_answer(message_box(
            &text,
            "typors",
            MB_YESNOCANCEL | MB_ICONWARNING,
        )) {
            DiscardAnswer::Save => self.save_document(cx),
            DiscardAnswer::Discard => true,
            DiscardAnswer::Cancel => false,
        }
    }

    fn dirty(&self, cx: &App) -> bool {
        self.editor.read(cx).doc().revision() != self.saved_revision
    }

    fn close_menu(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.menu.close();
        window.focus(&self.editor_focus, cx);
        cx.notify();
    }

    fn apply_preferences(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let errors = self.preferences.draft.validate();
        if !errors.is_empty() {
            self.preferences.error = Some(
                errors
                    .iter()
                    .map(|e| format!("{}: {}", e.field, e.message))
                    .collect::<Vec<_>>()
                    .join("；"),
            );
            cx.notify();
            return;
        }
        if let Err(error) = self.preferences.draft.save() {
            self.preferences.error = Some(error);
            cx.notify();
            return;
        }
        self.settings = self.preferences.draft.clone();
        self.theme = EditorTheme::from_settings(&self.settings);
        let settings = self.settings.clone();
        self.editor.update(cx, |view, cx| {
            view.apply_settings(&settings, cx);
        });
        self.preferences.cancel();
        window.focus(&self.editor_focus, cx);
        cx.notify();
    }

    fn open_menu(&mut self, index: usize, window: &mut Window, cx: &mut Context<Self>) {
        self.menu.open_menu(index);
        window.focus(&self.menu_focus, cx);
        cx.notify();
    }

    fn current_text(&self, cx: &App) -> String {
        self.editor.read(cx).doc().text()
    }

    fn replace_doc(
        &mut self,
        doc: Document,
        path: Option<PathBuf>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.editor
            .update(cx, |view, cx| view.replace_document(doc, cx));
        self.path = path;
        self.saved_revision = self.editor.read(cx).doc().revision();
        window.set_window_title(&self.title());
        cx.notify();
    }

    fn confirm_discard(&mut self, action: &str, cx: &App) -> bool {
        if !self.dirty(cx) {
            return true;
        }
        let text = format!("“{}”有未保存的更改。保存后再{}吗？", self.title(), action);
        match discard_answer(message_box(
            &text,
            "typors",
            MB_YESNOCANCEL | MB_ICONWARNING,
        )) {
            DiscardAnswer::Save => self.save_document(cx),
            DiscardAnswer::Discard => true,
            DiscardAnswer::Cancel => false,
        }
    }

    fn new_document(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if !self.confirm_discard("新建", cx) {
            return;
        }
        self.replace_doc(Document::parse(""), None, window, cx);
    }

    fn open_document(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if !self.confirm_discard("打开", cx) {
            return;
        }
        let Some(path) = file_dialog::open("打开 Markdown 文件") else {
            return;
        };
        match std::fs::read_to_string(&path) {
            Ok(text) => {
                let doc = Document::parse(&text);
                self.replace_doc(doc, Some(path), window, cx);
            }
            Err(error) => {
                message_box(
                    &format!("无法读取 {}：{error}", path.display()),
                    "typors",
                    MB_OK | MB_ICONWARNING,
                );
            }
        }
    }

    fn save_document(&mut self, cx: &App) -> bool {
        let Some(path) = self.path.clone() else {
            return self.save_as(cx);
        };
        let text = self.current_text(cx);
        match std::fs::write(&path, text) {
            Ok(()) => {
                self.saved_revision = self.editor.read(cx).doc().revision();
                true
            }
            Err(error) => {
                message_box(
                    &format!("无法保存 {}：{error}", path.display()),
                    "typors",
                    MB_OK | MB_ICONWARNING,
                );
                false
            }
        }
    }

    fn save_as(&mut self, cx: &App) -> bool {
        let Some(path) = file_dialog::save("另存为 Markdown 文件") else {
            return false;
        };
        let text = self.current_text(cx);
        match std::fs::write(&path, text) {
            Ok(()) => {
                self.path = Some(path);
                self.saved_revision = self.editor.read(cx).doc().revision();
                true
            }
            Err(error) => {
                message_box(
                    &format!("无法保存 {}：{error}", path.display()),
                    "typors",
                    MB_OK | MB_ICONWARNING,
                );
                false
            }
        }
    }

    fn window_title_after_save(&self, window: &mut Window) {
        window.set_window_title(&self.title());
    }

    fn run(&mut self, command: MenuCommand, window: &mut Window, cx: &mut Context<Self>) {
        match command {
            MenuCommand::New => self.new_document(window, cx),
            MenuCommand::Open => self.open_document(window, cx),
            MenuCommand::Save => {
                if self.save_document(cx) {
                    if let Some(path) = &self.path {
                        window.set_window_title(&format!(
                            "{} - typors",
                            path.file_name().unwrap_or_default().to_string_lossy()
                        ));
                    }
                }
            }
            MenuCommand::SaveAs => {
                if self.save_as(cx) {
                    self.window_title_after_save(window);
                }
            }
            MenuCommand::Exit => {
                if self.confirm_close(window, cx) {
                    cx.quit();
                    return;
                }
            }
            MenuCommand::Undo => {
                self.editor.update(cx, |view, _| {
                    view.undo();
                });
            }
            MenuCommand::Redo => {
                self.editor.update(cx, |view, _| {
                    view.redo();
                });
            }
            MenuCommand::Cut => {
                self.editor.update(cx, |view, cx| {
                    view.cut(cx);
                });
            }
            MenuCommand::Copy => {
                self.editor.update(cx, |view, cx| {
                    view.copy(cx);
                });
            }
            MenuCommand::Paste => {
                self.editor.update(cx, |view, cx| {
                    view.paste(cx);
                });
            }
            MenuCommand::SelectAll => {
                self.editor.update(cx, |view, _| view.select_all());
            }
            MenuCommand::About => {
                message_box(
                    "typors 1.0.1\n混合 Markdown 编辑器\nGPUI 自绘界面",
                    "关于 typors",
                    MB_OK | MB_ICONINFORMATION,
                );
            }
            MenuCommand::Preferences => {
                self.preferences.open(&self.settings);
                window.focus(&self.menu_focus, cx);
            }
            MenuCommand::Unavailable => {}
            _ => {
                if let Some(command) = command.editor_command() {
                    self.editor.update(cx, |view, _| {
                        view.apply_editor_command(command);
                    });
                }
            }
        }
        self.close_menu(window, cx);
    }

    fn on_key_down(&mut self, event: &KeyDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        let key = event.keystroke.key.as_str();
        // The editor shell and the focused menu bar both see the same bubbled
        // key event. Without this guard a single Down would move twice (New ->
        // Open) and a single Enter would run the wrong item.
        let now = std::time::Instant::now();
        if let Some((last, at)) = &self.last_key
            && last == key
            && now.duration_since(*at) < std::time::Duration::from_millis(8)
        {
            return;
        }
        self.last_key = Some((key.to_string(), now));
        if self.preferences.open {
            if key == "escape" {
                self.preferences.cancel();
                window.focus(&self.editor_focus, cx);
                cx.notify();
            }
            return;
        }
        let fence_open = self.editor.read(cx).fence_language().is_some();
        if fence_open && self.menu.open.is_none() {
            let handled = match key {
                "escape" => {
                    self.editor.update(cx, |view, _| view.fence_language_cancel());
                    true
                }
                "backspace" => {
                    self.editor.update(cx, |view, _| {
                        view.fence_language_backspace();
                    });
                    true
                }
                "up" => {
                    self.editor.update(cx, |view, _| view.fence_language_move(-1));
                    true
                }
                "down" => {
                    self.editor.update(cx, |view, _| view.fence_language_move(1));
                    true
                }
                "enter" | "tab" => {
                    self.editor.update(cx, |view, cx| {
                        view.fence_language_commit(cx);
                    });
                    true
                }
                _ => false,
            };
            if handled {
                cx.notify();
                return;
            }
        }
        if self.editor.read(cx).completion().is_some() {
            // Only the popup's own keys are consumed here. Everything else
            // (typing, Backspace, arrows) must keep reaching the editor: the
            // language list is filtered by continuing to type, and a swallowed
            // Backspace would make a typo impossible to correct without Esc.
            let handled = match key {
                "escape" => {
                    self.editor.update(cx, |view, _| view.dismiss_completion());
                    true
                }
                "up" => {
                    self.editor.update(cx, |view, _| view.move_completion(-1));
                    true
                }
                "down" => {
                    self.editor.update(cx, |view, _| view.move_completion(1));
                    true
                }
                "enter" | "tab" => {
                    self.editor.update(cx, |view, _| {
                        view.accept_completion();
                    });
                    true
                }
                _ => false,
            };
            if handled {
                cx.notify();
                return;
            }
        }
        if event.keystroke.modifiers.alt {
            let index = match key {
                "f" => Some(0),
                "e" => Some(1),
                "p" => Some(2),
                "o" => Some(3),
                "v" => Some(4),
                "t" => Some(5),
                "h" => Some(6),
                _ => None,
            };
            if let Some(index) = index {
                self.open_menu(index, window, cx);
                return;
            }
        }
        if !self.menu.is_open() {
            return;
        }
        match key {
            "escape" => self.close_menu(window, cx),
            "left" => {
                self.menu.move_menu(-1);
                cx.notify();
            }
            "right" => {
                self.menu.move_menu(1);
                cx.notify();
            }
            "up" | "down" => {
                let delta = if key == "down" { 1 } else { -1 };
                let editor = self.editor.read(cx);
                self.menu.move_highlight(delta, &editor);
                cx.notify();
            }
            "enter" | "space" => {
                let command = {
                    let editor = self.editor.read(cx);
                    self.menu.active_command(&editor)
                };
                if let Some(command) = command {
                    self.run(command, window, cx);
                }
            }
            _ => {}
        }
    }

    fn pref_button(
        &self,
        label: impl Into<SharedString>,
        active: bool,
        cx: &mut Context<Self>,
        action: impl Fn(&mut Self) + 'static,
    ) -> AnyElement {
        let label: SharedString = label.into();
        let theme = self.theme.clone();
        div()
            .id(ElementId::Name(format!("pref-btn-{label}").into()))
            .px_3()
            .py_1()
            .rounded(px(3.))
            .border_1()
            .border_color(theme.rule)
            .text_size(px(13.))
            .when(active, |this| this.bg(theme.selection))
            .hover(|this| this.bg(theme.selection))
            .cursor_pointer()
            .on_click(cx.listener(move |this, _: &ClickEvent, _window, cx| {
                action(this);
                cx.notify();
            }))
            .child(label)
            .into_any_element()
    }

    fn pref_row(&self, label: &'static str, control: AnyElement) -> AnyElement {
        let theme = self.theme.clone();
        div()
            .flex()
            .items_center()
            .justify_between()
            .py_2()
            .border_b_1()
            .border_color(theme.rule)
            .child(div().text_size(px(13.)).text_color(theme.text).child(label))
            .child(control)
            .into_any_element()
    }

    fn render_preferences(&self, cx: &mut Context<Self>) -> AnyElement {
        let theme = self.theme.clone();
        let draft = &self.preferences.draft;
        let mut rail = div()
            .w(px(PREFERENCES_RAIL as f32))
            .h_full()
            .flex()
            .flex_col()
            .bg(rgb(0xFAFAFA))
            .border_r_1()
            .border_color(theme.rule)
            .py_3();
        for (index, label) in ["外观", "文件与保存", "编辑", "快捷键与语言"]
            .into_iter()
            .enumerate()
        {
            let active = self.preferences.category == index;
            rail = rail.child(
                div()
                    .id(ElementId::Name(format!("pref-cat-{index}").into()))
                    .h(px(32.))
                    .px_4()
                    .flex()
                    .items_center()
                    .text_size(px(13.))
                    .when(active, |this| this.bg(rgb(0xEEEEEE)))
                    .hover(|this| this.bg(rgb(0xEEEEEE)))
                    .cursor_pointer()
                    .on_click(cx.listener(move |this, _: &ClickEvent, _window, cx| {
                        this.preferences.category = index;
                        cx.notify();
                    }))
                    .child(label),
            );
        }

        let mut pane = div()
            .id("typors-preferences-pane")
            .flex_1()
            .h_full()
            .overflow_y_scroll()
            .bg(rgb(0xFFFFFF))
            .px(px(30.))
            .py_4();
        match self.preferences.category {
            0 => {
                pane = pane
                    .child(
                        div()
                            .text_size(px(18.))
                            .font_weight(FontWeight::SEMIBOLD)
                            .child("外观"),
                    )
                    .child(
                        self.pref_row(
                            "主题",
                            div()
                                .flex()
                                .gap_2()
                                .child(self.pref_button(
                                    "浅色",
                                    draft.appearance.theme == typors_settings::ThemeChoice::Light,
                                    cx,
                                    |this| {
                                        this.preferences.draft.appearance.theme =
                                            typors_settings::ThemeChoice::Light;
                                    },
                                ))
                                .child(self.pref_button(
                                    "深色",
                                    draft.appearance.theme == typors_settings::ThemeChoice::Dark,
                                    cx,
                                    |this| {
                                        this.preferences.draft.appearance.theme =
                                            typors_settings::ThemeChoice::Dark;
                                    },
                                ))
                                .child(self.pref_button(
                                    "跟随系统",
                                    draft.appearance.theme == typors_settings::ThemeChoice::System,
                                    cx,
                                    |this| {
                                        this.preferences.draft.appearance.theme =
                                            typors_settings::ThemeChoice::System;
                                    },
                                ))
                                .into_any_element(),
                        ),
                    )
                    .child(
                        self.pref_row(
                            "字号",
                            div()
                                .flex()
                                .items_center()
                                .gap_2()
                                .child(self.pref_button("−", false, cx, |this| {
                                    this.preferences.step_size(-1.0)
                                }))
                                .child(format!("{:.0}", draft.appearance.font_size))
                                .child(self.pref_button("+", false, cx, |this| {
                                    this.preferences.step_size(1.0)
                                }))
                                .into_any_element(),
                        ),
                    )
                    .child(
                        self.pref_row(
                            "行高",
                            div()
                                .flex()
                                .items_center()
                                .gap_2()
                                .child(self.pref_button("−", false, cx, |this| {
                                    this.preferences.step_line(-0.1)
                                }))
                                .child(format!("{:.1}", draft.appearance.line_height))
                                .child(self.pref_button("+", false, cx, |this| {
                                    this.preferences.step_line(0.1)
                                }))
                                .into_any_element(),
                        ),
                    )
                    .child(
                        self.pref_row(
                            "页宽",
                            div()
                                .flex()
                                .items_center()
                                .gap_2()
                                .child(self.pref_button("−", false, cx, |this| {
                                    this.preferences.step_width(-20.0)
                                }))
                                .child(format!("{:.0} px", draft.appearance.page_width))
                                .child(self.pref_button("+", false, cx, |this| {
                                    this.preferences.step_width(20.0)
                                }))
                                .into_any_element(),
                        ),
                    );
            }
            1 => {
                pane = pane
                    .child(
                        div()
                            .text_size(px(18.))
                            .font_weight(FontWeight::SEMIBOLD)
                            .child("文件与保存"),
                    )
                    .child(
                        self.pref_row(
                            "自动保存",
                            div()
                                .flex()
                                .gap_2()
                                .child(self.pref_button(
                                    "关闭",
                                    draft.files.autosave == typors_settings::Autosave::Off,
                                    cx,
                                    |this| {
                                        this.preferences.draft.files.autosave =
                                            typors_settings::Autosave::Off;
                                    },
                                ))
                                .child(self.pref_button(
                                    "30 秒",
                                    draft.files.autosave == typors_settings::Autosave::Seconds30,
                                    cx,
                                    |this| {
                                        this.preferences.draft.files.autosave =
                                            typors_settings::Autosave::Seconds30;
                                    },
                                ))
                                .child(self.pref_button(
                                    "60 秒",
                                    draft.files.autosave == typors_settings::Autosave::Seconds60,
                                    cx,
                                    |this| {
                                        this.preferences.draft.files.autosave =
                                            typors_settings::Autosave::Seconds60;
                                    },
                                ))
                                .into_any_element(),
                        ),
                    )
                    .child(self.pref_row(
                        "关闭时提示保存",
                        self.pref_button(
                            if draft.files.prompt_on_close {
                                "开"
                            } else {
                                "关"
                            },
                            draft.files.prompt_on_close,
                            cx,
                            |this| {
                                this.preferences.draft.files.prompt_on_close =
                                    !this.preferences.draft.files.prompt_on_close;
                            },
                        ),
                    ))
                    .child(self.pref_row(
                        "恢复上次会话",
                        self.pref_button(
                            if draft.files.restore_last_session {
                                "开"
                            } else {
                                "关"
                            },
                            draft.files.restore_last_session,
                            cx,
                            |this| {
                                this.preferences.draft.files.restore_last_session =
                                    !this.preferences.draft.files.restore_last_session;
                            },
                        ),
                    ));
            }
            2 => {
                pane = pane
                    .child(
                        div()
                            .text_size(px(18.))
                            .font_weight(FontWeight::SEMIBOLD)
                            .child("编辑"),
                    )
                    .child(self.pref_row(
                        "Markdown 提示",
                        self.pref_button(
                            if draft.editing.completion_enabled {
                                "开"
                            } else {
                                "关"
                            },
                            draft.editing.completion_enabled,
                            cx,
                            |this| {
                                this.preferences.draft.editing.completion_enabled =
                                    !this.preferences.draft.editing.completion_enabled;
                            },
                        ),
                    ))
                    .child(
                        self.pref_row(
                            "提示延迟",
                            div()
                                .flex()
                                .items_center()
                                .gap_2()
                                .child(self.pref_button("−", false, cx, |this| {
                                    this.preferences.step_delay(-10)
                                }))
                                .child(format!("{} ms", draft.editing.completion_delay_ms))
                                .child(self.pref_button("+", false, cx, |this| {
                                    this.preferences.step_delay(10)
                                }))
                                .into_any_element(),
                        ),
                    )
                    .child(self.pref_row(
                        "光标闪烁",
                        self.pref_button(
                            if draft.editing.blink_enabled {
                                "开"
                            } else {
                                "关"
                            },
                            draft.editing.blink_enabled,
                            cx,
                            |this| {
                                this.preferences.draft.editing.blink_enabled =
                                    !this.preferences.draft.editing.blink_enabled;
                            },
                        ),
                    ))
                    .child(
                        self.pref_row(
                            "闪烁间隔",
                            div()
                                .flex()
                                .items_center()
                                .gap_2()
                                .child(self.pref_button("−", false, cx, |this| {
                                    this.preferences.step_blink(-30)
                                }))
                                .child(format!("{} ms", draft.editing.blink_interval_ms))
                                .child(self.pref_button("+", false, cx, |this| {
                                    this.preferences.step_blink(30)
                                }))
                                .into_any_element(),
                        ),
                    );
            }
            _ => {
                let mut shortcuts = div().flex().flex_col().text_size(px(13.));
                for (command, chord) in draft.shortcuts.iter() {
                    shortcuts = shortcuts.child(
                        div()
                            .flex()
                            .justify_between()
                            .py_1()
                            .child(command.clone())
                            .child(chord.clone()),
                    );
                }
                pane = pane
                    .child(
                        div()
                            .text_size(px(18.))
                            .font_weight(FontWeight::SEMIBOLD)
                            .child("快捷键与语言"),
                    )
                    .child(
                        div()
                            .mt_2()
                            .text_size(px(12.))
                            .text_color(theme.muted)
                            .child("菜单快捷键（点击菜单项可重新绑定；当前版本提供参考表）"),
                    )
                    .child(shortcuts)
                    .child(
                        self.pref_row(
                            "语言",
                            div()
                                .flex()
                                .gap_2()
                                .child(self.pref_button(
                                    "简体中文",
                                    draft.language == typors_settings::Language::Zh,
                                    cx,
                                    |this| {
                                        this.preferences.draft.language =
                                            typors_settings::Language::Zh;
                                    },
                                ))
                                .child(self.pref_button(
                                    "English",
                                    draft.language == typors_settings::Language::En,
                                    cx,
                                    |this| {
                                        this.preferences.draft.language =
                                            typors_settings::Language::En;
                                    },
                                ))
                                .child(self.pref_button(
                                    "跟随系统",
                                    draft.language == typors_settings::Language::System,
                                    cx,
                                    |this| {
                                        this.preferences.draft.language =
                                            typors_settings::Language::System;
                                    },
                                ))
                                .into_any_element(),
                        ),
                    );
            }
        }

        let mut footer = div()
            .h(px(48.))
            .flex()
            .items_center()
            .justify_between()
            .px_4()
            .border_t_1()
            .border_color(theme.rule)
            .child(self.pref_button("恢复默认值", false, cx, |this| {
                this.preferences.draft = typors_settings::Settings::default();
            }));
        let mut actions =
            div()
                .flex()
                .gap_2()
                .child(self.pref_button("取消", false, cx, |this| {
                    this.preferences.cancel();
                }));
        actions = actions.child(
            div()
                .id("prefs-ok")
                .px_3()
                .py_1()
                .rounded(px(3.))
                .bg(theme.selection)
                .border_1()
                .border_color(theme.rule)
                .text_size(px(13.))
                .cursor_pointer()
                .on_click(cx.listener(|this, _: &ClickEvent, window, cx| {
                    this.apply_preferences(window, cx);
                }))
                .child("确定"),
        );
        footer = footer.child(actions);

        let mut overlay = div()
            .id("typors-preferences-backdrop")
            .absolute()
            .inset_0()
            .bg(rgba(0x0000001A))
            .on_click(cx.listener(|this, _: &ClickEvent, _window, cx| {
                this.preferences.cancel();
                cx.notify();
            }))
            .child(
                div()
                    .id("typors-preferences-guard")
                    .absolute()
                    .inset_0()
                    .on_click(cx.listener(|_, _: &ClickEvent, _, _| {})),
            );
        overlay
            .child(
                div()
                    .absolute()
                    .top(px(60.))
                    .left_0()
                    .right_0()
                    .mx_auto()
                    .w(px(900.))
                    .h(px(620.))
                    .flex()
                    .flex_col()
                    .bg(rgb(0xFFFFFF))
                    .border_1()
                    .border_color(theme.rule)
                    .shadow_lg()
                    .rounded(px(2.))
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .justify_between()
                            .h(px(48.))
                            .px_4()
                            .border_b_1()
                            .border_color(theme.rule)
                            .child(
                                div()
                                    .text_size(px(15.))
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .child("偏好设置"),
                            )
                            .child(self.pref_button("关闭", false, cx, |this| {
                                this.preferences.cancel()
                            })),
                    )
                    .child(div().flex_1().min_h_0().flex().child(rail).child(pane))
                    .child(footer),
            )
            .into_any_element()
    }

    fn menu_item(&self, menu: usize, index: usize, cx: &mut Context<Self>) -> AnyElement {
        let item = &self.menu.menus[menu].items[index];
        let enabled = self.menu.enabled(menu, index, &self.editor.read(cx));
        let highlighted = self.menu.highlight == Some(index);
        let theme = self.theme.clone();
        div()
            .id(ElementId::Name(format!("menu-{menu}-{index}").into()))
            .flex()
            .items_center()
            .justify_between()
            .w_full()
            .px_3()
            .py_1()
            .text_size(px(13.))
            .text_color(if enabled { theme.text } else { theme.muted })
            .when(highlighted && enabled, |this| this.bg(theme.selection))
            .when(enabled, |this| {
                this.cursor_pointer().on_click(cx.listener(
                    move |this, _: &ClickEvent, window, cx| {
                        let command = this.menu.menus[menu].items[index].command;
                        this.run(command, window, cx);
                    },
                ))
            })
            .child(SharedString::from(item.label))
            .child(
                div()
                    .text_size(px(11.))
                    .text_color(theme.muted)
                    .child(SharedString::from(item.shortcut)),
            )
            .into_any_element()
    }

    fn render_dropdown(&self, menu: usize, cx: &mut Context<Self>) -> AnyElement {
        let spec = &self.menu.menus[menu];
        let theme = self.theme.clone();
        let mut column = div()
            .absolute()
            .top(px(28.))
            .left(px([8., 84., 148., 210., 274., 334., 388.][menu.min(6)]))
            .w(px(240.))
            .flex()
            .flex_col()
            .bg(theme.page_bg)
            .border_1()
            .border_color(theme.rule)
            .shadow_md()
            .py_1()
            .text_size(px(13.));
        for (index, item) in spec.items.iter().enumerate() {
            if item.separator_before {
                column = column.child(div().h(px(1.)).my_1().bg(theme.rule).into_any_element());
            }
            column = column.child(self.menu_item(menu, index, cx));
        }
        column.into_any_element()
    }
}

impl Render for EditorShell {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        // Tell the editor the band it actually occupies: the menu row and the
        // status bar take 28 px each, and caret scrolling must respect them.
        let window_height = window.viewport_size().height.as_f32();
        let editor_viewport = typors_editor::view::EditorViewport {
            top: 28.0,
            height: (window_height - 28.0 - 28.0).max(1.0),
        };
        let _ = self
            .editor
            .update(cx, |view, _| view.set_editor_viewport(Some(editor_viewport)));
        if let Some(interval) = self.settings.files.interval()
            && self.last_autosave.elapsed() >= interval
        {
            self.last_autosave = std::time::Instant::now();
            if self.dirty(cx)
                && let Some(path) = self.path.clone()
            {
                let text = self.current_text(cx);
                if std::fs::write(&path, text).is_ok() {
                    self.saved_revision = self.editor.read(cx).doc().revision();
                }
            }
        }
        let title = self.display_title(cx);
        if title != self.last_title {
            window.set_window_title(&title);
            self.last_title = title;
        }
        let theme = self.theme.clone();
        let mut bar = div()
            .id("typors-menu-bar")
            .flex()
            .items_center()
            .h(px(28.))
            .w_full()
            .px_2()
            .bg(theme.page_bg)
            .border_b_1()
            .border_color(theme.rule)
            .text_size(px(13.))
            .text_color(theme.text)
            .track_focus(&self.menu_focus)
            .on_key_down(cx.listener(Self::on_key_down));

        for (index, spec) in self.menu.menus.iter().enumerate() {
            let open = self.menu.open == Some(index);
            bar = bar.child(
                div()
                    .id(ElementId::Name(format!("menu-label-{index}").into()))
                    .flex()
                    .items_center()
                    .h_full()
                    .px_2()
                    .rounded_sm()
                    .when(open, |this| this.bg(theme.selection))
                    .hover(|this| this.bg(theme.selection))
                    .cursor_pointer()
                    .on_click(cx.listener(move |this, _: &ClickEvent, window, cx| {
                        if this.menu.open == Some(index) {
                            this.close_menu(window, cx);
                        } else {
                            this.open_menu(index, window, cx);
                        }
                    }))
                    .child(SharedString::from(spec.label)),
            );
        }

        let revision = self.editor.read(cx).doc().revision();
        if self.word_count_cache.0 != revision {
            let words = self.editor.read(cx).doc().text().split_whitespace().count();
            self.word_count_cache = (revision, words);
        }
        let word_count = self.word_count_cache.1;

        let mut root = div()
            .relative()
            .size_full()
            .flex()
            .flex_col()
            .bg(theme.page_bg)
            .on_key_down(cx.listener(Self::on_key_down))
            .child(bar)
            .child(
                div()
                    .flex_1()
                    .min_h_0()
                    .child(self.editor.clone())
                    .into_any_element(),
            )
            .child(
                div()
                    .h(px(28.))
                    .w_full()
                    .flex()
                    .items_center()
                    .justify_between()
                    .px_3()
                    .border_t_1()
                    .border_color(theme.rule)
                    .text_size(px(12.))
                    .text_color(theme.muted)
                    .child("◉  ‹›")
                    .child(format!("{word_count} 词")),
            );

        if let Some(open) = self.menu.open {
            root = root
                .child(
                    div()
                        .id("typors-menu-backdrop")
                        .absolute()
                        .top(px(28.))
                        .left_0()
                        .right_0()
                        .bottom_0()
                        .on_click(cx.listener(|this, _: &ClickEvent, window, cx| {
                            this.close_menu(window, cx);
                        })),
                )
                .child(self.render_dropdown(open, cx));
        }
        if self.preferences.open {
            root = root.child(self.render_preferences(cx));
        }
        let completion = self
            .editor
            .read(cx)
            .completion()
            .cloned()
            .zip(self.editor.read(cx).completion_anchor());
        if !self.preferences.open
            && self.menu.open.is_none()
            && let Some((state, (anchor, height))) = completion
        {
            let theme = theme.clone();
            let mut popup = div()
                .absolute()
                .left(px(anchor.x.as_f32()))
                .top(px(anchor.y.as_f32() + height.as_f32()))
                .w(px(280.))
                .flex()
                .flex_col()
                .bg(theme.page_bg)
                .border_1()
                .border_color(theme.rule)
                .shadow_md()
                .py_1();
            // The language list is long; show a window around the selection,
            // with ellipsis rows when it is truncated.
            const MAX_VISIBLE: usize = 8;
            let start = state
                .selected
                .saturating_sub(3)
                .min(state.items.len().saturating_sub(MAX_VISIBLE));
            let end = (start + MAX_VISIBLE).min(state.items.len());
            if start > 0 {
                popup = popup.child(
                    div()
                        .px_3()
                        .py_1()
                        .text_size(px(11.))
                        .text_color(theme.muted)
                        .child("…"),
                );
            }
            for (index, item) in state.items[start..end]
                .iter()
                .enumerate()
                .map(|(offset, item)| (start + offset, item))
            {
                popup = popup.child(
                    div()
                        .flex()
                        .justify_between()
                        .px_3()
                        .py_1()
                        .text_size(px(13.))
                        .when(index == state.selected, |this| this.bg(theme.selection))
                        .child(item.label.clone())
                        .child(
                            div()
                                .text_size(px(11.))
                                .text_color(theme.muted)
                                .child(item.detail.clone()),
                        ),
                );
            }
            if end < state.items.len() {
                popup = popup.child(
                    div()
                        .px_3()
                        .py_1()
                        .text_size(px(11.))
                        .text_color(theme.muted)
                        .child("…"),
                );
            }
            root = root.child(popup);
        }

        // The fence-language chip and picker for the caret's code block.
        if !self.preferences.open
            && self.menu.open.is_none()
            && let Some((block_idx, bounds, language, editing)) =
                self.editor.read(cx).fence_pill()
        {
            let right = bounds.origin.x.as_f32() + bounds.size.width.as_f32();
            let top = bounds.origin.y.as_f32() + 8.0;
            root = root.child(
                div()
                    .id("typors-fence-language")
                    .absolute()
                    .left(px(right - 96.))
                    .top(px(top))
                    .w(px(88.))
                    .flex()
                    .justify_end()
                    .child(
                        div()
                            .id("typors-fence-language-chip")
                            .px_2()
                            .py(px(2.))
                            .rounded(px(4.))
                            .bg(theme.code_bg)
                            .border_1()
                            .border_color(theme.rule)
                            .text_size(px(11.))
                            .text_color(theme.muted)
                            .cursor_pointer()
                            .on_click(cx.listener(
                                move |this, _: &ClickEvent, _window, cx| {
                                    if std::env::var("TYPORS_PILL_DEBUG").is_ok() {
                                        eprintln!("pill: chip clicked for block {block_idx}");
                                    }
                                    this.editor.update(cx, |view, cx| {
                                        view.begin_fence_language_edit(block_idx, cx)
                                    });
                                    cx.notify();
                                },
                            ))
                            .child(SharedString::from(language)),
                    ),
            );
            if editing {
                let items = self.editor.read(cx).fence_language_items();
                let selected = self
                    .editor
                    .read(cx)
                    .fence_language_selected()
                    .unwrap_or(0);
                let filter = self
                    .editor
                    .read(cx)
                    .fence_language()
                    .map(|edit| edit.filter.clone())
                    .unwrap_or_default();
                let mut picker = div()
                    .id("typors-fence-language-picker")
                    .absolute()
                    .left(px(right - 246.))
                    .top(px(top + 24.))
                    .w(px(240.))
                    .flex()
                    .flex_col()
                    .bg(theme.page_bg)
                    .border_1()
                    .border_color(theme.rule)
                    .shadow_md()
                    .py_1();
                picker = picker.child(
                    div()
                        .px_3()
                        .py_1()
                        .text_size(px(13.))
                        .child(SharedString::from(if filter.is_empty() {
                            "语言".to_string()
                        } else {
                            filter.clone()
                        })),
                );
                const MAX_PICKER: usize = 8;
                let start = selected
                    .saturating_sub(3)
                    .min(items.len().saturating_sub(MAX_PICKER));
                let end = (start + MAX_PICKER).min(items.len());
                if start > 0 {
                    picker = picker.child(
                        div()
                            .px_3()
                            .py_1()
                            .text_size(px(11.))
                            .text_color(theme.muted)
                            .child("…"),
                    );
                }
                for (index, item) in items[start..end]
                    .iter()
                    .enumerate()
                    .map(|(offset, item)| (start + offset, item))
                {
                    let item = item.clone();
                    picker = picker.child(
                        div()
                            .id(ElementId::Name(
                                format!("typors-fence-language-{index}").into(),
                            ))
                            .flex()
                            .justify_between()
                            .px_3()
                            .py_1()
                            .text_size(px(13.))
                            .when(index == selected, |this| this.bg(theme.selection))
                            .cursor_pointer()
                            .on_click(cx.listener(
                                move |this, _: &ClickEvent, _window, cx| {
                                    this.editor.update(cx, |view, cx| {
                                        view.fence_language_choose(index, cx)
                                    });
                                    cx.notify();
                                },
                            ))
                            .child(item.label.clone())
                            .child(
                                div()
                                    .text_size(px(11.))
                                    .text_color(theme.muted)
                                    .child(item.detail.clone()),
                            ),
                    );
                }
                if end < items.len() {
                    picker = picker.child(
                        div()
                            .px_3()
                            .py_1()
                            .text_size(px(11.))
                            .text_color(theme.muted)
                            .child("…"),
                    );
                }
                root = root.child(picker);
            }
        }
        root
    }
}

fn message_box(text: &str, title: &str, flags: u32) -> i32 {
    let text: Vec<u16> = text.encode_utf16().chain(std::iter::once(0)).collect();
    let title: Vec<u16> = title.encode_utf16().chain(std::iter::once(0)).collect();
    unsafe { MessageBoxW(std::ptr::null_mut(), text.as_ptr(), title.as_ptr(), flags) }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum DiscardAnswer {
    Save,
    Discard,
    Cancel,
}

pub(crate) fn discard_answer(button: i32) -> DiscardAnswer {
    match button {
        IDYES => DiscardAnswer::Save,
        IDNO => DiscardAnswer::Discard,
        _ => DiscardAnswer::Cancel,
    }
}
