//! The IME composition value shared by the render layout and the editor.
//!
//! The composition never reaches the rope; it is spliced into one block's source
//! projection for painting. Keeping the type in the document-model crate lets the
//! geometry crate consume it without depending on the editor.

use std::ops::Range;

/// One IME composition in progress: the string, where it would land, and where
/// the IME's own cursor sits inside it.
///
/// Windows reports the string as `GCS_COMPSTR` and the cursor as `GCS_CURSORPOS`;
/// both arrive in one `replace_and_mark_text_in_range` call, which is why
/// `selection` is stored with the text rather than tracked separately.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MarkedText {
    /// The composition string, as the IME last reported it.
    pub text: String,
    /// Source byte offset the composition would be inserted at on commit. The
    /// rope is untouched while composing, so this stays valid for the whole
    /// composition.
    pub anchor: usize,
    /// Selection inside [`MarkedText::text`], in UTF-8 bytes (`GCS_CURSORPOS`).
    pub selection: Option<Range<usize>>,
}

impl MarkedText {
    pub fn new(text: &str, anchor: usize, selection: Option<Range<usize>>) -> Self {
        Self {
            text: text.to_string(),
            anchor,
            selection,
        }
    }

    pub fn len(&self) -> usize {
        self.text.len()
    }

    pub fn is_empty(&self) -> bool {
        self.text.is_empty()
    }

    /// The composition's byte range in composite coordinates.
    pub fn composite_range(&self) -> Range<usize> {
        self.anchor..self.anchor + self.len()
    }

    /// Where the IME cursor sits, in composite bytes: `GCS_CURSORPOS` when the
    /// platform sent one, otherwise the end of the string.
    pub fn composite_caret(&self) -> usize {
        self.anchor
            + self
                .selection
                .as_ref()
                .map_or(self.len(), |s| s.end.min(self.len()))
    }

    /// Splice the composition into one block's buffer slice.
    ///
    /// Returns the text to paint and the composition's start **inside that text**
    /// (always a char boundary of `slice`). Used by the render layout's
    /// marked-mode path, which is the only caller: the composition is painted by
    /// the source projection of the block that owns the caret, and by nothing
    /// else.
    pub fn splice(&self, slice: &str, block_src_start: usize) -> (String, usize) {
        let mut local = self.anchor.saturating_sub(block_src_start).min(slice.len());
        while local > 0 && !slice.is_char_boundary(local) {
            local -= 1;
        }
        let mut out = String::with_capacity(slice.len() + self.len());
        out.push_str(&slice[..local]);
        out.push_str(&self.text);
        out.push_str(&slice[local..]);
        (out, local)
    }

    /// Composite byte -> buffer byte. The composition itself contributes zero
    /// buffer bytes, so the whole of it collapses onto `anchor`.
    pub fn to_source(&self, composite: usize) -> usize {
        if composite <= self.anchor {
            composite
        } else {
            (composite - self.len()).max(self.anchor)
        }
    }

    /// Buffer byte -> composite byte. Right-biased: an offset equal to `anchor`
    /// maps to the start of the composition, which is where the caret sits while
    /// nothing is committed.
    pub fn to_composite(&self, src: usize) -> usize {
        if src <= self.anchor {
            src
        } else {
            src + self.len()
        }
    }
}
