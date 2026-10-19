// Plugins: the list the host reports (shared by the menu, the palette, the
// keymap and the Plugins window), the command each runnable plugin becomes,
// and what to tell the user after a run. idalib has no Qt UI, so a plugin that
// needs one does nothing; `outcome` says so instead of pretending.
use crate::actions::{self, Command};
use gpui::{Action, App};
use ida_proto::{PluginInfo, PluginRunReply, plugin_flags};
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Mutex, RwLock};

/// Runs one entry of the plugin list. Carries the path and argument, not an
/// index, so a key binding survives the list being reloaded.
#[derive(Clone, PartialEq, Action)]
#[action(namespace = ida, no_json)]
pub struct RunPlugin {
    pub path: String,
    pub arg: u64,
}

static LIST: RwLock<Vec<PluginInfo>> = RwLock::new(Vec::new());

/// The plugins of the last database that reported them.
pub fn list() -> Vec<PluginInfo> {
    LIST.read().map(|l| l.clone()).unwrap_or_default()
}

/// IDA lists a plugin once per configured argument and some twice (a user copy
/// and the stock one); the same file with the same argument is shown once.
pub fn dedupe(mut list: Vec<PluginInfo>) -> Vec<PluginInfo> {
    let mut seen: Vec<(String, u64)> = Vec::new();
    list.retain(|p| {
        let key = (p.path.to_lowercase(), p.arg);
        let fresh = !seen.contains(&key);
        seen.push(key);
        fresh
    });
    list.sort_by_key(|p| p.name.to_lowercase());
    list
}

/// Shown in IDA's Plugins menu: not hidden and not a debugger.
pub fn runnable(p: &PluginInfo) -> bool {
    p.flags & (plugin_flags::HIDE | plugin_flags::DBG) == 0
}

pub fn command_name(p: &PluginInfo) -> String {
    format!("plugin:{}:{}", p.id, p.arg)
}

pub fn run_action(p: &PluginInfo) -> RunPlugin {
    RunPlugin {
        path: p.path.clone(),
        arg: p.arg,
    }
}

/// "Ctrl-Alt-M" or "Ctrl+Shift+F11" in IDA's spelling, as a gpui keystroke
/// (`ctrl-alt-m`). `None` when it is not a single key with modifiers.
pub fn hotkey_to_keystroke(text: &str) -> Option<String> {
    let mut mods: Vec<&str> = Vec::new();
    let mut key: Option<String> = None;
    // "Ctrl+-" and "Ctrl--" end in a literal minus: split it off first.
    let (head, tail) = match text.trim().strip_suffix(['-', '+']) {
        Some(h) if h.ends_with(['-', '+']) => {
            (h.trim_end_matches(['-', '+']), Some("-".to_owned()))
        }
        _ => (text.trim(), None),
    };
    for part in head.split(['-', '+']).filter(|p| !p.is_empty()) {
        match part.to_lowercase().as_str() {
            "ctrl" | "control" => mods.push("ctrl"),
            "alt" => mods.push("alt"),
            "shift" => mods.push("shift"),
            "meta" | "win" | "cmd" => mods.push("platform"),
            other => {
                if key.is_some() {
                    return None;
                }
                key = Some(other.to_owned());
            }
        }
    }
    let key = match (key, tail) {
        (Some(k), None) => k,
        (None, Some(t)) => t,
        _ => return None,
    };
    if mods.is_empty() && key.chars().count() == 1 {
        // A bare letter would swallow typing in the code views.
        return None;
    }
    let mut out = mods.join("-");
    if !out.is_empty() {
        out.push('-');
    }
    out.push_str(&key);
    gpui::Keystroke::parse(&out).ok().map(|k| k.unparse())
}

static CACHE: Mutex<Option<HashMap<String, &'static Command>>> = Mutex::new(None);

