// The palettes ("Workbench x Native Pro"). Views never
// hard-code colours; IDA's syntax colour codes map through `Theme::tag`. The
// active theme is a gpui global; fonts live in process-wide cells so painting
// and hit-testing can read them without a context.
use gpui::{App, Global, Hsla, SharedString, rgb, rgba};
use ida_proto::tags::color as c;
use std::sync::RwLock;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};

/// Every colour is `0xRRGGBB`, except `scrim` and `shadow` (`0xRRGGBBAA`).
#[derive(Clone, Copy, Debug)]
pub struct Theme {
    pub name: &'static str,
    /// Window chrome: title strip, menu bar, toolbar, rail, status bar, tab strip.
    pub bg: u32,
    /// Editor and list content.
    pub surface: u32,
    /// Side panels: sidebar, inspector, pane headers, segmented track.
    pub surface2: u32,
    /// Menus, palette, dialogs, tooltips.
    pub popover: u32,
    /// Chips, badges, key caps, scrollbar thumbs.
    pub raised: u32,
    pub hover: u32,
    pub border: u32,
    pub border_strong: u32,
    pub text: u32,
    pub muted: u32,
    pub faint: u32,
    pub accent: u32,
    /// Text on an accent fill.
    pub on_accent: u32,
    pub accent_soft: u32,
    pub ring: u32,
    pub selection: u32,
    pub caret_row: u32,
    /// Occurrences of the identifier under the caret.
    pub hl: u32,
    pub danger: u32,
    pub danger_soft: u32,
    pub success: u32,
    pub success_soft: u32,
    pub warning: u32,
    pub warning_soft: u32,
    pub scrim: u32,
    pub shadow: u32,
    pub syntax: Syntax,
}

/// Listing and pseudocode colours .
#[derive(Clone, Copy, Debug)]
pub struct Syntax {
    pub insn: u32,
    pub reg: u32,
    pub num: u32,
    pub str: u32,
    pub cmt: u32,
    pub sym: u32,
    pub code_name: u32,
    pub data_name: u32,
    pub imp_name: u32,
    pub loc_name: u32,
    pub kw: u32,
    pub ty: u32,
}

/// Type names that IDA and Hex-Rays colour as keywords; the design gives them their own token.
const TYPE_WORDS: &[&str] = &[
    "void", "char", "short", "int", "long", "float", "double", "signed", "unsigned", "bool",
    "_BOOL1", "_BOOL2", "_BOOL4", "_BOOL8", "__int8", "__int16", "__int32", "__int64", "__int128",
    "_BYTE", "_WORD", "_DWORD", "_QWORD", "_OWORD", "_UNKNOWN", "byte", "word", "dword", "qword",
    "ptr", "xmmword", "ymmword", "tbyte", "oword", "size_t", "wchar_t", "BYTE", "WORD", "DWORD",
    "QWORD", "BOOL", "HANDLE", "LPVOID", "PVOID", "NTSTATUS",
];

/// Whether `word` is one of the type names the design colours as types.
pub fn is_type_word(word: &str) -> bool {
    TYPE_WORDS.contains(&word)
}

impl Theme {
    pub const DARK: Self = Self {
        name: "Dark",
        bg: 0x1c1d21,
        surface: 0x232428,
        surface2: 0x2a2b30,
        popover: 0x2f3036,
        raised: 0x383a41,
        hover: 0x32343b,
        border: 0x3b3d44,
        border_strong: 0x4b4e57,
        text: 0xe8e9ec,
        muted: 0xb2b6be,
        faint: 0xa3a8b1,
        accent: 0x6fb0ff,
        on_accent: 0x06101f,
        accent_soft: 0x2a3d58,
        ring: 0x6fb0ff,
        selection: 0x273e60,
        caret_row: 0x2d3038,
        hl: 0x453b1b,
        danger: 0xff8c85,
        danger_soft: 0x42262b,
        success: 0x62c48e,
        success_soft: 0x1f3a2c,
        warning: 0xe3b459,
        warning_soft: 0x43361a,
        scrim: 0x06080c9e,
        shadow: 0x00000073,
        syntax: Syntax {
            insn: 0x82b4f7,
            reg: 0xecaa6e,
            num: 0xd6a4ea,
            str: 0x94d494,
            cmt: 0xa6adb9,
            sym: 0xc3c9d0,
            code_name: 0x8fcbff,
            data_name: 0xe3c67f,
            imp_name: 0x74d4c8,
            loc_name: 0xc8bcf9,
            kw: 0xf48fa0,
            ty: 0x80c8b4,
        },
    };

