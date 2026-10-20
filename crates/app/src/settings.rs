// User settings: theme, fonts, keymap overrides and recent files, stored as
// JSON in %APPDATA%\ida-zui\settings.json (or $IDA_ZUI_CONFIG_DIR). A missing
// or corrupt file means defaults; saving writes a temp file and renames it.
use crate::theme::{self, Theme};
use gpui::{App, Global};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

pub const MAX_RECENT: usize = 10;
pub const DEFAULT_INDENT: u32 = 8;
pub const DEFAULT_COMMENT_COLUMN: u32 = 32;

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct Recent {
    pub path: String,
    /// Seconds since the Unix epoch.
    pub opened: u64,
    /// "x86-64", "ARM", ...: what the database was when it last opened.
    #[serde(default)]
    pub arch: String,
}

/// Which panels are open and how big, remembered between runs.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct PanelLayout {
    pub sidebar: bool,
    pub inspector: bool,
    pub bottom: bool,
    /// Listing (or graph) and pseudocode side by side.
    pub split: bool,
    pub sidebar_w: f32,
    pub inspector_w: f32,
    pub bottom_h: f32,
    /// Activity rail selection: `functions`, `names`, ...
    pub rail: String,
    /// `output`, `xrefs`, `python` or a debugger panel (`registers`, ...).
    pub bottom_tab: String,
    /// `info`, `type`, `xrefs` or `comments`.
    pub inspector_tab: String,
}

impl Default for PanelLayout {
    fn default() -> Self {
        Self {
            sidebar: true,
            inspector: true,
            bottom: true,
            split: false,
            sidebar_w: 264.,
            inspector_w: 300.,
            bottom_h: 152.,
            rail: "functions".to_owned(),
            bottom_tab: "xrefs".to_owned(),
            inspector_tab: "info".to_owned(),
        }
    }
}

impl PanelLayout {
    fn clamp(&mut self) {
        self.sidebar_w = self.sidebar_w.clamp(200., 480.);
        self.inspector_w = self.inspector_w.clamp(240., 480.);
        self.bottom_h = self.bottom_h.clamp(96., 600.);
    }
}

/// What happens to exceptions in the debuggee when a session starts.
#[derive(Clone, Copy, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ExceptionPolicy {
    /// IDA's own table (stop on the usual faults).
    #[default]
    Ida,
    /// Never stop; every exception goes to the program.
    PassAll,
    /// Stop on every exception.
    StopAll,
}

impl ExceptionPolicy {
    pub const ALL: [ExceptionPolicy; 3] = [Self::Ida, Self::PassAll, Self::StopAll];

    pub fn label(self) -> &'static str {
        match self {
            Self::Ida => "IDA defaults",
            Self::PassAll => "Pass all to the program",
            Self::StopAll => "Stop on all",
        }
    }
}

/// How the debugger starts a process (Settings > Debugger).
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct DebuggerSettings {
    /// The program to run; empty means the database's input file.
    pub app_path: String,
    pub args: String,
    /// Working directory; empty means the program's own folder.
    pub cwd: String,
    pub stop_at_entry: bool,
    pub stop_on_library: bool,
    pub stop_on_thread: bool,
    pub disable_aslr: bool,
    pub exceptions: ExceptionPolicy,
}