/// One catalog command per runnable plugin. The few strings leak once per
/// distinct plugin; the cache makes reloading the list free.
fn command_for(p: &PluginInfo) -> &'static Command {
    let default = hotkey_to_keystroke(&p.hotkey)
        .filter(|k| actions::conflict("", k, &Default::default()).is_none());
    let key = format!("{}|{}|{:?}", command_name(p), p.name, default);
    let mut cache = CACHE.lock().unwrap_or_else(|e| e.into_inner());
    if let Some(c) = cache.get_or_insert_with(HashMap::new).get(&key) {
        return c;
    }
    let action = run_action(p);
    let keys: &'static [(&'static str, &'static str)] = match default {
        Some(k) => {
            Box::leak(vec![(&*Box::leak(k.into_boxed_str()), "Workspace")].into_boxed_slice())
        }
        None => &[],
    };
    let c: &'static Command = Box::leak(Box::new(Command {
        name: Box::leak(command_name(p).into_boxed_str()),
        label: Box::leak(format!("Run plugin: {}", p.name).into_boxed_str()),
        make: Box::new(move || Box::new(action.clone())),
        needs_db: true,
        palette: true,
        keywords: Box::leak(format!("plugin {} {}", p.id, p.comment).into_boxed_str()),
        group: "Plugins",
        keys,
    }));
    cache.get_or_insert_with(HashMap::new).insert(key, c);
    c
}

/// Replaces the shared list, rebuilds the plugin commands and rebinds the keys.
pub fn set_list(plugins: Vec<PluginInfo>, cx: &mut App) {
    let plugins = dedupe(plugins);
    let commands = plugins
        .iter()
        .filter(|p| runnable(p))
        .map(command_for)
        .collect();
    if let Ok(mut l) = LIST.write() {
        *l = plugins;
    }
    actions::set_plugin_commands(commands);
    let keymap = crate::settings::get(cx).keymap.clone();
    actions::apply_keymap(&keymap, cx);
    cx.refresh_windows();
}

/// Whether the plugin ships with IDA or comes from the user's directories.
pub fn from_ida_dir(p: &PluginInfo) -> bool {
    let ida = ida_proto::client::HostConfig::discover().ida_dir;
    let norm = |s: &str| s.replace('/', "\\").to_lowercase();
    norm(&p.path).starts_with(&norm(&ida.display().to_string()))
}

/// The user's plugins folder (the first IDAUSR entry that is not ours).
pub fn user_plugins_dir() -> PathBuf {
    let base = std::env::var("IDAUSR")
        .ok()
        .and_then(|v| v.split(';').find(|s| !s.is_empty()).map(PathBuf::from))
        .unwrap_or_else(|| {
            PathBuf::from(std::env::var("APPDATA").unwrap_or_default())
                .join("Hex-Rays")
                .join("IDA Pro")
        });
    base.join("plugins")
}

/// What a finished run tells the user.
pub struct Outcome {
    pub ok: bool,
    /// One line for the status bar.
    pub short: String,
    /// The long text of the dialog, when one is warranted.
    pub detail: Option<String>,
}

const NEEDS_UI: &str = "This plugin requires IDA's graphical interface and can't run headless";

/// The log lines worth quoting: the last few that are not blank.
fn tail(log: &[String]) -> String {
    let lines: Vec<&str> = log
        .iter()
        .map(|l| l.trim_end())
        .filter(|l| !l.trim().is_empty())
        .collect();
    lines[lines.len().saturating_sub(6)..].join("\n")
}

