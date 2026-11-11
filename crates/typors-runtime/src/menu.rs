//! Menu specification and state for the editor shell.
//!
//! The menu is data, not paint code: labels, shortcuts, enabled predicates and
//! the currently open/highlighted row live here so they can be tested without a
//! window.

use typors_editor::view::{EditorCommand, MarkdownView};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MenuCommand {
    New,
    Open,
    Save,
    SaveAs,
    Preferences,
    Exit,
    Undo,
    Redo,
    Cut,
    Copy,
    Paste,
    SelectAll,
    About,
    Unavailable,
    Heading(u8),
    Paragraph,
    BulletList,
    OrderedList,
    Quote,
    CodeBlock,
    Bold,
    Italic,
    Strike,
    InlineCode,
    InlineMath,
    Link,
    Image,
}

impl MenuCommand {
    pub fn editor_command(self) -> Option<EditorCommand> {
        match self {
            Self::Undo
            | Self::Redo
            | Self::Cut
            | Self::Copy
            | Self::Paste
            | Self::SelectAll
            | Self::New
            | Self::Open
            | Self::Save
            | Self::SaveAs
            | Self::Preferences
            | Self::Exit
            | Self::About
            | Self::Unavailable => None,
            Self::Heading(level) => Some(EditorCommand::Heading(level)),
            Self::Paragraph => Some(EditorCommand::Paragraph),
            Self::BulletList => Some(EditorCommand::BulletList),
            Self::OrderedList => Some(EditorCommand::OrderedList),
            Self::Quote => Some(EditorCommand::Quote),
            Self::CodeBlock => Some(EditorCommand::CodeBlock),
            Self::Bold => Some(EditorCommand::Bold),
            Self::Italic => Some(EditorCommand::Italic),
            Self::Strike => Some(EditorCommand::Strike),
            Self::InlineCode => Some(EditorCommand::InlineCode),
            Self::InlineMath => Some(EditorCommand::InlineMath),
            Self::Link => Some(EditorCommand::Link),
            Self::Image => Some(EditorCommand::Image),
        }
    }
}

#[derive(Clone, Debug)]
pub struct MenuItem {
    pub label: &'static str,
    pub shortcut: &'static str,
    pub command: MenuCommand,
    pub separator_before: bool,
}

#[derive(Clone, Debug)]
pub struct MenuSpec {
    pub label: &'static str,
    pub mnemonic: char,
    pub items: Vec<MenuItem>,
}

fn item(label: &'static str, shortcut: &'static str, command: MenuCommand) -> MenuItem {
    MenuItem {
        label,
        shortcut,
        command,
        separator_before: false,
    }
}

pub fn menus() -> Vec<MenuSpec> {
    vec![
        MenuSpec {
            label: "文件(F)",
            mnemonic: 'f',
            items: vec![
                item("新建", "Ctrl+N", MenuCommand::New),
                item("打开…", "Ctrl+O", MenuCommand::Open),
                item("保存", "Ctrl+S", MenuCommand::Save),
                item("另存为…", "Ctrl+Shift+S", MenuCommand::SaveAs),
                item("偏好设置…", "Ctrl+,", MenuCommand::Preferences),
                MenuItem {
                    label: "退出",
                    shortcut: "",
                    command: MenuCommand::Exit,
                    separator_before: true,
                },
            ],
        },
        MenuSpec {
            label: "编辑(E)",
            mnemonic: 'e',
            items: vec![
                item("撤销", "Ctrl+Z", MenuCommand::Undo),
                item("重做", "Ctrl+Y", MenuCommand::Redo),
                MenuItem {
                    label: "剪切",
                    shortcut: "Ctrl+X",
                    command: MenuCommand::Cut,
                    separator_before: true,
                },
                item("复制", "Ctrl+C", MenuCommand::Copy),
                item("粘贴", "Ctrl+V", MenuCommand::Paste),
                item("全选", "Ctrl+A", MenuCommand::SelectAll),
            ],
        },
        MenuSpec {
            label: "段落(P)",
            mnemonic: 'p',
            items: vec![
                item("标题 1", "Ctrl+1", MenuCommand::Heading(1)),
                item("标题 2", "Ctrl+2", MenuCommand::Heading(2)),
                item("标题 3", "Ctrl+3", MenuCommand::Heading(3)),
                item("标题 4", "Ctrl+4", MenuCommand::Heading(4)),
                item("标题 5", "Ctrl+5", MenuCommand::Heading(5)),
                item("标题 6", "Ctrl+6", MenuCommand::Heading(6)),
                item("正文", "Ctrl+0", MenuCommand::Paragraph),
                MenuItem {
                    label: "无序列表",
                    shortcut: "Ctrl+Shift+8",
                    command: MenuCommand::BulletList,
                    separator_before: true,
                },
                item("有序列表", "Ctrl+Shift+7", MenuCommand::OrderedList),
                item("引用", "Ctrl+Shift+Q", MenuCommand::Quote),
                item("代码块", "Ctrl+Shift+K", MenuCommand::CodeBlock),
            ],
        },
        MenuSpec {
            label: "格式(O)",
            mnemonic: 'o',
            items: vec![
                item("粗体", "Ctrl+B", MenuCommand::Bold),
                item("斜体", "Ctrl+I", MenuCommand::Italic),
                item("删除线", "Alt+Shift+5", MenuCommand::Strike),
                item("行内代码", "Ctrl+`", MenuCommand::InlineCode),
                MenuItem {
                    label: "行内公式",
                    shortcut: "",
                    command: MenuCommand::InlineMath,
                    separator_before: true,
                },
                item("链接", "Ctrl+K", MenuCommand::Link),
                item("图片", "", MenuCommand::Image),
            ],
        },
        MenuSpec {
            label: "视图(V)",
            mnemonic: 'v',
            items: vec![
                item("放大（暂未提供）", "", MenuCommand::Unavailable),
                item("缩小（暂未提供）", "", MenuCommand::Unavailable),
                item("重置缩放（暂未提供）", "", MenuCommand::Unavailable),
            ],
        },
        MenuSpec {
            label: "主题(T)",
            mnemonic: 't',
            items: vec![
                item("浅色（暂未提供）", "", MenuCommand::Unavailable),
                item("深色（暂未提供）", "", MenuCommand::Unavailable),
                item("跟随系统（暂未提供）", "", MenuCommand::Unavailable),
            ],
        },
        MenuSpec {
            label: "帮助(H)",
            mnemonic: 'h',
            items: vec![item("关于 typors", "", MenuCommand::About)],
        },
    ]
}