impl Default for DebuggerSettings {
    fn default() -> Self {
        Self {
            app_path: String::new(),
            args: String::new(),
            cwd: String::new(),
            stop_at_entry: true,
            stop_on_library: false,
            stop_on_thread: false,
            disable_aslr: false,
            exceptions: ExceptionPolicy::Ida,
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct Settings {
    pub theme: String,
    pub code_font: String,
    pub code_font_size: f32,
    pub ui_font_size: f32,
    /// Tighter code rows (about 25% more lines on screen).
    pub compact_rows: bool,
    /// Column of the instruction in listing and graph lines (IDA's default is 16).
    pub listing_indent: u32,
    /// Column of the end-of-line comment (IDA's default is 40).
    pub listing_comment: u32,
    /// Run auto-analysis when a binary is opened (the open dialog can still
    /// turn it off for one file).
    pub analysis_on_open: bool,
    /// Show the analysis stage and percentage while analysing; off leaves
    /// elapsed time only.
    pub show_analysis_progress: bool,
    /// How many console commands the Python console remembers.
    pub python_history_size: usize,
    /// The script file last run from File > Script file.
    pub last_script: String,
    /// The IDA folder picked with "Locate IDA..." (`IDADIR` overrides it).
    pub ida_dir: Option<String>,
    /// Action name (`ida::Rename`) to keystrokes; replaces that action's defaults.
    pub keymap: BTreeMap<String, Vec<String>>,
    pub recent: Vec<Recent>,
    pub layout: PanelLayout,
    pub debugger: DebuggerSettings,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            theme: Theme::DARK.name.to_owned(),
            code_font: theme::DEFAULT_CODE_FONT.to_owned(),
            code_font_size: theme::DEFAULT_CODE_SIZE,
            ui_font_size: theme::DEFAULT_UI_SIZE,
            compact_rows: false,
            listing_indent: DEFAULT_INDENT,
            listing_comment: DEFAULT_COMMENT_COLUMN,
            analysis_on_open: true,
            show_analysis_progress: true,
            python_history_size: crate::py_history::DEFAULT_CAP,
            last_script: String::new(),
            ida_dir: None,
            keymap: BTreeMap::new(),
            recent: Vec::new(),
            layout: PanelLayout::default(),
            debugger: DebuggerSettings::default(),
        }
    }
}

impl Settings {
    /// Reads `path`; a missing, unreadable or corrupt file gives defaults.
    pub fn load(path: &Path) -> Self {
        let mut s: Self = std::fs::read_to_string(path)
            .ok()
            .and_then(|t| serde_json::from_str(&t).ok())
            .unwrap_or_default();
        s.code_font_size = s.code_font_size.clamp(8., 36.);
        s.ui_font_size = s.ui_font_size.clamp(10., 24.);
        s.listing_indent = s.listing_indent.clamp(0, 32);
        s.listing_comment = s.listing_comment.clamp(8, 96);
        s.python_history_size = s.python_history_size.clamp(10, 5000);
        if s.code_font.trim().is_empty() {
            s.code_font = theme::DEFAULT_CODE_FONT.to_owned();
        }
        s.layout.clamp();
        s.recent.retain(|r| Path::new(&r.path).exists());
        s.recent.truncate(MAX_RECENT);
        s
    }

    pub fn save(&self, path: &Path) -> std::io::Result<()> {
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        let tmp = path.with_extension("json.tmp");
        std::fs::write(
            &tmp,
            serde_json::to_vec_pretty(self).map_err(std::io::Error::other)?,
        )?;
        std::fs::rename(&tmp, path)
    }

    pub fn theme(&self) -> Theme {
        Theme::by_name(&self.theme).unwrap_or(Theme::DARK)
    }

    /// Moves `path` to the front of the recent list.
    pub fn touch_recent(&mut self, path: &Path, now: u64) {
        let p = path.display().to_string();
        self.recent.retain(|r| !r.path.eq_ignore_ascii_case(&p));
        self.recent.insert(
            0,
            Recent {
                path: p,
                opened: now,
                arch: String::new(),
            },
        );
        self.recent.truncate(MAX_RECENT);
    }
}

/// `$IDA_ZUI_CONFIG_DIR`, else `%APPDATA%\ida-zui`.
pub fn config_dir() -> PathBuf {
    std::env::var_os("IDA_ZUI_CONFIG_DIR")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("APPDATA").map(|a| PathBuf::from(a).join("ida-zui")))
        .unwrap_or_else(|| PathBuf::from("."))
}

pub fn settings_path() -> PathBuf {
    config_dir().join("settings.json")
}

struct Store(Settings);

impl Global for Store {}

