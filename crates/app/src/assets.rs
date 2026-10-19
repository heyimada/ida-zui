// Embedded assets: the interface icons (16 px stroke glyphs, tinted by the
// text colour) and the IBM Plex Sans faces (OFL, see assets/fonts).
// The icon table is generated: add a file to assets/icons and run
// scripts/gen_assets.py.
use gpui::{App, AssetSource, SharedString};
use std::borrow::Cow;

pub struct Assets;

impl AssetSource for Assets {
    fn load(&self, path: &str) -> gpui::Result<Option<Cow<'static, [u8]>>> {
        Ok(icon_bytes(path))
    }

    fn list(&self, _: &str) -> gpui::Result<Vec<SharedString>> {
        Ok(Vec::new())
    }
}

fn icon_bytes(path: &str) -> Option<Cow<'static, [u8]>> {
    if path == "icons/back.svg" {
        return Some(Cow::Borrowed(include_bytes!("../assets/icons/back.svg")));
    }
    if path == "icons/bars.svg" {
        return Some(Cow::Borrowed(include_bytes!("../assets/icons/bars.svg")));
    }
    if path == "icons/check.svg" {
        return Some(Cow::Borrowed(include_bytes!("../assets/icons/check.svg")));
    }
    if path == "icons/chevron-down.svg" {
        return Some(Cow::Borrowed(include_bytes!(
            "../assets/icons/chevron-down.svg"
        )));
    }
    if path == "icons/chevron-right.svg" {
        return Some(Cow::Borrowed(include_bytes!(
            "../assets/icons/chevron-right.svg"
        )));
    }
    if path == "icons/close.svg" {
        return Some(Cow::Borrowed(include_bytes!("../assets/icons/close.svg")));
    }
    if path == "icons/code.svg" {
        return Some(Cow::Borrowed(include_bytes!("../assets/icons/code.svg")));
    }
    if path == "icons/contrast.svg" {
        return Some(Cow::Borrowed(include_bytes!(
            "../assets/icons/contrast.svg"
        )));
    }
    if path == "icons/debug.svg" {
        return Some(Cow::Borrowed(include_bytes!("../assets/icons/debug.svg")));
    }
    if path == "icons/dot.svg" {
        return Some(Cow::Borrowed(include_bytes!("../assets/icons/dot.svg")));
    }
    if path == "icons/expand.svg" {
        return Some(Cow::Borrowed(include_bytes!("../assets/icons/expand.svg")));
    }
    if path == "icons/exports.svg" {
        return Some(Cow::Borrowed(include_bytes!("../assets/icons/exports.svg")));
    }
    if path == "icons/file.svg" {
        return Some(Cow::Borrowed(include_bytes!("../assets/icons/file.svg")));
    }
    if path == "icons/filter.svg" {
        return Some(Cow::Borrowed(include_bytes!("../assets/icons/filter.svg")));
    }
    if path == "icons/folder.svg" {
        return Some(Cow::Borrowed(include_bytes!("../assets/icons/folder.svg")));
    }
    if path == "icons/forward.svg" {
        return Some(Cow::Borrowed(include_bytes!("../assets/icons/forward.svg")));
    }
    if path == "icons/functions.svg" {
        return Some(Cow::Borrowed(include_bytes!(
            "../assets/icons/functions.svg"
        )));
    }
    if path == "icons/imports.svg" {
        return Some(Cow::Borrowed(include_bytes!("../assets/icons/imports.svg")));
    }
    if path == "icons/info.svg" {
        return Some(Cow::Borrowed(include_bytes!("../assets/icons/info.svg")));
    }
    if path == "icons/keyboard.svg" {
        return Some(Cow::Borrowed(include_bytes!(
            "../assets/icons/keyboard.svg"
        )));
    }
    if path == "icons/maximize.svg" {
        return Some(Cow::Borrowed(include_bytes!(
            "../assets/icons/maximize.svg"
        )));
    }
    if path == "icons/minimize.svg" {
        return Some(Cow::Borrowed(include_bytes!(
            "../assets/icons/minimize.svg"
        )));
    }
    if path == "icons/minus.svg" {
        return Some(Cow::Borrowed(include_bytes!("../assets/icons/minus.svg")));
    }
    if path == "icons/names.svg" {
        return Some(Cow::Borrowed(include_bytes!("../assets/icons/names.svg")));
    }
    if path == "icons/panel-bottom.svg" {
        return Some(Cow::Borrowed(include_bytes!(
            "../assets/icons/panel-bottom.svg"
        )));
    }
    if path == "icons/panel-right.svg" {
        return Some(Cow::Borrowed(include_bytes!(
            "../assets/icons/panel-right.svg"
        )));
    }
    if path == "icons/play.svg" {
        return Some(Cow::Borrowed(include_bytes!("../assets/icons/play.svg")));
    }
    if path == "icons/plus.svg" {
        return Some(Cow::Borrowed(include_bytes!("../assets/icons/plus.svg")));
    }
    if path == "icons/save.svg" {
        return Some(Cow::Borrowed(include_bytes!("../assets/icons/save.svg")));
    }
    if path == "icons/search.svg" {
        return Some(Cow::Borrowed(include_bytes!("../assets/icons/search.svg")));
    }
    if path == "icons/segments.svg" {
        return Some(Cow::Borrowed(include_bytes!(
            "../assets/icons/segments.svg"
        )));
    }
    if path == "icons/settings.svg" {
        return Some(Cow::Borrowed(include_bytes!(
            "../assets/icons/settings.svg"
        )));
    }
    if path == "icons/sidebar.svg" {
        return Some(Cow::Borrowed(include_bytes!("../assets/icons/sidebar.svg")));
    }
    if path == "icons/spinner.svg" {
        return Some(Cow::Borrowed(include_bytes!("../assets/icons/spinner.svg")));
    }
    if path == "icons/split.svg" {
        return Some(Cow::Borrowed(include_bytes!("../assets/icons/split.svg")));
    }
    if path == "icons/stop.svg" {
        return Some(Cow::Borrowed(include_bytes!("../assets/icons/stop.svg")));
    }
    if path == "icons/strings.svg" {
        return Some(Cow::Borrowed(include_bytes!("../assets/icons/strings.svg")));
    }
    if path == "icons/trash.svg" {
        return Some(Cow::Borrowed(include_bytes!("../assets/icons/trash.svg")));
    }
    if path == "icons/types.svg" {
        return Some(Cow::Borrowed(include_bytes!("../assets/icons/types.svg")));
    }
    if path == "icons/undo.svg" {
        return Some(Cow::Borrowed(include_bytes!("../assets/icons/undo.svg")));
    }
    if path == "icons/view.svg" {
        return Some(Cow::Borrowed(include_bytes!("../assets/icons/view.svg")));
    }
    if path == "icons/warning.svg" {
        return Some(Cow::Borrowed(include_bytes!("../assets/icons/warning.svg")));
    }
    None
}

/// Registers the bundled fonts; the UI falls back to the system font if this fails.
pub fn load_fonts(cx: &mut App) {
    let faces: Vec<Cow<'static, [u8]>> = vec![
        Cow::Borrowed(include_bytes!("../assets/fonts/IBMPlexSans-Regular.ttf")),
        Cow::Borrowed(include_bytes!("../assets/fonts/IBMPlexSans-Medium.ttf")),
        Cow::Borrowed(include_bytes!("../assets/fonts/IBMPlexSans-SemiBold.ttf")),
    ];
    if let Err(e) = cx.text_system().add_fonts(faces) {
        eprintln!("Could not load the interface fonts: {e}");
    }
}
