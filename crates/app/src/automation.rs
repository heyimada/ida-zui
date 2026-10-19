// In-app automation channel for UI captures and tests. When IDA_ZUI_AUTOMATION
// names a file, the app executes the commands appended to it, one per tick, on
// the UI thread. Input goes through gpui's own event dispatch, so nothing is
// ever sent to the OS and no other window can receive it.
//
//   key <keystroke>      e.g. ctrl-shift-p, n, enter, escape, alt
//   type <text>          typed into the focused input, character by character
//   action <ns::Name>    e.g. ida::Rename (runs along the focus path)
//   move <x> <y>         mouse move, logical window pixels
//   click <x> <y>        move, press and release the left button
//   dblclick <x> <y>     a click followed by the second click of a double click
//   confirm <n>          answer button n (1-based) of an in-app question
//   drop <path>[|<path>..]  files dropped on the window, through gpui's file-drop events (what an OS drop becomes)
//   marker <name>        appends <name> to <file>.done
//   clipboard <text>     seeds the in-process clipboard (the system one is never used)
//   clipdump             appends the in-process clipboard to <file>.done as `clipboard: <text>`
use gpui::{
    AnyWindowHandle, App, AppContext, ExternalPaths, FileDropEvent, Keystroke, Modifiers,
    ModifiersChangedEvent, MouseButton, MouseDownEvent, MouseMoveEvent, MouseUpEvent,
    PlatformInput, Window, point, px,
};
use std::io::Write;
use std::path::PathBuf;
use std::time::Duration;

const POLL: Duration = Duration::from_millis(100);
/// Time for a command's effects to be laid out and painted.
const SETTLE: Duration = Duration::from_millis(120);

pub fn channel_path() -> Option<PathBuf> {
    std::env::var_os("IDA_ZUI_AUTOMATION").map(PathBuf::from)
}

/// Starts polling `path` and executing commands in `handle`'s window.
pub fn start(path: PathBuf, handle: AnyWindowHandle, cx: &mut App) {
    cx.spawn(async move |cx| {
        let mut offset = 0usize;
        let mut pending: Vec<String> = Vec::new();
        loop {
            cx.background_executor().timer(POLL).await;
            if pending.is_empty() {
                let Ok(bytes) = std::fs::read(&path) else {
                    continue;
                };
                if bytes.len() < offset {
                    offset = 0; // truncated: start over
                }
                // Only complete lines; a half-written line waits for its newline.
                let end = bytes[offset..]
                    .iter()
                    .rposition(|&b| b == b'\n')
                    .map(|i| offset + i + 1);
                if let Some(end) = end {
                    let text = String::from_utf8_lossy(&bytes[offset..end]).into_owned();
                    offset = end;
                    pending = text
                        .lines()
                        .map(|l| l.trim_end_matches('\r').to_owned())
                        .filter(|l| !l.trim().is_empty())
                        .rev()
                        .collect();
                }
            }
            if let Some(line) = pending.pop() {
                let done_path = path.clone();
                let ok = cx
                    .update_window(handle, |_, window, cx| run(&line, &done_path, window, cx))
                    .unwrap_or(false);
                if !ok {
                    eprintln!("automation: could not run '{line}'");
                }
                cx.background_executor().timer(SETTLE).await;
            }
        }
    })
    .detach();
}

fn run(line: &str, channel: &std::path::Path, window: &mut Window, cx: &mut App) -> bool {
    let (cmd, arg) = line.split_once(' ').unwrap_or((line, ""));
    let xy = || {
        let (x, y) = arg.split_once(' ')?;
        Some(point(
            px(x.trim().parse().ok()?),
            px(y.trim().parse().ok()?),
        ))
    };
    match cmd {
        "key" => match Keystroke::parse(arg.trim()) {
            Ok(k) => {
                window.dispatch_keystroke(k, cx);
                true
            }
            Err(_) if arg.trim() == "alt" => {
                // Bare Alt: pressed, then released with no key in between.
                for alt in [true, false] {
                    window.dispatch_event(
                        PlatformInput::ModifiersChanged(ModifiersChangedEvent {
                            modifiers: Modifiers {
                                alt,
                                ..Default::default()
                            },
                            capslock: Default::default(),
                        }),
                        cx,
                    );
                }
                true
            }
            Err(_) => false,
        },
        "type" => {
            for ch in arg.chars() {
                let key = if ch == ' ' {
                    "space".to_owned()
                } else {
                    ch.to_string()
                };
                window.dispatch_keystroke(
                    Keystroke {
                        modifiers: Modifiers::default(),
                        key,
                        key_char: Some(ch.to_string()),
                    },
                    cx,
                );
            }
            true
        }
        "action" => match cx.build_action(arg.trim(), None) {
            Ok(action) => {
                window.dispatch_action(action, cx);
                true
            }
            Err(e) => {
                eprintln!("automation: {e}");
                false
            }
        },
        "move" => xy().is_some_and(|position| {
            window.dispatch_event(
                PlatformInput::MouseMove(MouseMoveEvent {
                    position,
                    pressed_button: None,
                    modifiers: Modifiers::default(),
                }),
                cx,
            );
            true
        }),
        "click" | "dblclick" => xy().is_some_and(|position| {
            let modifiers = Modifiers::default();
            window.dispatch_event(
                PlatformInput::MouseMove(MouseMoveEvent {
                    position,
                    pressed_button: None,
                    modifiers,
                }),
                cx,
            );
            let clicks = if cmd == "dblclick" { 2 } else { 1 };
            for click_count in 1..=clicks {
                window.dispatch_event(
                    PlatformInput::MouseDown(MouseDownEvent {
                        button: MouseButton::Left,
                        position,
                        modifiers,
                        click_count,
                        first_mouse: false,
                    }),
                    cx,
                );
                window.dispatch_event(
                    PlatformInput::MouseUp(MouseUpEvent {
                        button: MouseButton::Left,
                        position,
                        modifiers,
                        click_count,
                    }),
                    cx,
                );
            }
            true
        }),
        "drop" => {
            let paths = ExternalPaths(
                arg.split('|')
                    .map(|p| PathBuf::from(p.trim()))
                    .filter(|p| !p.as_os_str().is_empty())
                    .collect(),
            );
            if paths.paths().is_empty() {
                return false;
            }
            let position = point(px(400.), px(300.));
            for ev in [
                FileDropEvent::Entered { position, paths },
                FileDropEvent::Submit { position },
                FileDropEvent::Exited,
            ] {
                window.dispatch_event(PlatformInput::FileDrop(ev), cx);
            }
            true
        }
        "confirm" => match arg.trim().parse::<u8>() {
            Ok(n) if (1..=9).contains(&n) => {
                window.dispatch_keystroke(Keystroke::parse(&n.to_string()).unwrap(), cx);
                true
            }
            _ => false,
        },
        "clipboard" => {
            zui_editor::clipboard::write(arg.to_owned(), cx);
            true
        }
        "clipdump" => {
            let text = zui_editor::clipboard::read(cx).unwrap_or_default();
            append_done(channel, &format!("clipboard: {text}"))
        }
        "marker" => append_done(channel, arg.trim()),
        _ => false,
    }
}

fn append_done(channel: &std::path::Path, line: &str) -> bool {
    let mut done = channel.as_os_str().to_owned();
    done.push(".done");
    std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(done)
        .and_then(|mut f| writeln!(f, "{line}"))
        .is_ok()
}