    pub const LIGHT: Self = Self {
        name: "Light",
        bg: 0xebebee,
        surface: 0xffffff,
        surface2: 0xf4f4f6,
        popover: 0xffffff,
        raised: 0xe3e4e8,
        hover: 0xe6e7eb,
        border: 0xd4d5db,
        border_strong: 0xbabcc5,
        text: 0x1c1d21,
        muted: 0x545862,
        faint: 0x5e636d,
        accent: 0x0a5cd0,
        on_accent: 0xffffff,
        accent_soft: 0xdce8fc,
        ring: 0x0a5cd0,
        selection: 0xd2e2fb,
        caret_row: 0xeef3fc,
        hl: 0xffe08a,
        danger: 0xb3212f,
        danger_soft: 0xfbe3e5,
        success: 0x176a3b,
        success_soft: 0xdff2e6,
        warning: 0x8a5700,
        warning_soft: 0xfbefd0,
        scrim: 0x1218225c,
        shadow: 0x141c282e,
        syntax: Syntax {
            insn: 0x1a55b5,
            reg: 0x9b4a08,
            num: 0x7529a0,
            str: 0x17683a,
            cmt: 0x56626e,
            sym: 0x38404a,
            code_name: 0x0a5aa8,
            data_name: 0x855400,
            imp_name: 0x0a6a62,
            loc_name: 0x5640b2,
            kw: 0xb01f3d,
            ty: 0x0a6a62,
        },
    };

    pub const HIGH_CONTRAST: Self = Self {
        name: "High contrast",
        bg: 0x000000,
        surface: 0x050505,
        surface2: 0x0f0f0f,
        popover: 0x0a0a0a,
        raised: 0x242424,
        hover: 0x1c1c1c,
        border: 0x7a7a7a,
        border_strong: 0xb0b0b0,
        text: 0xffffff,
        muted: 0xe0e0e0,
        faint: 0xbdbdbd,
        accent: 0x4fc3ff,
        on_accent: 0x000000,
        accent_soft: 0x0b3552,
        ring: 0xffe14d,
        selection: 0x143b5c,
        caret_row: 0x14202c,
        hl: 0x403500,
        danger: 0xff7b7b,
        danger_soft: 0x4a1616,
        success: 0x5dff7a,
        success_soft: 0x0f3d1a,
        warning: 0xffe14d,
        warning_soft: 0x4a3f00,
        scrim: 0x000000cc,
        shadow: 0x000000cc,
        syntax: Syntax {
            insn: 0xa8d4ff,
            reg: 0xffb066,
            num: 0xe8b8ff,
            str: 0x9dff7a,
            cmt: 0xc8c8c8,
            sym: 0xe6e6e6,
            code_name: 0x7fdcff,
            data_name: 0xffe14d,
            imp_name: 0x5ffbe0,
            loc_name: 0xd2c8ff,
            kw: 0xff9cb0,
            ty: 0x8cf0d6,
        },
    };

    /// IDA's own default listing colours on its white background.
    pub const IDA_CLASSIC: Self = Self {
        name: "IDA Classic",
        bg: 0xece9d8,
        surface: 0xffffff,
        surface2: 0xf3f1e6,
        popover: 0xfaf9f2,
        raised: 0xdcd9c8,
        hover: 0xe4e1d0,
        border: 0xaca899,
        border_strong: 0x7f7c70,
        text: 0x000000,
        muted: 0x404040,
        faint: 0x555555,
        accent: 0x1f4e9e,
        on_accent: 0xffffff,
        accent_soft: 0xcddcf3,
        ring: 0x1f4e9e,
        selection: 0xc4d4ee,
        caret_row: 0xe0e8f8,
        hl: 0xffff80,
        danger: 0xb00000,
        danger_soft: 0xf6d5d5,
        success: 0x006400,
        success_soft: 0xd5ecd5,
        warning: 0x7a5200,
        warning_soft: 0xf6e8c4,
        scrim: 0x00000055,
        shadow: 0x00000040,
        syntax: Syntax {
            insn: 0x000000,
            reg: 0x000080,
            num: 0x0000ff,
            str: 0x006000,
            cmt: 0x0000b0,
            sym: 0x000000,
            code_name: 0x000080,
            data_name: 0x800000,
            imp_name: 0x800080,
            loc_name: 0x000080,
            kw: 0x000080,
            ty: 0x005f5f,
        },
    };

