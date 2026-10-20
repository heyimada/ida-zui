// A code view torn off into an OS window of its own (View > Move to new
// window, or a tab dragged out of the strip). The window shares the
// database's session with the main window, so it follows navigation like any
// other view. Closing it returns the view to the tab strip.
use super::Workspace;
use crate::actions::*;
use crate::database::Floating;
use crate::docs::{CodeView, DocKey, DocView};
use crate::menubar::{MenuBar, MenuEvent, Target};
use crate::modal::{Choice, ChoiceValue, Modal, ModalEvent, Style as ModalStyle};
use crate::session::{Driver, Session, SessionEvent, SubmitFn};
use crate::theme::{Theme, UI_FONT, hsla, hsla_a};
use gpui::{
    AnyWindowHandle, App, AppContext, Bounds, Context, Entity, FocusHandle, Focusable, Global,
    ParentElement, Render, SharedString, Styled, Subscription, WeakEntity, Window, WindowBounds,
    WindowOptions, div, prelude::*, px, size,
};
use std::rc::Rc;

/// The floating windows, in the order they were opened (what the automation
/// channel's `@<n>` prefix counts, 1-based; 0 is the main window).
#[derive(Default)]
pub struct FloatingWindows(pub Vec<AnyWindowHandle>);

impl Global for FloatingWindows {}

/// The window that last received a key or a click: a prompt opens there.
#[derive(Default)]
pub struct InputWindow(pub Option<gpui::WindowId>);

impl Global for InputWindow {}

/// True when the last input went to a floating window, so the main window
/// leaves prompts to it.
pub fn input_is_floating(cx: &App) -> bool {
    let Some(id) = cx.try_global::<InputWindow>().and_then(|w| w.0) else {
        return false;
    };
    cx.try_global::<FloatingWindows>()
        .is_some_and(|f| f.0.iter().any(|w| w.window_id() == id))
}

pub fn mark_input(window: &Window, cx: &mut App) {
    let id = window.window_handle().window_id();
    if cx.try_global::<InputWindow>().and_then(|w| w.0) != Some(id) {
        cx.set_global(InputWindow(Some(id)));
    }
}

/// What a modal in a floating window does with its answer.
pub enum ModalKind {
    Prompt(SubmitFn),
    Goto,
    /// A list whose rows act on their own (xrefs, structure choosers).
    Pick,
}

/// Something the workspace opened for a floating window: shown there, not in the main window.
pub enum Overlay {
    Modal {
        view: Entity<Modal>,
        kind: ModalKind,
        return_focus: Option<FocusHandle>,
    },
    Dialog {
        view: gpui::AnyView,
        focus: FocusHandle,
        return_focus: Option<FocusHandle>,
        sub: Subscription,
    },
    CloseDialog,
}

#[derive(Default)]
pub struct Inbox(Vec<(gpui::WindowId, Overlay)>);

impl Global for Inbox {}

/// True when `window` is one of the floating windows.
pub fn is_floating(window: &Window, cx: &App) -> bool {
    let id = window.window_handle().window_id();
    cx.try_global::<FloatingWindows>()
        .is_some_and(|f| f.0.iter().any(|w| w.window_id() == id))
}

/// The floating window that last had input, if any.
pub fn input_floating(cx: &App) -> Option<AnyWindowHandle> {
    let id = cx.try_global::<InputWindow>().and_then(|w| w.0)?;
    cx.try_global::<FloatingWindows>()?
        .0
        .iter()
        .find(|w| w.window_id() == id)
        .copied()
}

/// Hands `overlay` to the floating window `window` (it picks it up when it renders).
pub fn push_overlay(window: &mut Window, overlay: Overlay, cx: &mut App) {
    let id = window.window_handle().window_id();
    cx.default_global::<Inbox>().0.push((id, overlay));
    window.refresh();
}

/// A dialog shown in a floating window closed itself.
pub fn dialog_closed(handle: AnyWindowHandle, cx: &mut App) {
    cx.default_global::<Inbox>()
        .0
        .push((handle.window_id(), Overlay::CloseDialog));
    // Fails while the window is being updated; it renders afterwards anyway.
    let _ = handle.update(cx, |_, window, _| window.refresh());
}

struct ActiveDialog {
    view: gpui::AnyView,
    return_focus: Option<FocusHandle>,
    _sub: Subscription,
}

