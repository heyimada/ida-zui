//! A multi-line code editor view for zui (gpui): virtualised painting, IME,
//! undo, pluggable highlighting and error markers.
//!
//! ```ignore
//! zui_editor::bind_keys(cx);
//! let editor = cx.new(|cx| {
//!     Editor::new(cx).with_text("int x;").with_highlighter(CHighlighter)
//! });
//! cx.subscribe(&editor, |_, event: &EditorEvent, _| { /* Changed, Submit */ }).detach();
//! ```
pub mod clipboard;
pub mod doc;
mod editor;
mod element;
pub mod highlight;
pub mod runs;
pub mod text;
mod theme;

pub use doc::{Doc, Pos};
pub use editor::*;
pub use highlight::{CHighlighter, HighlightKind, Highlighter, LineState, PythonHighlighter};
pub use theme::EditorTheme;