    pub const ALL: [Self; 4] = [
        Self::DARK,
        Self::LIGHT,
        Self::HIGH_CONTRAST,
        Self::IDA_CLASSIC,
    ];

    pub fn by_name(name: &str) -> Option<Self> {
        Self::ALL
            .into_iter()
            .find(|t| t.name.eq_ignore_ascii_case(name))
    }

    pub fn get(cx: &App) -> Self {
        *cx.global::<Self>()
    }

    /// Like `tag`, but the keyword family also carries type names, which get their own token.
    pub fn tag_text(&self, code: u8, text: &str) -> Hsla {
        if matches!(code, c::KEYWORD | c::MACRO) && TYPE_WORDS.contains(&text.trim()) {
            return hsla(self.syntax.ty);
        }
        self.tag(code)
    }

    /// Foreground colour for an IDA syntax colour code.
    pub fn tag(&self, code: u8) -> Hsla {
        let s = &self.syntax;
        let hex = match code {
            c::INSN => s.insn,
            c::KEYWORD | c::MACRO | c::ASMDIR | c::SEGNAME => s.kw,
            c::REG => s.reg,
            c::NUMBER | c::DNUM => s.num,
            c::STRING | c::DSTR | c::CHAR | c::DCHAR => s.str,
            c::REGCMT | c::RPTCMT | c::AUTOCMT | c::DEMNAME | c::EXTRA | c::COLLAPSED => s.cmt,
            c::SYMBOL | c::ALTOP | c::VOIDOP | c::HIDNAME => s.sym,
            c::CNAME | c::CODNAME | c::CREF | c::CREFTAIL => s.code_name,
            c::DNAME | c::DATNAME | c::DREF | c::DREFTAIL | c::UNAME | c::UNKNAME => s.data_name,
            c::IMPNAME | c::LIBNAME => s.imp_name,
            c::LOCNAME => s.loc_name,
            c::PREFIX | c::BINPREF => self.faint,
            c::ERROR => self.danger,
            _ => self.text,
        };
        rgb(hex).into()
    }

    /// The multi-line editor's colours for this palette.
    pub fn editor(&self) -> zui_editor::EditorTheme {
        let s = &self.syntax;
        let alpha = |hex: u32, a: f32| Hsla { a, ..hsla(hex) };
        zui_editor::EditorTheme {
            background: hsla(self.surface),
            text: hsla(self.text),
            caret: hsla(self.accent),
            selection: hsla(self.selection),
            current_line: hsla(self.caret_row),
            gutter_background: hsla(self.surface2),
            gutter_text: hsla(self.faint),
            gutter_text_active: hsla(self.text),
            keyword: hsla(s.kw),
            type_: hsla(s.ty),
            number: hsla(s.num),
            string: hsla(s.str),
            comment: hsla(s.cmt),
            preprocessor: hsla(s.reg),
            function: hsla(s.code_name),
            builtin: hsla(s.imp_name),
            error: hsla(self.danger),
            error_text: alpha(self.danger, 0.85),
            scrollbar: hsla(self.raised),
        }
    }
}

impl Global for Theme {}

pub fn hsla(color: u32) -> Hsla {
    rgb(color).into()
}

/// A colour that already carries its alpha byte (`scrim`, `shadow`).
pub fn hsla_a(color: u32) -> Hsla {
    rgba(color).into()
}

pub const UI_FONT: &str = "IBM Plex Sans";
pub const DEFAULT_CODE_FONT: &str = "Cascadia Mono";
pub const DEFAULT_CODE_SIZE: f32 = 12.;
pub const DEFAULT_UI_SIZE: f32 = 12.;

static CODE_FONT: RwLock<Option<SharedString>> = RwLock::new(None);
static CODE_SIZE: AtomicU32 = AtomicU32::new(0);
static UI_SIZE: AtomicU32 = AtomicU32::new(0);
static COMPACT_ROWS: AtomicBool = AtomicBool::new(false);

pub fn set_fonts(code_family: &str, code_size: f32, ui_size: f32) {
    *CODE_FONT.write().unwrap() = Some(SharedString::from(code_family.to_owned()));
    CODE_SIZE.store(code_size.to_bits(), Ordering::Relaxed);
    UI_SIZE.store(ui_size.to_bits(), Ordering::Relaxed);
}

