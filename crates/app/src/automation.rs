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
//   waitready            holds the commands behind it until a database is open with its lists loaded
//   marker <name>        appends <name> to <file>.done
//   clipboard <text>     seeds the in-process clipboard (the system one is never used)
//   (open_url: Help > Documentation and Report an issue append `url: <url>` to <file>.done instead of opening a browser)
//   clipdump             appends the in-process clipboard to <file>.done as `clipboard: <text>` (newlines as \\n)
//   rightclick <x> <y>   a right click; shiftclick <x> <y> a left click with Shift held
//   down|heldmove|up <x> <y>  the button held across steps
//   drag <x1> <y1> <x2> <y2>  press, move with the button held, release
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
                // `waitready`: the commands behind it wait until the database is open and its
                // lists have arrived (a fixed wait is too short on a busy machine).
                if line.trim() == "waitready" {
                    let ready = cx
                        .update_window(handle, |_, window, cx| {
                            window
                                .root::<crate::workspace::Workspace>()
                                .flatten()
                                .is_some_and(|w| w.read(cx).is_loaded(cx))
                        })
                        .unwrap_or(false);
                    if !ready {
                        pending.push(line);
                        continue;
                    }
                    cx.background_executor()
                        .timer(Duration::from_millis(1500))
                        .await;
                    continue;
                }
                let done_path = path.clone();
                // `@<n> <command>` targets a floating window (1-based, in opening order).
                let (target, line) = match line.strip_prefix('@').and_then(|l| l.split_once(' ')) {
                    Some((n, rest)) if n.parse::<usize>().is_ok() => {
                        let n: usize = n.parse().unwrap_or(0);
                        let win = if n == 0 {
                            Some(handle)
                        } else {
                            cx.update(|cx| {
                                cx.try_global::<crate::workspace::FloatingWindows>()
                                    .and_then(|f| f.0.get(n - 1).copied())
                            })
                        };
                        (win, rest.to_owned())
                    }
                    _ => (Some(handle), line),
                };
                let ok = match target {
                    Some(h) => cx
                        .update_window(h, |_, window, cx| {
                            crate::workspace::mark_input(window, cx);
                            run(&line, &done_path, window, cx)
                        })
                        .unwrap_or(false),
                    None => false,
                };
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
    // An action is dispatched from the focused element as found in the last *painted*
    // frame (`Window::dispatch_action`). A window that is covered or throttled may not
    // have painted since the focus moved (a prompt closed, a dialog opened), and the
    // action would then reach no handler. Paint first, so every command lands where a
    // user's would.
    if matches!(cmd, "key" | "type" | "action") {
        window.draw(cx).clear();
    }
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
        "click" | "dblclick" | "shiftclick" | "rightclick" => xy().is_some_and(|position| {
            let modifiers = Modifiers {
                shift: cmd == "shiftclick",
                ..Default::default()
            };
            let button = if cmd == "rightclick" {
                MouseButton::Right
            } else {
                MouseButton::Left
            };
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
                        button,
                        position,
                        modifiers,
                        click_count,
                        first_mouse: false,
                    }),
                    cx,
                );
                window.dispatch_event(
                    PlatformInput::MouseUp(MouseUpEvent {
                        button,
                        position,
                        modifiers,
                        click_count,
                    }),
                    cx,
                );
            }
            true
        }),
        // The button held across steps: `down x y`, `heldmove x y`, `up x y`.
        "down" | "heldmove" | "up" => xy().is_some_and(|position| {
            let modifiers = Modifiers::default();
            let event = match cmd {
                "down" => PlatformInput::MouseDown(MouseDownEvent {
                    button: MouseButton::Left,
                    position,
                    modifiers,
                    click_count: 1,
                    first_mouse: false,
                }),
                "heldmove" => PlatformInput::MouseMove(MouseMoveEvent {
                    position,
                    pressed_button: Some(MouseButton::Left),
                    modifiers,
                }),
                _ => PlatformInput::MouseUp(MouseUpEvent {
                    button: MouseButton::Left,
                    position,
                    modifiers,
                    click_count: 1,
                }),
            };
            window.dispatch_event(event, cx);
            true
        }),
        // Press at (x1, y1), move to (x2, y2) with the button held, release.
        "drag" => {
            let n: Vec<f32> = arg
                .split_whitespace()
                .filter_map(|v| v.parse().ok())
                .collect();
            let [x1, y1, x2, y2] = n[..] else {
                return false;
            };
            let (from, to) = (point(px(x1), px(y1)), point(px(x2), px(y2)));
            let modifiers = Modifiers::default();
            let moved = |position, pressed_button| {
                PlatformInput::MouseMove(MouseMoveEvent {
                    position,
                    pressed_button,
                    modifiers,
                })
            };
            window.dispatch_event(moved(from, None), cx);
            window.dispatch_event(
                PlatformInput::MouseDown(MouseDownEvent {
                    button: MouseButton::Left,
                    position: from,
                    modifiers,
                    click_count: 1,
                    first_mouse: false,
                }),
                cx,
            );
            for step in 1..=4 {
                let t = step as f32 / 4.;
                let at = point(from.x + (to.x - from.x) * t, from.y + (to.y - from.y) * t);
                window.dispatch_event(moved(at, Some(MouseButton::Left)), cx);
            }
            window.dispatch_event(
                PlatformInput::MouseUp(MouseUpEvent {
                    button: MouseButton::Left,
                    position: to,
                    modifiers,
                    click_count: 1,
                }),
                cx,
            );
            true
        }
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
            // One line: newlines are written as `\n` (and `\r`) so a
            // multi-line copy can be compared exactly by the capture script.
            let text = text.replace('\r', "\\r").replace('\n', "\\n");
            append_done(channel, &format!("clipboard: {text}"))
        }
        "marker" => append_done(channel, arg.trim()),
        _ => false,
    }
}

/// Opens `url` in the default browser. Under automation nothing is opened:
/// the URL goes to `<file>.done` as `url: <url>` so a test can check it.
pub fn open_url(url: &str, cx: &mut App) {
    match channel_path() {
        Some(channel) => {
            append_done(&channel, &format!("url: {url}"));
        }
        None => cx.open_url(url),
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