struct OpenModal {
    view: Entity<Modal>,
    kind: ModalKind,
    return_focus: Option<FocusHandle>,
    _sub: Subscription,
}

pub struct FloatingWindow {
    session: Entity<Session>,
    view: CodeView,
    ws: WeakEntity<Workspace>,
    main: AnyWindowHandle,
    tab: u64,
    modal: Option<OpenModal>,
    dialog: Option<ActiveDialog>,
    /// Context menus only; its commands run in this window.
    menubar: Entity<MenuBar>,
    focus: FocusHandle,
    _subs: Vec<Subscription>,
}

impl Workspace {
    /// Takes the tab out of the strip into a window of its own.
    pub(super) fn tear_off(&mut self, id: u64, window: &mut Window, cx: &mut Context<Self>) {
        self.tear_off_at(id, None, window, cx)
    }

    /// As `tear_off`, at saved bounds.
    pub(super) fn tear_off_at(
        &mut self,
        id: u64,
        bounds: Option<Bounds<gpui::Pixels>>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(at) = self.docs.iter().position(|d| d.id == id) else {
            return;
        };
        if !matches!(self.docs[at].view, DocView::Code(_)) {
            return;
        }
        let Some(tab) = self.remove_doc(id, window, cx) else {
            return;
        };
        if let DocView::Code(view) = tab.view {
            self.open_floating(id, view, bounds, window, cx);
        }
    }

    /// View > Move to new window: the tab in front, or, for the main view, a
    /// second view of the same kind at the cursor.
    pub(super) fn move_to_new_window(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if !self.session.read(cx).is_ready() {
            return;
        }
        match self.focused_front() {
            Some(id) => self.tear_off(id, window, cx),
            None => {
                let kind = self
                    .active_kind()
                    .unwrap_or(crate::session::ViewKind::Listing);
                let ea = self.session.read(cx).cursor;
                let session = self.session.clone();
                let view = CodeView::open(kind, &session, ea, false, cx);
                let id = self.next_doc;
                self.next_doc += 1;
                self.open_floating(id, view, None, window, cx);
            }
        }
    }

    fn open_floating(
        &mut self,
        id: u64,
        view: CodeView,
        bounds: Option<Bounds<gpui::Pixels>>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let session = self.session.clone();
        let ws = cx.weak_entity();
        let main = window.window_handle();
        let at = window.bounds();
        let bounds = bounds.unwrap_or_else(|| {
            Bounds::new(
                at.origin + gpui::point(px(60.), px(60.)),
                size(px(900.), px(700.)),
            )
        });
        let automation = crate::automation::channel_path().is_some();
        let shown = view.clone();
        let opened = cx.open_window(
            WindowOptions {
                window_bounds: Some(WindowBounds::Windowed(bounds)),
                window_min_size: Some(size(px(360.), px(240.))),
                focus: !automation,
                titlebar: Some(gpui::TitlebarOptions {
                    title: Some("ida-zui".into()),
                    ..Default::default()
                }),
                ..Default::default()
            },
            move |window, cx| {
                let fw = cx.new(|cx| FloatingWindow::new(session, shown, ws, main, id, window, cx));
                let guard = fw.downgrade();
                window.on_window_should_close(cx, move |window, cx| {
                    guard.update(cx, |f, cx| f.dock(window, cx)).unwrap_or(true)
                });
                fw
            },
        );
        match opened {
            Ok(handle) => {
                let handle: AnyWindowHandle = handle.into();
                if !cx.has_global::<FloatingWindows>() {
                    cx.set_global(FloatingWindows::default());
                }
                cx.global_mut::<FloatingWindows>().0.push(handle);
                self.floating.push(Floating {
                    id,
                    view,
                    window: handle,
                });
                self.sync_views(cx);
                self.tabs_changed(cx);
                cx.notify();
            }
            Err(e) => {
                eprintln!("Could not open the window: {e}");
                // Back into the strip.
                self.dock_view(id, view, window, cx);
            }
        }
    }

    /// A floating window closed: its view becomes a tab again.
    pub(super) fn dock_floating(&mut self, id: u64, window: &mut Window, cx: &mut Context<Self>) {
        let Some(at) = self.floating.iter().position(|f| f.id == id) else {
            return;
        };
        let f = self.floating.remove(at);
        if cx.has_global::<FloatingWindows>() {
            cx.global_mut::<FloatingWindows>()
                .0
                .retain(|w| *w != f.window);
        }
        self.dock_view(id, f.view, window, cx);
    }

