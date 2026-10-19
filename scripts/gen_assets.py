"""Regenerates crates/app/src/assets.rs from crates/app/assets/icons/*.svg."""
import os
import pathlib
root = str(pathlib.Path(__file__).resolve().parent.parent / "crates" / "app")
names = sorted(f[:-4] for f in os.listdir(root + r"\assets\icons") if f.endswith(".svg"))
blocks = "\n".join(
    f'    if path == "icons/{n}.svg" {{\n        return Some(Cow::Borrowed(include_bytes!("../assets/icons/{n}.svg")));\n    }}'
    for n in names
)
fonts = ["IBMPlexSans-Regular", "IBMPlexSans-Medium", "IBMPlexSans-SemiBold"]
fl = "\n".join(f'        Cow::Borrowed(include_bytes!("../assets/fonts/{f}.ttf")),' for f in fonts)
src = f"""// Embedded assets: the interface icons (16 px stroke glyphs, tinted by the
// text colour) and the IBM Plex Sans faces (OFL, see assets/fonts).
// The icon table is generated: add a file to assets/icons and run
// scripts/gen_assets.py.
use gpui::{{App, AssetSource, SharedString}};
use std::borrow::Cow;

pub struct Assets;

impl AssetSource for Assets {{
    fn load(&self, path: &str) -> gpui::Result<Option<Cow<'static, [u8]>>> {{
        Ok(icon_bytes(path))
    }}

    fn list(&self, _: &str) -> gpui::Result<Vec<SharedString>> {{
        Ok(Vec::new())
    }}
}}

fn icon_bytes(path: &str) -> Option<Cow<'static, [u8]>> {{
{blocks}
    None
}}

/// Registers the bundled fonts; the UI falls back to the system font if this fails.
pub fn load_fonts(cx: &mut App) {{
    let faces: Vec<Cow<'static, [u8]>> = vec![
{fl}
    ];
    if let Err(e) = cx.text_system().add_fonts(faces) {{
        eprintln!("Could not load the interface fonts: {{e}}");
    }}
}}
"""
open(root + r"\src\assets.rs", "w", encoding="utf8").write(src)
print(len(names), "icons")
