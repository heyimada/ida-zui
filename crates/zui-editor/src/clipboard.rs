//! The only door to the clipboard. Under UI automation (`IDA_ZUI_AUTOMATION`
//! set) reads and writes go to a buffer inside this process, so a test run can
//! neither read nor overwrite the user's real clipboard. Everything that
//! copies or pastes must call these functions, never `cx.read_from_clipboard`
//! or `cx.write_to_clipboard`.
use gpui::{App, ClipboardItem};
use std::sync::{Mutex, OnceLock};

static LOCAL: Mutex<Option<String>> = Mutex::new(None);

fn isolated() -> bool {
    static ON: OnceLock<bool> = OnceLock::new();
    *ON.get_or_init(|| std::env::var_os("IDA_ZUI_AUTOMATION").is_some())
}

/// The clipboard text, or `None` when it holds no text.
pub fn read(cx: &App) -> Option<String> {
    if isolated() {
        return LOCAL.lock().unwrap().clone();
    }
    cx.read_from_clipboard().and_then(|item| item.text())
}

pub fn write(text: impl Into<String>, cx: &mut App) {
    let text = text.into();
    if isolated() {
        *LOCAL.lock().unwrap() = Some(text);
    } else {
        cx.write_to_clipboard(ClipboardItem::new_string(text));
    }
}