    fn dock_view(&mut self, id: u64, view: CodeView, window: &mut Window, cx: &mut Context<Self>) {
        let sub = cx.observe(&self.session.clone(), |_, _, _| {});
        self.docs.push(crate::database::DocTab {
            id,
            key: DocKey::Code(id),
            title: SharedString::default(),
            view: DocView::Code(view),
            _sub: sub,
        });
        self.show_doc(id, window, cx);
        self.tabs_changed(cx);
    }

    /// Closes every floating window of the database (it was closed).
    pub(super) fn close_floating(&mut self, ix: usize, cx: &mut Context<Self>) {
        let list = std::mem::take(&mut self.dbs[ix].floating);
        for f in list {
            if cx.has_global::<FloatingWindows>() {
                cx.global_mut::<FloatingWindows>()
                    .0
                    .retain(|w| *w != f.window);
            }
            let _ = f.window.update(cx, |_, window, _| window.remove_window());
        }
    }
}

impl FloatingWindow {
    #[allow(clippy::too_many_arguments)]
    fn new(
        session: Entity<Session>,
        view: CodeView,
        ws: WeakEntity<Workspace>,
        main: AnyWindowHandle,
        tab: u64,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let mut subs = vec![
            cx.observe(&session, |_, _, cx| cx.notify()),
            cx.subscribe_in(
                &session,
                window,
                |this, _, ev: &SessionEvent, window, cx| {
                    if let SessionEvent::ContextMenu(req) = ev
                        && input_is_this(window, cx)
                    {
                        let ops = this.session.read(cx).ops();
                        let (pos, title, entries, keyboard) = (
                            req.pos,
                            req.title.clone(),
                            req.entries.clone(),
                            req.keyboard,
                        );
                        this.menubar.update(cx, |m, cx| {
                            m.ops = ops;
                            m.open_popup(pos, title, entries, keyboard, window, cx)
                        });
                    }
                    if let SessionEvent::Prompt(req) = ev
                        && crate::workspace::floating::input_is_this(window, cx)
                    {
                        let req = req.clone();
                        this.open_modal(
                            req.title.clone(),
                            &req.initial,
                            &req.placeholder,
                            Rc::default(),
                            true,
                            ModalKind::Prompt(req.on_submit.clone()),
                            ModalStyle::Prompt {
                                confirm: req.confirm.clone(),
                                validate_name: req.name,
                            },
                            window,
                            cx,
                        );
                    }
                },
            ),
        ];
        // Being activated makes this view the one the user drives.
        subs.push(cx.observe_window_activation(window, |this, window, cx| {
            if window.is_window_active() {
                this.take_input(window, cx);
            }
        }));
        let menubar = cx.new(|cx| {
            let mut m = MenuBar::new(window, cx);
            m.popup_only = true;
            m
        });
        subs.push(cx.subscribe_in(
            &menubar,
            window,
            |_, _, ev: &MenuEvent, window, cx| match ev {
                MenuEvent::Invoke(Target::Action(action)) => {
                    window.dispatch_action(action.boxed_clone(), cx)
                }
                MenuEvent::Invoke(Target::Run(f)) => f(window, cx),
                MenuEvent::Invoke(_) => {}
            },
        ));
        let focus = cx.focus_handle();
        let this = Self {
            session,
            view,
            ws,
            main,
            tab,
            modal: None,
            dialog: None,
            menubar,
            focus,
            _subs: subs,
        };
        this.set_shown(cx);
        window.focus(&this.view_focus(cx), cx);
        this
    }

    fn view_focus(&self, cx: &App) -> FocusHandle {
        DocView::Code(self.view.clone()).focus_handle(cx)
    }

    fn set_shown(&self, cx: &mut App) {
        self.view.set_visible(true, cx);
    }

    /// This window's view drives the session from now on.
    fn take_input(&mut self, window: &Window, cx: &mut Context<Self>) {
        mark_input(window, cx);
        let driver = Driver {
            origin: self.view.origin(cx),
            pinned: self.view.pinned(cx),
        };
        let ea = self
            .view
            .current_ea(cx)
            .map(|ea| (ea, self.view.kind() == crate::session::ViewKind::Pseudocode));
        self.session.update(cx, |s, cx| {
            s.driver = Some(driver);
            if let Some((ea, _)) = ea {
                s.cursor = ea;
                cx.notify();
            }
        });
    }

