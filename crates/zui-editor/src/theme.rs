// Every colour the editor paints with. Painting code never hard-codes one;
// `dark()` is the only place values are written down.
use crate::highlight::HighlightKind;
use gpui::{Hsla, rgb, rgba};

#[derive(Clone, Debug)]
pub struct EditorTheme {
    pub background: Hsla,
    pub text: Hsla,
    pub caret: Hsla,
    pub selection: Hsla,
    pub current_line: Hsla,
    pub gutter_background: Hsla,
    pub gutter_text: Hsla,
    pub gutter_text_active: Hsla,
    pub keyword: Hsla,
    pub type_: Hsla,
    pub number: Hsla,
    pub string: Hsla,
    pub comment: Hsla,
    pub preprocessor: Hsla,
    pub function: Hsla,
    pub builtin: Hsla,
    /// Squiggle and gutter marker for error lines.
    pub error: Hsla,
    /// Inline message after an error line.
    pub error_text: Hsla,
    pub scrollbar: Hsla,
}

impl EditorTheme {
    pub fn dark() -> Self {
        Self {
            background: rgb(0x0f1115).into(),
            text: rgb(0xd7dce4).into(),
            caret: rgb(0x6aa6ff).into(),
            selection: rgba(0x6aa6ff55).into(),
            current_line: rgba(0xffffff0a).into(),
            gutter_background: rgb(0x14171c).into(),
            gutter_text: rgb(0x566070).into(),
            gutter_text_active: rgb(0xa9b3c4).into(),
            keyword: rgb(0xc792ea).into(),
            type_: rgb(0xffcb6b).into(),
            number: rgb(0xf78c6c).into(),
            string: rgb(0xc3e88d).into(),
            comment: rgb(0x6b7689).into(),
            preprocessor: rgb(0x7fdbca).into(),
            function: rgb(0x82aaff).into(),
            builtin: rgb(0x89ddff).into(),
            error: rgb(0xf07178).into(),
            error_text: rgba(0xf07178cc).into(),
            scrollbar: rgba(0x8a93a366).into(),
        }
    }

    pub fn kind_color(&self, kind: HighlightKind) -> Hsla {
        match kind {
            HighlightKind::Keyword => self.keyword,
            HighlightKind::Type => self.type_,
            HighlightKind::Number => self.number,
            HighlightKind::String => self.string,
            HighlightKind::Comment => self.comment,
            HighlightKind::Preprocessor => self.preprocessor,
            HighlightKind::Function => self.function,
            HighlightKind::Builtin => self.builtin,
        }
    }
}

impl Default for EditorTheme {
    fn default() -> Self {
        Self::dark()
    }
}