pub fn outcome(name: &str, r: &PluginRunReply) -> Outcome {
    let quoted = tail(&r.log);
    let with_log = |head: String| {
        if quoted.is_empty() {
            head
        } else {
            format!("{head}\n\n{quoted}")
        }
    };
    if !r.loadable {
        return Outcome {
            ok: false,
            short: format!("{name}: IDA did not load the plugin"),
            detail: Some(with_log(format!(
                "IDA did not load {name}. A plugin can decline to work with this database, or its module may need a part of IDA that is not present here."
            ))),
        };
    }
    if r.needs_gui {
        return Outcome {
            ok: false,
            short: format!("{name}: {NEEDS_UI}"),
            detail: Some(with_log(format!("{name}: {NEEDS_UI}."))),
        };
    }
    if r.failed {
        return Outcome {
            ok: false,
            short: format!("{name} failed (see Output)"),
            detail: Some(with_log(format!("{name} raised an error while running."))),
        };
    }
    let short = match (r.changed, r.log.iter().any(|l| !l.trim().is_empty())) {
        (true, _) => format!("{name} ran and changed the database"),
        (false, true) => format!("{name} ran (output in the Output panel)"),
        (false, false) => format!(
            "{name} ran; it printed nothing and changed nothing. Plugins that show windows need IDA's graphical interface"
        ),
    };
    Outcome {
        ok: true,
        short,
        detail: None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn reply() -> PluginRunReply {
        PluginRunReply {
            loadable: true,
            ..Default::default()
        }
    }

    // Failure modes: nothing to bind, a bare key, a modifier alone, two keys,
    // and spellings IDA uses that gpui does not.
    #[test]
    fn hotkeys_become_keystrokes_or_nothing() {
        assert_eq!(
            hotkey_to_keystroke("Ctrl-Alt-M").as_deref(),
            Some("ctrl-alt-m")
        );
        assert_eq!(
            hotkey_to_keystroke("Ctrl+Shift+F11").as_deref(),
            Some("ctrl-shift-f11")
        );
        assert_eq!(hotkey_to_keystroke("Alt-F11").as_deref(), Some("alt-f11"));
        assert_eq!(hotkey_to_keystroke("Ctrl--").as_deref(), Some("ctrl--"));
        assert_eq!(hotkey_to_keystroke(""), None);
        assert_eq!(hotkey_to_keystroke("Ctrl-Alt"), None);
        assert_eq!(hotkey_to_keystroke("Q"), None);
        assert_eq!(hotkey_to_keystroke("Ctrl-A-B"), None);
    }

    #[test]
    fn the_same_file_and_argument_is_listed_once() {
        let p = |path: &str, arg: u64, name: &str| PluginInfo {
            path: path.into(),
            arg,
            name: name.into(),
            ..Default::default()
        };
        let list = dedupe(vec![
            p("C:\\a\\x.dll", 0, "B"),
            p("c:\\a\\X.dll", 0, "B"),
            p("C:\\a\\x.dll", 1, "A"),
        ]);
        assert_eq!(
            list.iter().map(|p| p.name.as_str()).collect::<Vec<_>>(),
            ["A", "B"]
        );
    }

    #[test]
    fn hidden_and_debugger_plugins_stay_out_of_the_menu() {
        let flags = |f: i32| PluginInfo {
            flags: f,
            ..Default::default()
        };
        assert!(runnable(&flags(plugin_flags::MULTI)));
        assert!(!runnable(&flags(plugin_flags::HIDE | plugin_flags::MULTI)));
        assert!(!runnable(&flags(plugin_flags::DBG)));
    }

    #[test]
    fn a_plugin_that_needs_qt_is_never_reported_as_success() {
        let mut r = reply();
        r.failed = true;
        r.needs_gui = true;
        r.log = vec!["ModuleNotFoundError: No module named 'PyQt5.sip'".into()];
        let o = outcome("X", &r);
        assert!(!o.ok);
        assert!(o.short.contains("graphical interface"));
        assert!(o.detail.unwrap().contains("PyQt5"));
    }

    #[test]
    fn a_plugin_that_did_nothing_says_so() {
        let o = outcome("X", &reply());
        assert!(o.ok && o.short.contains("printed nothing"));
        let mut changed = reply();
        changed.changed = true;
        assert!(
            outcome("X", &changed)
                .short
                .contains("changed the database")
        );
        let mut declined = reply();
        declined.loadable = false;
        assert!(!outcome("X", &declined).ok);
    }
}