    /// The window closes: the view goes back to the tab strip.
    fn dock(&mut self, window: &mut Window, cx: &mut Context<Self>) -> bool {
        let (ws, tab) = (self.ws.clone(), self.tab);
        let main = self.main;
        // The main window may be gone already (the app is quitting).
        let _ = cx.update_window(main, move |_, mw, cx| {
            let _ = ws.update(cx, |w, cx| w.dock_floating(tab, mw, cx));
        });
        let _ = window;
        true
    }

    #[allow(clippy::too_many_arguments)]
    fn open_modal(
        &mut self,
        title: impl Into<SharedString>,
        initial: &str,
        placeholder: &str,
        choices: Rc<Vec<Choice>>,
        prefer_text: bool,
        kind: ModalKind,
        style: ModalStyle,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let return_focus = window.focused(cx);
        let view = cx.new(|cx| {
            Modal::new(
                title,
                initial,
                placeholder,
                choices,
                prefer_text,
                style,
                window,
                cx,
            )
        });
        let sub = cx.subscribe_in(&view, window, |this, _, ev: &ModalEvent, window, cx| {
            this.on_modal_event(ev, window, cx)
        });
        self.modal = Some(OpenModal {
            view,
            kind,
            return_focus,
            _sub: sub,
        });
        cx.notify();
    }

    fn close_modal(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(m) = self.modal.take() {
            match m.return_focus {
                Some(f) => window.focus(&f, cx),
                None => window.focus(&self.view_focus(cx), cx),
            }
        }
        cx.notify();
    }

    fn on_modal_event(&mut self, ev: &ModalEvent, window: &mut Window, cx: &mut Context<Self>) {
        let Some(open) = self.modal.as_ref() else {
            return;
        };
        match ev {
            ModalEvent::Dismiss => self.close_modal(window, cx),
            ModalEvent::Pick(ChoiceValue::Run(f)) => {
                let f = f.clone();
                self.close_modal(window, cx);
                f(cx);
            }
            ModalEvent::Pick(ChoiceValue::Action(action)) => {
                let action = action.boxed_clone();
                self.close_modal(window, cx);
                window.dispatch_action(action, cx);
            }
            ModalEvent::Pick(ChoiceValue::Ea(ea)) | ModalEvent::Pick(ChoiceValue::Mark(ea, _)) => {
                let ea = *ea;
                self.close_modal(window, cx);
                self.session.update(cx, |s, cx| s.navigate(ea, cx));
            }
            ModalEvent::Submit(text) => {
                let text = text.clone();
                match &open.kind {
                    ModalKind::Prompt(f) => {
                        let f = f.clone();
                        self.close_modal(window, cx);
                        cx.defer(move |cx| f(text, cx));
                    }
                    ModalKind::Goto => {
                        self.close_modal(window, cx);
                        self.session.update(cx, |s, cx| s.goto_expr(text, cx));
                    }
                    ModalKind::Pick => {}
                }
            }
            _ => {}
        }
    }

    /// Picks up what the workspace opened for this window.
    fn take_overlays(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if !cx.has_global::<Inbox>() {
            return;
        }
        let id = window.window_handle().window_id();
        let all = std::mem::take(&mut cx.global_mut::<Inbox>().0);
        let (mine, rest): (Vec<_>, Vec<_>) = all.into_iter().partition(|(w, _)| *w == id);
        cx.global_mut::<Inbox>().0 = rest;
        for (_, overlay) in mine {
            match overlay {
                Overlay::Modal {
                    view,
                    kind,
                    return_focus,
                } => {
                    let sub =
                        cx.subscribe_in(&view, window, |this, _, ev: &ModalEvent, window, cx| {
                            this.on_modal_event(ev, window, cx)
                        });
                    self.modal = Some(OpenModal {
                        view,
                        kind,
                        return_focus,
                        _sub: sub,
                    });
                }
                Overlay::Dialog {
                    view,
                    focus,
                    return_focus,
                    sub,
                } => {
                    window.focus(&focus, cx);
                    self.dialog = Some(ActiveDialog {
                        view,
                        return_focus,
                        _sub: sub,
                    });
                }
                Overlay::CloseDialog => self.close_dialog(window, cx),
            }
        }
    }

