//! The small Markdown snippet catalog behind the completion popup.
//!
//! Two kinds of suggestions: the few Markdown constructs worth a snippet, and the
//! fence-language list. Headings (#) and inline code (`) deliberately do not
//! suggest anything: typing them is faster than picking a snippet, and the only
//! popup a backtick context needs is the fence language list.

use typors_core::doc::{BlockKind, Document};

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

/// A fence marker, written without a literal backtick in this file.
const FENCE: &str = "\u{0060}\u{0060}\u{0060}";

/// Common fence languages, in the order the unfiltered list shows them.
const LANGUAGES: &[(&str, &str)] = &[
    ("rust", "Rust"),
    ("python", "Python"),
    ("javascript", "JavaScript"),
    ("typescript", "TypeScript"),
    ("tsx", "React TSX"),
    ("jsx", "React JSX"),
    ("java", "Java"),
    ("c", "C"),
    ("cpp", "C++"),
    ("csharp", "C#"),
    ("go", "Go"),
    ("kotlin", "Kotlin"),
    ("swift", "Swift"),
    ("php", "PHP"),
    ("ruby", "Ruby"),
    ("lua", "Lua"),
    ("r", "R"),
    ("scala", "Scala"),
    ("dart", "Dart"),
    ("zig", "Zig"),
    ("bash", "Bash"),
    ("powershell", "PowerShell"),
    ("html", "HTML"),
    ("css", "CSS"),
    ("scss", "SCSS"),
    ("json", "JSON"),
    ("yaml", "YAML"),
    ("toml", "TOML"),
    ("xml", "XML"),
    ("sql", "SQL"),
    ("markdown", "Markdown"),
    ("mermaid", "Mermaid"),
    ("chart", "typors chart"),
    ("diff", "Diff"),
    ("ini", "INI"),
    ("http", "HTTP"),
    ("plaintext", "Plain text"),
];

/// A fence opener being typed, as `(start of the language text, partial)`.
///
/// Only the line start counts, and the info string must be a single word: a
/// closing fence or a language with attributes is not something to complete.
fn fence_language(before_caret: &str) -> Option<(usize, String)> {
    let indent = before_caret.len() - before_caret.trim_start_matches([' ', '\t']).len();
    let rest = &before_caret[indent..];
    let after = rest
        .strip_prefix(FENCE)
        .or_else(|| rest.strip_prefix("~~~"))?;
    let spaces = after.len() - after.trim_start_matches(' ').len();
    let partial = &after[spaces..];
    if partial.contains(['\u{0060}', '~', ' ', '\t']) {
        return None;
    }
    Some((indent + 3 + spaces, partial.to_string()))
}

/// The fence languages matching a filter, for the language editor at the
/// top-right of a code block.
pub fn language_items(prefix: &str) -> Vec<CompletionItem> {
    let lower = prefix.to_ascii_lowercase();
    LANGUAGES
        .iter()
        .filter(|(name, _)| name.starts_with(&lower))
        .map(|(name, detail)| item(name, detail, name.to_string(), name.len()))
        .collect()
}

/// The "no language" entry shown when the filter is empty.
pub fn plain_language_item() -> CompletionItem {
    item("(无语言)", "纯代码块", String::new(), 0)
}

/// Byte range of a fence's info string inside its block source.
///
/// Returns None when the first line is not a fence opener (three or more
/// backticks or tildes). The range excludes the marker and the spaces before the
/// info, and stops at the first newline, so replacing it rewrites only the
/// language: "python" -> "rust" keeps the fence and the block body intact.
pub fn fence_info_range(source: &str, src_start: usize) -> Option<std::ops::Range<usize>> {
    let first_line_len = source.find('\n').unwrap_or(source.len());
    let line = &source[..first_line_len];
    let marker_len = line
        .chars()
        .take_while(|ch| *ch == '\u{0060}' || *ch == '~')
        .count();
    if marker_len < 3 {
        return None;
    }
    let after = &line[marker_len..];
    let spaces = after.len() - after.trim_start_matches(' ').len();
    Some(src_start + marker_len + spaces..src_start + line.len())
}

