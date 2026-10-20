#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]
// Painting and layout helpers pass many small values and tuples around.
#![allow(clippy::type_complexity, clippy::too_many_arguments)]
mod actions;
mod addr_map;
mod assets;
mod automation;
mod bottom;
mod code;
mod commands;
mod context_menu;
mod database;
mod debugger;
mod decl_names;
mod dialog;
mod docs;
mod edit_dialogs;
mod enum_view;
mod fields;
mod focus;
mod frame_view;
mod graph;
mod graph_layout;
mod graph_paint;
mod help;
mod hex;
mod input;
mod inspector;
mod jumps;
mod list_table;
mod listing;
mod loadlog;
mod lumina;
mod menubar;
mod modal;
mod nav_history;
mod open_dialog;
mod operands;
mod panes;
mod plugins;
mod plugins_view;
mod proximity_view;
mod pseudo;
mod py_console;
mod py_history;
mod script_dialog;
mod scrollbar;
mod selection;
mod session;
mod settings;
mod settings_view;
mod sidebar;
mod sigs_view;
mod status;
mod struct_view;
mod table;
mod theme;
mod types_view;
mod ui;
mod workspace;

use gpui::{App, AppContext, Bounds, TitlebarOptions, WindowBounds, WindowOptions, px, size};
use std::path::PathBuf;

/// `ida-zui [FILE...] [--goto EXPR]`: every file opens in a tab of its own.
struct Args {
    files: Vec<PathBuf>,
    goto: Option<String>,
}

fn parse_args() -> Args {
    let mut args = std::env::args().skip(1);
    let mut out = Args {
        files: Vec::new(),
        goto: None,
    };
    while let Some(a) = args.next() {
        match a.as_str() {
            "--goto" => out.goto = args.next(),
            _ => out.files.push(PathBuf::from(a)),
        }
    }
    out
}

fn main() {
    let args = parse_args();
    gpui_platform::application()
        .with_assets(assets::Assets)
        .run(move |cx: &mut App| {
            assets::load_fonts(cx);
            // Theme, fonts and the keymap all come from settings.json.
            settings::init(cx);
            // Under automation the window must never take focus from the user.
            let automation = automation::channel_path();
            let bounds = Bounds::centered(None, size(px(1480.), px(920.)), cx);
            let files = args.files.clone();
            let goto = args.goto.clone();
            let result = cx.open_window(
                WindowOptions {
                    window_bounds: Some(WindowBounds::Windowed(bounds)),
                    window_min_size: Some(size(px(900.), px(560.))),
                    focus: automation.is_none(),
                    // The title strip draws its own caption and window buttons.
                    titlebar: Some(TitlebarOptions {
                        title: Some("ida-zui".into()),
                        appears_transparent: true,
                        ..Default::default()
                    }),
                    ..Default::default()
                },
                move |window, cx| {
                    let ws = cx.new(|cx| workspace::Workspace::new(window, cx));
                    let guard = ws.downgrade();
                    window.on_window_should_close(cx, move |window, cx| {
                        guard
                            .update(cx, |w, cx| w.should_close(window, cx))
                            .unwrap_or(true)
                    });
                    ws.update(cx, |w, cx| {
                        window.focus(&gpui::Focusable::focus_handle(w, cx), cx);
                        w.open_files(files, goto, window, cx);
                    });
                    ws
                },
            );
            let main_id = result.as_ref().ok().map(|h| h.window_id());
            match (result, automation) {
                (Ok(handle), Some(channel)) => automation::start(channel, handle.into(), cx),
                (Err(e), _) => {
                    eprintln!("Could not open the window: {e}");
                    cx.quit();
                }
                _ => {}
            }
            // The app ends with its main window (floating windows only dock back).
            cx.on_window_closed(move |cx, _| {
                if main_id.is_none_or(|id| !cx.windows().iter().any(|w| w.window_id() == id)) {
                    cx.quit();
                }
            })
            .detach();
            if automation::channel_path().is_none() {
                cx.activate(true);
            }
        });
}