    fn close_dialog(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(d) = self.dialog.take() {
            match d.return_focus {
                Some(f) => window.focus(&f, cx),
                None => window.focus(&self.view_focus(cx), cx),
            }
        }
        cx.notify();
    }

    fn goto(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let choices = self
            .ws
            .update(cx, |w, cx| w.goto_choices(cx))
            .unwrap_or_default();
        self.open_modal(
            "Jump to address, name or expression",
            "",
            "Jump to address, name or expression",
            choices,
            false,
            ModalKind::Goto,
            ModalStyle::Palette {
                enter: "jump",
                key_hint: None,
            },
            window,
            cx,
        );
    }

    /// A command that only makes sense in the main window.
    fn unavailable(&self, cx: &mut Context<Self>) {
        self.session.update(cx, |s, cx| {
            s.info_msg("Not available in a floating window", cx)
        });
    }

    /// Runs `f` on the workspace (the operand commands and the like).
    fn forward(
        &self,
        cx: &mut Context<Self>,
        f: impl FnOnce(&mut Workspace, &mut Context<Workspace>),
    ) {
        let _ = self.ws.update(cx, f);
    }

    /// The title: kind and place.
    fn title(&self, cx: &App) -> SharedString {
        let s = self.session.read(cx);
        let at = match self.view.current_ea(cx) {
            Some(ea) => match s.function_at(ea) {
                Some(f) => crate::workspace::chrome::short_name(&f.name),
                None => format!("{ea:X}"),
            },
            None => String::new(),
        };
        let file = s.file_name().unwrap_or_default();
        use super::tabs::ViewKindLabel;
        format!("{}: {at} - {file}", self.view.kind().label()).into()
    }
}

/// True when the last input went to this window.
fn input_is_this(window: &Window, cx: &App) -> bool {
    cx.try_global::<InputWindow>()
        .and_then(|w| w.0)
        .is_some_and(|id| id == window.window_handle().window_id())
}

impl Focusable for FloatingWindow {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}