pub fn init(cx: &mut App) {
    let s = Settings::load(&settings_path());
    cx.set_global(Store(s));
    apply(cx);
}

pub fn get(cx: &App) -> &Settings {
    &cx.global::<Store>().0
}

/// Mutates the settings, then saves and applies them to every window.
pub fn update(cx: &mut App, f: impl FnOnce(&mut Settings)) {
    f(&mut cx.global_mut::<Store>().0);
    if let Err(e) = get(cx).save(&settings_path()) {
        eprintln!("settings: cannot save: {e}");
    }
    apply(cx);
}

/// Pushes the current settings into the theme, the font cells and the keymap.
pub fn apply(cx: &mut App) {
    let s = get(cx).clone();
    ida_proto::client::set_ida_dir_setting(s.ida_dir.as_ref().map(PathBuf::from));
    cx.set_global(s.theme());
    theme::set_fonts(&s.code_font, s.code_font_size, s.ui_font_size);
    theme::set_compact_rows(s.compact_rows);
    crate::actions::apply_keymap(&s.keymap, cx);
    cx.refresh_windows();
}

pub fn unix_now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs())
}

/// "3 min ago", "2 days ago" for the recent list.
pub fn ago(then: u64, now: u64) -> String {
    let d = now.saturating_sub(then);
    let (n, unit) = match d {
        0..=59 => return "just now".to_owned(),
        60..=3599 => (d / 60, "min"),
        3600..=86399 => (d / 3600, "hour"),
        86400..=2591999 => (d / 86400, "day"),
        _ => (d / 2592000, "month"),
    };
    format!(
        "{n} {unit}{} ago",
        if n == 1 || unit == "min" { "" } else { "s" }
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("ida-zui-test-{}-{name}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        dir.join("settings.json")
    }

    #[test]
    fn missing_or_corrupt_files_give_defaults() {
        let missing = temp("missing");
        assert_eq!(Settings::load(&missing), Settings::default());
        let corrupt = temp("corrupt");
        std::fs::write(&corrupt, "{ not json").unwrap();
        assert_eq!(Settings::load(&corrupt), Settings::default());
    }

    #[test]
    fn partial_files_keep_their_values_and_clamp_nonsense() {
        let p = temp("partial");
        std::fs::write(&p, r#"{"theme":"Light","code_font_size":500}"#).unwrap();
        let s = Settings::load(&p);
        assert_eq!(s.theme().name, "Light");
        assert_eq!(s.code_font_size, 36.);
        assert_eq!(s.ui_font_size, theme::DEFAULT_UI_SIZE);
    }

    #[test]
    fn unknown_theme_falls_back_to_dark() {
        let s = Settings {
            theme: "Nope".into(),
            ..Settings::default()
        };
        assert_eq!(s.theme().name, "Dark");
    }

    #[test]
    fn recent_list_is_most_recent_first_unique_and_capped() {
        let mut s = Settings::default();
        for i in 0..15 {
            s.touch_recent(Path::new(&format!(r"C:\f{i}.exe")), i);
        }
        s.touch_recent(Path::new(r"c:\F7.EXE"), 99);
        assert_eq!(s.recent.len(), MAX_RECENT);
        assert_eq!(s.recent[0].path, r"c:\F7.EXE");
        assert_eq!(
            s.recent
                .iter()
                .filter(|r| r.path.eq_ignore_ascii_case(r"C:\f7.exe"))
                .count(),
            1
        );
    }

    #[test]
    fn load_prunes_missing_recent_files() {
        let p = temp("prune");
        let real = p.with_file_name("real.bin");
        std::fs::write(&real, b"x").unwrap();
        let mut s = Settings::default();
        s.touch_recent(&real, 1);
        s.touch_recent(Path::new(r"C:\definitely\not\here.bin"), 2);
        s.save(&p).unwrap();
        let loaded = Settings::load(&p);
        assert_eq!(loaded.recent.len(), 1);
        assert_eq!(loaded.recent[0].path, real.display().to_string());
    }
}
