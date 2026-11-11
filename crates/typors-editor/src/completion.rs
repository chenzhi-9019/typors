//! The small Markdown snippet catalog behind the completion popup.

use typors_core::doc::Document;

#[derive(Clone, Debug)]
pub struct CompletionItem {
    pub label: String,
    pub detail: String,
    /// Full text replacing `start..caret` when accepted.
    pub insert: String,
    /// Caret offset inside `insert` (where the user continues typing).
    pub caret: usize,
}

#[derive(Clone, Debug)]
pub struct CompletionState {
    pub start: usize,
    pub end: usize,
    pub items: Vec<CompletionItem>,
    pub selected: usize,
}

impl CompletionState {
    pub fn current(&self) -> &CompletionItem {
        &self.items[self.selected]
    }

    pub fn move_selection(&mut self, delta: isize) {
        if self.items.is_empty() {
            return;
        }
        self.selected =
            (self.selected as isize + delta).rem_euclid(self.items.len() as isize) as usize;
    }
}

fn item(label: &str, detail: &str, insert: String, caret: usize) -> CompletionItem {
    CompletionItem {
        label: label.to_string(),
        detail: detail.to_string(),
        insert,
        caret,
    }
}

/// One suggestion, or `None` when the typed prefix is ordinary text.
pub fn suggest(doc: &Document, caret: usize) -> Option<CompletionState> {
    let text = doc.text();
    let caret = doc.snap_floor(caret);
    let _ = doc.block_at_src(caret)?;
    let line_start = text[..caret].rfind('\n').map_or(0, |i| i + 1);
    let before = &text[line_start..caret];
    let trimmed = before.trim_start_matches([' ', '\t']);
    let indent = before.len() - trimmed.len();

    let heading = trimmed.chars().take_while(|c| *c == '#').count();
    if heading > 0
        && heading <= 6
        && trimmed.len() == heading
        && caret == line_start + indent + heading
    {
        let prefix = format!("{} ", "#".repeat(heading));
        return Some(CompletionState {
            start: line_start + indent,
            end: caret,
            items: vec![item(&format!("标题 {heading}"), "Markdown 标题", prefix, 0)]
                .into_iter()
                .map(|mut i| {
                    i.caret = i.insert.len();
                    i
                })
                .collect(),
            selected: 0,
        });
    }

    let (start, items) = if trimmed == "-" || trimmed == "*" || trimmed == "+" {
        (
            caret - trimmed.len(),
            vec![item("无序列表", "Markdown 列表", "- ".to_string(), 2)],
        )
    } else if trimmed == "1." {
        (
            caret - 2,
            vec![item("有序列表", "Markdown 列表", "1. ".to_string(), 3)],
        )
    } else if trimmed == ">" {
        (
            caret - 1,
            vec![item("引用", "Markdown 引用", "> ".to_string(), 2)],
        )
    } else if trimmed == "```" {
        (
            caret - 3,
            vec![item("代码块", "围栏代码块", "```\n\n```".to_string(), 4)],
        )
    } else if before.ends_with("**") {
        (caret, vec![item("粗体", "闭合 **", "**".to_string(), 2)])
    } else if before.ends_with("~~") {
        (caret, vec![item("删除线", "闭合 ~~", "~~".to_string(), 2)])
    } else if before.ends_with('`') {
        (caret, vec![item("行内代码", "闭合 `", "`".to_string(), 1)])
    } else if before.ends_with("$$") {
        (
            caret,
            vec![item("块级公式", "闭合 $$", "$$".to_string(), 2)],
        )
    } else if before.ends_with('$') {
        (caret, vec![item("行内公式", "闭合 $", "$".to_string(), 1)])
    } else if before.ends_with("![") {
        (
            caret,
            vec![item(
                "图片",
                "Markdown 图片",
                "](assets/image.png)".to_string(),
                0,
            )],
        )
    } else if before.ends_with('[') {
        (
            caret,
            vec![item("链接", "Markdown 链接", "](https://)".to_string(), 0)],
        )
    } else if before.ends_with("---") {
        (
            caret,
            vec![item("分隔线", "水平分隔线", "\n".to_string(), 0)],
        )
    } else {
        return None;
    };

    Some(CompletionState {
        start,
        end: caret,
        items,
        selected: 0,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use typors_core::Document;

    #[test]
    fn heading_and_list_triggers() {
        let doc = Document::parse("##");
        let state = suggest(&doc, 2).expect("heading suggestion");
        assert!(state.items[0].label.contains("标题"));
        assert_eq!(state.items[0].insert, "## ");

        let doc = Document::parse("```");
        let state = suggest(&doc, 3).expect("fence suggestion");
        assert_eq!(state.items[0].insert, "```\n\n```");
    }

    #[test]
    fn inline_and_no_match() {
        let doc = Document::parse("**");
        let state = suggest(&doc, 2).expect("bold closing");
        assert_eq!(state.items[0].insert, "**");

        let doc = Document::parse("ordinary prose");
        assert!(suggest(&doc, doc.len_bytes()).is_none());
    }
}