impl Render for FloatingWindow {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.take_overlays(window, cx);
        let t = Theme::get(cx);
        let title = self.title(cx);
        window.set_window_title(&title);
        let (cursor, status) = {
            let s = self.session.read(cx);
            (
                s.cursor,
                s.status
                    .get(std::time::Instant::now())
                    .map(|(t, _)| t)
                    .unwrap_or_default(),
            )
        };
        let place = self
            .session
            .read(cx)
            .function_at(cursor)
            .map(|f| format!("{}+{:X}", f.name, cursor.saturating_sub(f.start)))
            .unwrap_or_default();
        div()
            .id("floating")
            .key_context("Workspace")
            .track_focus(&self.focus)
            .size_full()
            .relative()
            .flex()
            .flex_col()
            .bg(hsla(t.bg))
            .text_color(hsla(t.text))
            .font_family(UI_FONT)
            .text_size(px(12.))
            .line_height(px(16.))
            .capture_any_mouse_down(cx.listener(|this, _, window, cx| this.take_input(window, cx)))
            .on_key_down(cx.listener(|_, _, window, cx| mark_input(window, cx)))
            .on_action(cx.listener(|this, _: &GoTo, window, cx| this.goto(window, cx)))
            .on_action(
                cx.listener(|this, _: &Back, _, cx| this.session.update(cx, |s, cx| s.back(cx))),
            )
            .on_action(cx.listener(|this, _: &Forward, _, cx| {
                this.session.update(cx, |s, cx| s.forward(cx))
            }))
            .on_action(cx.listener(|this, _: &Undo, _, cx| {
                this.session.update(cx, |s, cx| s.undo(false, cx))
            }))
            .on_action(cx.listener(|this, _: &Redo, _, cx| {
                this.session.update(cx, |s, cx| s.undo(true, cx))
            }))
            .on_action(cx.listener(|this, _: &CloseTab, window, cx| {
                // Closing from the keyboard does not ask the window: dock the view first.
                this.dock(window, cx);
                window.remove_window();
            }))
            .on_action(cx.listener(|this, _: &CopyAddress, _, cx| {
                let ea = this.session.read(cx).cursor;
                zui_editor::clipboard::write(format!("{ea:X}"), cx);
                this.session
                    .update(cx, |s, cx| s.info_msg(format!("Copied {ea:X}"), cx));
            }))
            .on_action(cx.listener(|this, _: &OpHexDec, _, cx| {
                this.forward(cx, |w, cx| w.operand_hex_or_decimal(cx))
            }))
            .on_action(cx.listener(|this, _: &OpHex, _, cx| {
                this.forward(cx, |w, cx| w.set_operand(ida_proto::OpRepr::Hex, cx))
            }))
            .on_action(cx.listener(|this, _: &OpDecimal, _, cx| {
                this.forward(cx, |w, cx| w.set_operand(ida_proto::OpRepr::Decimal, cx))
            }))
            .on_action(cx.listener(|this, _: &OpOctal, _, cx| {
                this.forward(cx, |w, cx| w.set_operand(ida_proto::OpRepr::Octal, cx))
            }))
            .on_action(cx.listener(|this, _: &OpBinary, _, cx| {
                this.forward(cx, |w, cx| w.set_operand(ida_proto::OpRepr::Binary, cx))
            }))
            .on_action(cx.listener(|this, _: &OpChar, _, cx| {
                this.forward(cx, |w, cx| w.set_operand(ida_proto::OpRepr::Char, cx))
            }))
            .on_action(cx.listener(|this, _: &OpClear, _, cx| {
                this.forward(cx, |w, cx| w.set_operand(ida_proto::OpRepr::Clear, cx))
            }))
            .on_action(cx.listener(|this, _: &OpNegate, _, cx| {
                this.forward(cx, |w, cx| w.operand_flip(true, cx))
            }))
            .on_action(cx.listener(|this, _: &OpInvert, _, cx| {
                this.forward(cx, |w, cx| w.operand_flip(false, cx))
            }))
            .on_action(cx.listener(|this, _: &DebugStart, _, cx| {
                this.session.update(cx, |s, cx| s.dbg_start_or_continue(cx))
            }))
            .on_action(cx.listener(|this, _: &DebugPause, _, cx| {
                this.session.update(cx, |s, cx| s.dbg_pause(cx))
            }))
            .on_action(cx.listener(|this, _: &DebugTerminate, _, cx| {
                this.session.update(cx, |s, cx| s.dbg_terminate(cx))
            }))
            .on_action(cx.listener(|this, _: &DebugDetach, _, cx| {
                this.session.update(cx, |s, cx| s.dbg_detach(cx))
            }))
            .on_action(cx.listener(|this, _: &DebugStepInto, _, cx| {
                this.session.update(cx, |s, cx| {
                    s.dbg_resume(ida_proto::ResumeHow::StepInto, 0, cx)
                })
            }))
            .on_action(cx.listener(|this, _: &DebugStepOver, _, cx| {
                this.session.update(cx, |s, cx| {
                    s.dbg_resume(ida_proto::ResumeHow::StepOver, 0, cx)
                })
            }))
            .on_action(cx.listener(|this, _: &DebugStepOut, _, cx| {
                this.session.update(cx, |s, cx| {
                    s.dbg_resume(ida_proto::ResumeHow::StepOut, 0, cx)
                })
            }))
            .on_action(cx.listener(|this, _: &DebugRunToCursor, _, cx| {
                let ea = this.session.read(cx).cursor;
                this.session.update(cx, |s, cx| {
                    s.dbg_resume(ida_proto::ResumeHow::RunTo, ea, cx)
                })
            }))
            .on_action(cx.listener(|this, _: &ToggleBreakpoint, _, cx| {
                let ea = this.session.read(cx).cursor;
                this.session.update(cx, |s, cx| s.dbg_toggle_bpt(ea, cx))
            }))
            .on_action(cx.listener(|this, _: &MarkPosition, _, cx| {
                this.forward(cx, |w, cx| w.mark_position(cx))
            }))
            .on_action(cx.listener(|this, _: &JumpToMark, window, cx| {
                this.forward(cx, |w, cx| w.jump_to_mark(window, cx))
            }))
            .on_action(cx.listener(|this, _: &ShowPatches, window, cx| {
                this.forward(cx, |w, cx| w.show_patches(window, cx))
            }))
            .on_action(cx.listener(|this, _: &PatchApply, window, cx| {
                this.forward(cx, |w, cx| w.patch_apply(window, cx))
            }))
            .on_action(cx.listener(|this, _: &DebugAttach, _, cx| this.unavailable(cx)))
            .on_action(cx.listener(|this, _: &ToggleSplit, _, cx| this.unavailable(cx)))
            .on_action(cx.listener(|this, _: &MoveToOtherPane, _, cx| this.unavailable(cx)))
            .on_action(cx.listener(|this, _: &MoveToNewWindow, _, cx| this.unavailable(cx)))
            .on_action(cx.listener(|this, _: &NextTab, _, cx| this.unavailable(cx)))
            .on_action(cx.listener(|this, _: &PrevTab, _, cx| this.unavailable(cx)))
            .on_action(cx.listener(|this, _: &ShowListing, _, cx| this.unavailable(cx)))
            .on_action(cx.listener(|this, _: &ShowGraph, _, cx| this.unavailable(cx)))
            .on_action(cx.listener(|this, _: &ShowHex, _, cx| this.unavailable(cx)))
            .on_action(cx.listener(|this, _: &ShowPseudocode, _, cx| this.unavailable(cx)))
            .on_action(cx.listener(|this, _: &TogglePseudocode, _, cx| this.unavailable(cx)))
            .on_action(cx.listener(|this, _: &OpOffsetBase, _, cx| {
                this.forward(cx, |w, cx| w.operand_offset_base(cx))
            }))
            .on_action(cx.listener(|this, _: &OpStructOffset, window, cx| {
                this.forward(cx, |w, cx| w.operand_type_chooser(false, window, cx))
            }))
            .on_action(cx.listener(|this, _: &OpEnum, window, cx| {
                this.forward(cx, |w, cx| w.operand_type_chooser(true, window, cx))
            }))
            .on_action(cx.listener(|this, _: &OpStackVar, _, cx| {
                this.forward(cx, |w, cx| w.set_operand(ida_proto::OpRepr::StackVar, cx))
            }))
            .on_action(cx.listener(|this, _: &SearchBytes, _, cx| {
                this.forward(cx, |w, cx| w.search(true, cx))
            }))
            .on_action(cx.listener(|this, _: &SearchText, _, cx| {
                this.forward(cx, |w, cx| w.search(false, cx))
            }))
            .on_action(cx.listener(|this, _: &SearchNextBytes, _, cx| {
                this.forward(cx, |w, cx| w.search_next(true, cx))
            }))
            .on_action(cx.listener(|this, _: &SearchNextText, _, cx| {
                this.forward(cx, |w, cx| w.search_next(false, cx))
            }))
            .on_action(cx.listener(|this, _: &SearchImmediate, _, cx| {
                this.forward(cx, |w, cx| w.search_immediate(false, cx))
            }))
            .on_action(cx.listener(|this, _: &SearchNextImmediate, _, cx| {
                this.forward(cx, |w, cx| w.search_immediate(true, cx))
            }))
            .on_action(cx.listener(|this, _: &SearchNextCode, _, cx| {
                this.forward(cx, |w, cx| w.find_kind("code", "code", cx))
            }))
            .on_action(cx.listener(|this, _: &SearchNextData, _, cx| {
                this.forward(cx, |w, cx| w.find_kind("data", "data", cx))
            }))
            .on_action(cx.listener(|this, _: &SearchNextUnexplored, _, cx| {
                this.forward(cx, |w, cx| w.find_kind("unknown", "unexplored bytes", cx))
            }))
            .on_action(cx.listener(|this, _: &SearchNotFunction, _, cx| {
                this.forward(cx, |w, cx| {
                    w.find_kind("notfunc", "code outside a function", cx)
                })
            }))
            .on_action(cx.listener(|this, _: &JumpToFunction, window, cx| {
                this.forward(cx, |w, cx| w.jump_to_function(window, cx))
            }))
            .on_action(cx.listener(|this, _: &DefineArray, window, cx| {
                this.forward(cx, |w, cx| w.define_array(window, cx))
            }))
            .on_action(cx.listener(|this, _: &SetStringType, window, cx| {
                this.forward(cx, |w, cx| w.set_string_type(window, cx))
            }))
            .on_action(cx.listener(|this, _: &EditFunctionProps, window, cx| {
                this.forward(cx, |w, cx| w.edit_function(window, cx))
            }))
            .on_action(cx.listener(|this, _: &PatchChangeBytes, window, cx| {
                this.forward(cx, |w, cx| w.patch_change_bytes(window, cx))
            }))
            .on_action(cx.listener(|this, _: &PatchAssemble, window, cx| {
                this.forward(cx, |w, cx| w.patch_assemble(window, cx))
            }))
            .on_action(cx.listener(|this, _: &PatchRevert, _, cx| {
                this.forward(cx, |w, cx| w.patch_revert(cx))
            }))
            .child(
                div()
                    .flex_1()
                    .min_h_0()
                    .min_w_0()
                    .child(DocView::Code(self.view.clone()).any()),
            )
            .child(
                div()
                    .h(px(22.))
                    .flex_none()
                    .flex()
                    .items_center()
                    .gap(px(12.))
                    .px(px(10.))
                    .border_t_1()
                    .border_color(hsla(t.border))
                    .text_size(px(11.))
                    .text_color(hsla(t.muted))
                    .child(format!("0x{cursor:X}"))
                    .child(place)
                    .child(div().flex_1().overflow_hidden().child(status)),
            )
            .child(
                div()
                    .absolute()
                    .top_0()
                    .left_0()
                    .size_0()
                    .child(self.menubar.clone()),
            )
            .when_some(self.dialog.as_ref(), |d, dialog| {
                d.child(
                    div()
                        .absolute()
                        .inset_0()
                        .bg(hsla_a(t.scrim))
                        .flex()
                        .items_center()
                        .justify_center()
                        .py(px(24.))
                        .child(
                            div()
                                .max_h_full()
                                .on_mouse_down(gpui::MouseButton::Left, |_, _, cx| {
                                    cx.stop_propagation()
                                })
                                .child(dialog.view.clone()),
                        ),
                )
            })
            .when_some(self.modal.as_ref(), |d, m| {
                d.child(
                    div()
                        .absolute()
                        .inset_0()
                        .bg(hsla_a(t.scrim))
                        .flex()
                        .justify_center()
                        .pt(px(60.))
                        .on_mouse_down(
                            gpui::MouseButton::Left,
                            cx.listener(|this, _, window, cx| this.close_modal(window, cx)),
                        )
                        .child(
                            div()
                                .on_mouse_down(gpui::MouseButton::Left, |_, _, cx| {
                                    cx.stop_propagation()
                                })
                                .child(m.view.clone()),
                        ),
                )
            })
    }
}

