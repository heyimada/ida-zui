// The debugger's workspace glue: showing its panels, the attach picker and
// the reaction to a stop.
use super::{ModalKind, ModalStyle, Workspace};
use crate::bottom::BottomTab;
use crate::modal::{Choice, ChoiceValue};
use gpui::{AppContext, Context, SharedString, Window};
use ida_proto::DbgProcesses;
use std::rc::Rc;

impl Workspace {
    /// Opens the bottom panel on a debugger panel.
    pub(super) fn show_debug_tab(&mut self, tab: BottomTab, cx: &mut Context<Self>) {
        self.show_bottom = true;
        self.bottom.update(cx, |b, cx| {
            b.tab = tab;
            cx.notify();
        });
        self.save_layout(cx);
        cx.notify();
    }

    /// A stop brings the registers forward the first time, so the state of
    /// the process is on screen; a panel the user chose stays.
    pub(super) fn on_debug_stopped(&mut self, ix: usize, cx: &mut Context<Self>) {
        if ix != self.active {
            return;
        }
        // Room for the registers.
        if self.bottom_h < 230. {
            self.bottom_h = 230.;
        }
        let tab = self.bottom.read(cx).tab;
        if !tab.is_debug() {
            self.show_debug_tab(BottomTab::Registers, cx);
        } else if !self.show_bottom {
            self.show_bottom = true;
            cx.notify();
        }
    }

    /// A picker over the running processes.
    pub(super) fn attach_picker(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if !self.session.read(cx).is_ready() || self.session.read(cx).dbg.active() {
            return;
        }
        let fut = self.session.read(cx).call(DbgProcesses {});
        let handle = window.window_handle();
        let db = self.id;
        cx.spawn(async move |this, cx| {
            let result = fut.await;
            cx.update_window(handle, |_, window, cx| {
                this.update(cx, |w, cx| match result {
                    _ if w.id != db => {}
                    Ok(list) => {
                        let choices: Vec<Choice> = list
                            .into_iter()
                            .map(|p| Choice {
                                value: ChoiceValue::Ea(u64::from(p.pid)),
                                label: p.name.into(),
                                detail: format!("pid {}", p.pid).into(),
                                kind: "proc".into(),
                                group: SharedString::default(),
                                keywords: SharedString::default(),
                            })
                            .collect();
                        w.open_modal(
                            "Attach to process",
                            "",
                            "Filter by name",
                            Rc::new(choices),
                            false,
                            ModalKind::Attach,
                            ModalStyle::Palette {
                                enter: "attach",
                                key_hint: None,
                            },
                            window,
                            cx,
                        );
                    }
                    Err(e) => w.session.update(cx, |s, cx| s.error(e.to_string(), cx)),
                })
                .ok();
            })
            .ok();
        })
        .detach();
    }
}