/// One suggestion, or `None` when the typed prefix is ordinary text.
pub fn suggest(doc: &Document, caret: usize) -> Option<CompletionState> {
    let text = doc.text();
    let caret = doc.snap_floor(caret);
    let _ = doc.block_at_src(caret)?;
    let line_start = text[..caret].rfind('\n').map_or(0, |i| i + 1);
    let before = &text[line_start..caret];
    let trimmed = before.trim_start_matches([' ', '\t']);
    let line_end = text[caret..].find('\n').map_or(text.len(), |i| caret + i);

    // Fence language: a line that starts with a fence marker and is still being
    // typed. Accepting an item also terminates the line, so Enter opens the block.
    if line_end == caret
        && let Some((start, partial)) = fence_language(before)
    {
        let inside_fence_body = doc.block_at_src(caret).is_some_and(|idx| {
            doc.blocks().get(idx).is_some_and(|block| {
                matches!(block.kind, BlockKind::FencedCode { .. })
                    && block.src.start != line_start
            })
        });
        if inside_fence_body {
            return None;
        }
        let prefix = partial.to_ascii_lowercase();
        let mut items: Vec<CompletionItem> = Vec::new();
        if partial.is_empty() {
            items.push(item("(无语言)", "纯代码块", "\n".to_string(), 1));
        }
        for (name, detail) in LANGUAGES {
            if !name.starts_with(&prefix) {
                continue;
            }
            let mut insert = name.to_string();
            insert.push('\n');
            items.push(item(name, detail, insert, name.len() + 1));
        }
        let exact = !partial.is_empty()
            && items.len() == 1
            && items[0].label.eq_ignore_ascii_case(&partial);
        if exact || items.is_empty() {
            return None;
        }
        return Some(CompletionState {
            start: line_start + start,
            end: caret,
            items,
            selected: 0,
        });
    }

    // The remaining snippets. Headings and inline code are deliberate omissions.
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
    } else if before.ends_with("**") {
        (caret, vec![item("粗体", "闭合 **", "**".to_string(), 2)])
    } else if before.ends_with("~~") {
        (caret, vec![item("删除线", "闭合 ~~", "~~".to_string(), 2)])
    } else if before.ends_with("$$") {
        (caret, vec![item("块级公式", "闭合 $$", "$$".to_string(), 2)])
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

    const F: &str = "\u{0060}\u{0060}\u{0060}";

    #[test]
    fn a_fence_offers_languages_and_accepting_opens_the_block() {
        let text = F.to_string();
        let doc = Document::parse(&text);
        let state = suggest(&doc, 3).expect("language list");
        assert_eq!(state.items[0].label, "(无语言)");
        let rust = state
            .items
            .iter()
            .find(|item| item.label == "rust")
            .expect("rust");
        assert_eq!(rust.insert, "rust\n");
    }

    #[test]
    fn a_language_prefix_filters_the_list() {
        let text = format!("{F}py");
        let doc = Document::parse(&text);
        let state = suggest(&doc, 5).expect("python");
        assert_eq!(state.items[0].label, "python");
        assert_eq!(state.items[0].insert, "python\n");
        assert!(state.items.iter().all(|item| item.label.starts_with("py")));
    }

    #[test]
    fn an_exact_language_hides_the_popup() {
        let text = format!("{F}python");
        let doc = Document::parse(&text);
        assert!(suggest(&doc, 9).is_none());
        let text = format!("{F}zzz");
        let doc = Document::parse(&text);
        assert!(suggest(&doc, 6).is_none());
    }

    #[test]
    fn headings_and_inline_code_do_not_suggest() {
        for text in ["#", "##", "###", "text \u{0060}", "\u{0060}"] {
            let doc = Document::parse(text);
            assert!(
                suggest(&doc, doc.len_bytes()).is_none(),
                "{text:?} must not open the snippet popup"
            );
        }
    }

    #[test]
    fn lists_inline_snippets_and_no_match_still_work() {
        let doc = Document::parse("-");
        assert_eq!(suggest(&doc, 1).unwrap().items[0].insert, "- ");
        let doc = Document::parse("**");
        assert_eq!(suggest(&doc, 2).unwrap().items[0].insert, "**");
        let doc = Document::parse("ordinary prose");
        assert!(suggest(&doc, doc.len_bytes()).is_none());
    }


    #[test]
    fn the_selection_wraps_around_the_language_list() {
        let text = F.to_string();
        let doc = Document::parse(&text);
        let mut state = suggest(&doc, 3).expect("language list");
        assert_eq!(state.current().label, "(无语言)");
        state.move_selection(1);
        assert_eq!(state.current().label, "rust");
        state.move_selection(-1);
        assert_eq!(state.current().label, "(无语言)");
        state.move_selection(-1);
        assert_eq!(state.current().label, "plaintext");
    }

    #[test]
    fn rewriting_the_info_keeps_the_block() {
        let source = format!("{F}python\nprint(1)\n{F}\n");
        let range = fence_info_range(&source, 0).expect("info range");
        assert_eq!(range, 3..9);
        let mut doc = Document::parse(&source);
        doc.apply_edit(range, "rust");
        assert_eq!(doc.text(), format!("{F}rust\nprint(1)\n{F}\n"));

        // Leading spaces before the language stay outside the replaced range.
        let spaced = format!("{F} python\n");
        assert_eq!(fence_info_range(&spaced, 10).expect("range"), 14..20);
        // Not a fence.
        assert!(fence_info_range("text\n", 0).is_none());
        // An unfinished opener has an empty info range.
        assert_eq!(fence_info_range(F, 0).expect("range"), 3..3);
    }

    #[test]
    fn a_closing_fence_does_not_offer_languages() {
        let text = format!("{F}rust\n{F}");
        let doc = Document::parse(&text);
        assert!(suggest(&doc, doc.len_bytes()).is_none());
        let text = format!("text\n{F}py");
        let doc = Document::parse(&text);
        assert_eq!(
            suggest(&doc, doc.len_bytes()).unwrap().items[0].label,
            "python"
        );
    }
}