/// The menu's interaction state. `open` is the top-level index, `highlight` the
/// item index inside it.
#[derive(Clone, Debug, Default)]
pub struct MenuState {
    pub open: Option<usize>,
    pub highlight: Option<usize>,
    pub menus: Vec<MenuSpec>,
    pub error: Option<String>,
}

impl MenuState {
    pub fn new() -> Self {
        Self {
            open: None,
            highlight: None,
            menus: menus(),
            error: None,
        }
    }

    pub fn is_open(&self) -> bool {
        self.open.is_some()
    }

    pub fn open_menu(&mut self, index: usize) {
        if index >= self.menus.len() {
            return;
        }
        self.open = Some(index);
        self.highlight = None;
    }

    pub fn close(&mut self) {
        self.open = None;
        self.highlight = None;
    }

    pub fn move_menu(&mut self, delta: isize) {
        let Some(open) = self.open else {
            return;
        };
        let len = self.menus.len();
        let next = (open as isize + delta).rem_euclid(len as isize) as usize;
        self.open_menu(next);
    }

    pub fn move_highlight(&mut self, delta: isize, view: &MarkdownView) {
        let Some(open) = self.open else {
            return;
        };
        let items = &self.menus[open].items;
        if items.is_empty() {
            return;
        }
        let mut index = self
            .highlight
            .unwrap_or(if delta > 0 { usize::MAX } else { 0 });
        for _ in 0..items.len() {
            index = if delta > 0 {
                if index == usize::MAX {
                    0
                } else {
                    (index + 1) % items.len()
                }
            } else if index == 0 {
                items.len() - 1
            } else {
                index - 1
            };
            if self.enabled(open, index, view) {
                self.highlight = Some(index);
                return;
            }
        }
        self.highlight = None;
    }

    pub fn enabled(&self, menu: usize, index: usize, view: &MarkdownView) -> bool {
        let Some(item) = self.menus.get(menu).and_then(|m| m.items.get(index)) else {
            return false;
        };
        match item.command {
            MenuCommand::Undo => view.can_undo(),
            MenuCommand::Redo => view.can_redo(),
            MenuCommand::Cut | MenuCommand::Copy => view.selection_range().is_some(),
            MenuCommand::Unavailable => false,
            MenuCommand::Save => true,
            _ => true,
        }
    }

    pub fn active_command(&self, view: &MarkdownView) -> Option<MenuCommand> {
        let open = self.open?;
        let index = self.highlight?;
        self.enabled(open, index, view)
            .then(|| self.menus[open].items[index].command)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn menu_labels_and_order_match_the_shell() {
        let menus = menus();
        let labels: Vec<_> = menus.iter().map(|m| m.label).collect();
        assert_eq!(
            labels,
            vec![
                "文件(F)",
                "编辑(E)",
                "段落(P)",
                "格式(O)",
                "视图(V)",
                "主题(T)",
                "帮助(H)"
            ]
        );
    }

    #[test]
    fn keyboard_navigation_wraps_and_close_resets() {
        let mut state = MenuState::new();
        state.open_menu(0);
        state.move_menu(-1);
        assert_eq!(state.open, Some(6));
        state.move_menu(1);
        assert_eq!(state.open, Some(0));
        state.close();
        assert!(!state.is_open());
    }
}

#[cfg(test)]
mod discard_tests {
    use super::super::shell::{DiscardAnswer, discard_answer};
    use windows_sys::Win32::UI::WindowsAndMessaging::{IDNO, IDYES};

    #[test]
    fn dirty_prompt_maps_yes_no_and_cancel() {
        assert_eq!(discard_answer(IDYES), DiscardAnswer::Save);
        assert_eq!(discard_answer(IDNO), DiscardAnswer::Discard);
        assert_eq!(discard_answer(0), DiscardAnswer::Cancel);
    }
}