pub fn code_font_name() -> SharedString {
    CODE_FONT
        .read()
        .unwrap()
        .clone()
        .unwrap_or_else(|| DEFAULT_CODE_FONT.into())
}

pub fn code_font_size() -> f32 {
    match CODE_SIZE.load(Ordering::Relaxed) {
        0 => DEFAULT_CODE_SIZE,
        bits => f32::from_bits(bits),
    }
}

/// Comfortable (the default) or compact code rows; see `code_line_height`.
pub fn set_compact_rows(compact: bool) {
    COMPACT_ROWS.store(compact, Ordering::Relaxed);
}

/// Height of one code row: 20 px at the default 12 px code size, 16 px when
/// compact.
pub fn code_line_height() -> f32 {
    let ratio = if COMPACT_ROWS.load(Ordering::Relaxed) {
        1.35
    } else {
        5. / 3.
    };
    (code_font_size() * ratio).round()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lum(c: u32) -> f64 {
        let ch = |shift: u32| {
            let v = ((c >> shift) & 0xff) as f64 / 255.;
            if v <= 0.03928 {
                v / 12.92
            } else {
                ((v + 0.055) / 1.055).powf(2.4)
            }
        };
        0.2126 * ch(16) + 0.7152 * ch(8) + 0.0722 * ch(0)
    }

    /// WCAG contrast ratio of two 0xRRGGBB colours.
    fn contrast(a: u32, b: u32) -> f64 {
        let (la, lb) = (lum(a), lum(b));
        (la.max(lb) + 0.05) / (la.min(lb) + 0.05)
    }

    /// Every text token must read at 4.5:1 on the grounds code text lands on
    /// in every theme: the selected row and the identifier highlight. The two
    /// themes that carry an accessibility promise (High contrast, and IDA
    /// Classic's real colours) are checked on every surface.
    #[test]
    fn text_tokens_reach_their_contrast_on_every_surface() {
        let mut failures = Vec::new();
        for (t, floor, every_surface) in [
            (Theme::DARK, 4.5, false),
            (Theme::LIGHT, 4.5, false),
            (Theme::HIGH_CONTRAST, 4.5, true),
            (Theme::IDA_CLASSIC, 4.5, true),
        ] {
            let mut grounds = vec![("selection", t.selection), ("hl", t.hl)];
            if every_surface {
                grounds.extend([
                    ("bg", t.bg),
                    ("surface", t.surface),
                    ("surface2", t.surface2),
                    ("popover", t.popover),
                    ("raised", t.raised),
                    ("hover", t.hover),
                    ("caret_row", t.caret_row),
                ]);
            }
            let s = t.syntax;
            let texts = [
                ("text", t.text),
                ("muted", t.muted),
                ("faint", t.faint),
                ("accent", t.accent),
                ("danger", t.danger),
                ("success", t.success),
                ("warning", t.warning),
                ("insn", s.insn),
                ("reg", s.reg),
                ("num", s.num),
                ("str", s.str),
                ("cmt", s.cmt),
                ("sym", s.sym),
                ("code_name", s.code_name),
                ("data_name", s.data_name),
                ("imp_name", s.imp_name),
                ("loc_name", s.loc_name),
                ("kw", s.kw),
                ("ty", s.ty),
            ];
            for (tn, tc) in texts {
                for &(gn, gc) in &grounds {
                    let r = contrast(tc, gc);
                    if r < floor {
                        failures.push(format!("{}: {tn} on {gn} = {r:.2}", t.name));
                    }
                }
            }
            // Text on fills that carry their own text colour.
            for (label, fg, bg) in [
                ("on_accent on accent", t.on_accent, t.accent),
                ("danger on danger_soft", t.danger, t.danger_soft),
                ("success on success_soft", t.success, t.success_soft),
                ("warning on warning_soft", t.warning, t.warning_soft),
                ("accent on accent_soft", t.accent, t.accent_soft),
                ("text on hl", t.text, t.hl),
            ] {
                let r = contrast(fg, bg);
                if r < floor {
                    failures.push(format!("{}: {label} = {r:.2}", t.name));
                }
            }
        }
        assert!(
            failures.is_empty(),
            "low contrast:\n{}",
            failures.join("\n")
        );
    }
}