/// A saved window rectangle `[x, y, w, h]` made to fit the displays now present: kept when its
/// title bar (top-left area) is on one of them, otherwise moved onto the primary display and
/// never larger than it.
pub fn fit_to_displays(win: [f32; 4], displays: &[[f32; 4]], primary: [f32; 4]) -> [f32; 4] {
    let on = |d: &[f32; 4]| {
        // Where the user grabs the window: a point just inside its top-left corner.
        let (gx, gy) = (win[0] + 40_f32.min(win[2]), win[1] + 10_f32.min(win[3]));
        gx >= d[0] && gx < d[0] + d[2] && gy >= d[1] && gy < d[1] + d[3]
    };
    if displays.iter().any(on) {
        return win;
    }
    let w = win[2].min(primary[2]);
    let h = win[3].min(primary[3]);
    [
        primary[0] + ((primary[2] - w) / 2.).max(0.),
        primary[1] + ((primary[3] - h) / 2.).max(0.),
        w,
        h,
    ]
}

#[cfg(test)]
mod tests {
    use super::fit_to_displays;

    const PRIMARY: [f32; 4] = [0., 0., 1920., 1080.];
    const SECOND: [f32; 4] = [1920., 0., 1920., 1080.];

    #[test]
    fn a_window_on_a_present_display_stays() {
        let w = [2000., 100., 900., 700.];
        assert_eq!(fit_to_displays(w, &[PRIMARY, SECOND], PRIMARY), w);
    }

    #[test]
    fn a_window_on_a_missing_display_moves_to_the_primary() {
        let got = fit_to_displays([2000., 100., 900., 700.], &[PRIMARY], PRIMARY);
        assert_eq!(got, [510., 190., 900., 700.]);
    }

    #[test]
    fn a_window_far_off_screen_is_recovered_and_shrunk_to_fit() {
        let got = fit_to_displays([-9000., -9000., 4000., 3000.], &[PRIMARY], PRIMARY);
        assert_eq!(got, [0., 0., 1920., 1080.]);
    }
}
