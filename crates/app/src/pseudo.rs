// Hex-Rays pseudocode for the function at the cursor. Tokens resolve through
// the ctree anchors Hex-Rays embeds in each line, not through colours (the
// decompiler reuses listing colour codes with other meanings).
use crate::actions::*;
use crate::code::{self, HitMap, line_height};
use crate::commands;
use crate::scrollbar::{self, Axes, Axis, Metrics, Source};
use crate::session::{Caret, Origin, Session, SessionEvent};
use crate::theme::{Theme, hsla};
use gpui::{
    App, Context, Entity, FocusHandle, Focusable, MouseButton, MouseDownEvent, ScrollWheelEvent,
    SharedString, Subscription, Window, canvas, div, point, prelude::*, px,
};
use ida_proto::tags::{self, Line};
use ida_proto::{Decompile, Ea, ItemInfo, PseudoItem, RenameLvar, SetLvarType};

pub struct PseudoView {
    session: Entity<Session>,
    focus: FocusHandle,
    func: Option<Ea>,
    name: SharedString,
    lines: Vec<Line>,
    error: Option<SharedString>,
    top: f32,
    cursor: usize,
    caret_col: usize,
    highlight: Option<SharedString>,
    generation: u64,
    loading: bool,
    viewport_rows: usize,
    scroll_x: f32,
    /// Painted sizes: the text area's width and the widest visible line.
    text_w: f32,
    content_w: f32,
    hit: HitMap,
    /// Only decompile while visible; remember what to show when shown.
    pub visible: bool,
    wanted: Option<Ea>,
    _subs: Vec<Subscription>,
}

impl PseudoView {
    pub fn new(session: Entity<Session>, cx: &mut Context<Self>) -> Self {
        let sub = cx.subscribe(&session, |this, _, ev, cx| match ev {
            SessionEvent::Navigated { ea, .. } => this.follow_address(*ea, cx),
            SessionEvent::CursorMoved { ea, origin } if *origin != Origin::Pseudocode => {
                this.follow_address(*ea, cx)
            }
            SessionEvent::Changed => {
                if let Some(f) = this.func {
                    this.load(f, true, cx);
                }
            }
            SessionEvent::Closed => {
                this.func = None;
                this.lines.clear();
                cx.notify();
            }
            _ => {}
        });
        Self {
            session,
            focus: cx.focus_handle(),
            func: None,
            name: SharedString::default(),
            lines: Vec::new(),
            error: None,
            top: 0.,
            cursor: 0,
            caret_col: 0,
            highlight: None,
            generation: 0,
            loading: false,
            viewport_rows: 40,
            scroll_x: 0.,
            text_w: 0.,
            content_w: 0.,
            hit: HitMap::default(),
            visible: false,
            wanted: None,
            _subs: vec![sub],
        }
    }

    pub fn set_visible(&mut self, visible: bool, cx: &mut Context<Self>) {
        if self.visible == visible {
            return;
        }
        self.visible = visible;
        if visible {
            let ea = self.wanted.unwrap_or(self.session.read(cx).cursor);
            self.follow_address(ea, cx);
        }
    }

    fn follow_address(&mut self, ea: Ea, cx: &mut Context<Self>) {
        self.wanted = Some(ea);
        if !self.visible {
            return;
        }
        let func = self.session.read(cx).function_at(ea).map(|f| f.start);
        match func {
            Some(f) if Some(f) == self.func => {}
            Some(f) => self.load(f, false, cx),
            None if self.func.is_none() => {
                self.error = Some("The cursor is not inside a function.".into());
                cx.notify();
            }
            None => {}
        }
    }

    fn load(&mut self, func: Ea, keep_scroll: bool, cx: &mut Context<Self>) {
        self.generation += 1;
        let generation = self.generation;
        self.loading = true;
        let fut = self.session.read(cx).call(Decompile { ea: func });
        cx.notify();
        cx.spawn(async move |this, cx| {
            let result = fut.await;
            this.update(cx, |v, cx| {
                if v.generation != generation {
                    return;
                }
                v.loading = false;
                match result {
                    Ok(pc) => {
                        v.func = Some(pc.func);
                        v.name = pc.name.into();
                        v.error = pc.error.map(Into::into);
                        v.lines = pc.lines.iter().map(|l| tags::parse(l)).collect();
                        if !keep_scroll {
                            v.top = 0.;
                            v.cursor = 0;
                        }
                        v.cursor = v.cursor.min(v.lines.len().saturating_sub(1));
                    }
                    Err(e) => {
                        v.func = Some(func);
                        v.lines.clear();
                        v.error = Some(e.to_string().into());
                    }
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    fn anchor(&self) -> Option<u64> {
        self.lines.get(self.cursor)?.anchor_at(self.caret_col)
    }

    /// Resolves the token under the caret, then runs `then` with it.
    fn with_item(
        &mut self,
        cx: &mut Context<Self>,
        then: impl FnOnce(&mut Self, Ea, ItemInfo, &mut Context<Self>) + 'static,
    ) {
        let Some(func) = self.func else { return };
        let Some(anchor) = self.anchor() else {
            then(self, func, ItemInfo::default(), cx);
            return;
        };
        let fut = self
            .session
            .read(cx)
            .call(PseudoItem { func, item: anchor });
        cx.spawn(async move |this, cx| {
            let info = fut.await.unwrap_or_default();
            this.update(cx, |v, cx| then(v, func, info, cx)).ok();
        })
        .detach();
    }

    fn follow(&mut self, cx: &mut Context<Self>) {
        self.with_item(cx, |v, _, info, cx| {
            if let Some(t) = info.target {
                v.session.update(cx, |s, cx| s.navigate(t, cx));
            }
        });
    }

    fn publish_caret(&mut self, cx: &mut Context<Self>) {
        let line = self.lines.get(self.cursor);
        let token = line.and_then(|l| l.word_at(self.caret_col).map(|r| l.text[r].to_owned()));
        let caret = Caret {
            origin: Some(Origin::Pseudocode),
            line: Some(self.cursor as u32 + 1),
            col: self.caret_col as u32 + 1,
            block: None,
            token,
        };
        self.session.update(cx, |s, cx| s.set_caret(caret, cx));
    }

    fn sync_cursor(&mut self, cx: &mut Context<Self>) {
        self.publish_caret(cx);
        self.with_item(cx, |v, _, info, cx| {
            if let Some(ea) = info.ea {
                v.session
                    .update(cx, |s, cx| s.set_cursor(ea, Origin::Pseudocode, cx));
            }
        });
    }

    fn rename(&mut self, cx: &mut Context<Self>) {
        self.with_item(cx, |v, func, info, cx| {
            if let Some(var) = info.lvar {
                let session = v.session.clone();
                v.session.update(cx, |s, cx| {
                    s.prompt_name(
                        format!("Rename local variable {var}"),
                        var.clone(),
                        move |new, cx| {
                            session.update(cx, |s, cx| {
                                s.mutate(
                                    RenameLvar {
                                        func,
                                        old: var.clone(),
                                        new,
                                    },
                                    cx,
                                )
                            })
                        },
                        cx,
                    )
                });
            } else {
                commands::rename_ea(&v.session, info.target.unwrap_or(func), cx);
            }
        });
    }

    fn set_type(&mut self, cx: &mut Context<Self>) {
        self.with_item(cx, |v, func, info, cx| {
            if let Some(var) = info.lvar {
                let session = v.session.clone();
                let current = info.lvar_type.unwrap_or_default();
                v.session.update(cx, |s, cx| {
                    s.prompt(
                        format!("Type of {var}"),
                        current,
                        move |decl, cx| {
                            session.update(cx, |s, cx| {
                                s.mutate(
                                    SetLvarType {
                                        func,
                                        name: var.clone(),
                                        decl,
                                    },
                                    cx,
                                )
                            })
                        },
                        cx,
                    )
                });
            } else {
                commands::set_type_ea(&v.session, info.target.unwrap_or(func), cx);
            }
        });
    }

    fn move_cursor(&mut self, delta: isize, cx: &mut Context<Self>) {
        if self.lines.is_empty() {
            return;
        }
        let max = self.lines.len() as isize - 1;
        self.cursor = (self.cursor as isize + delta).clamp(0, max) as usize;
        self.caret_col =
            self.lines[self.cursor].text.len() - self.lines[self.cursor].text.trim_start().len();
        let vp = self.viewport_rows.max(2) as f32;
        let c = self.cursor as f32;
        if c < self.top {
            self.top = c;
        } else if c >= self.top + vp - 1. {
            self.top = c - vp + 2.;
        }
        self.sync_cursor(cx);
        cx.notify();
    }

    fn on_scroll(&mut self, ev: &ScrollWheelEvent, _: &mut Window, cx: &mut Context<Self>) {
        let delta = ev.delta.pixel_delta(px(line_height()));
        if ev.modifiers.shift || f32::from(delta.x) != 0. {
            let dx = if ev.modifiers.shift { delta.y } else { delta.x };
            self.scroll_x = (self.scroll_x - f32::from(dx)).max(0.);
            cx.notify();
            return;
        }
        let max = (self.lines.len() as f32 - 3.).max(0.);
        self.top = (self.top - f32::from(delta.y) / line_height()).clamp(0., max);
        cx.notify();
    }

    fn on_mouse_down(&mut self, ev: &MouseDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        window.focus(&self.focus, cx);
        let Some((row, col)) = self.hit.hit(ev.position) else {
            return;
        };
        self.cursor = row;
        self.caret_col = col;
        self.highlight = self.lines.get(row).and_then(|l| {
            l.word_at(col)
                .map(|r| SharedString::from(l.text[r].to_owned()))
        });
        if ev.click_count >= 2 {
            self.follow(cx);
        } else {
            self.sync_cursor(cx);
        }
        cx.notify();
    }

    fn render_canvas(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let entity = cx.entity();
        canvas(
            |_, _, _| (),
            move |bounds, (), window, cx| {
                let theme = Theme::get(cx);
                let view = entity.read(cx);
                let first = view.top.floor() as usize;
                let frac = view.top - view.top.floor();
                let visible = (f32::from(bounds.size.height) / line_height()).ceil() as usize + 1;
                let end = (first + visible).min(view.lines.len());
                let lines: Vec<(usize, Line)> =
                    (first..end).map(|i| (i, view.lines[i].clone())).collect();
                let cursor = view.cursor;
                let highlight = view.highlight.clone();
                let focused = view.focus.is_focused(window);
                let text_x = bounds.left() + px(56.) - px(view.scroll_x);
                let widest = lines
                    .iter()
                    .map(|(_, l)| l.text.chars().count())
                    .max()
                    .unwrap_or(0) as f32;
                let cw = f32::from(code::char_width(window));
                let mut hit = HitMap {
                    bounds: Some(bounds),
                    text_x,
                    rows: Vec::new(),
                };
                window.with_content_mask(Some(gpui::ContentMask { bounds }), |window| {
                    for (k, (i, line)) in lines.into_iter().enumerate() {
                        let y = bounds.top() + px((k as f32 - frac) * line_height());
                        if i == cursor {
                            code::paint_row_bg(bounds, y, hsla(theme.caret_row), window);
                            if focused {
                                window.paint_quad(gpui::fill(
                                    gpui::Bounds::new(
                                        point(bounds.left(), y),
                                        gpui::size(px(2.), px(line_height())),
                                    ),
                                    hsla(theme.accent),
                                ));
                            }
                        }
                        let shaped = code::shape(window, &line, &theme, highlight.as_deref());
                        let text_bounds = gpui::Bounds::from_corners(
                            point(bounds.left() + px(56.), bounds.top()),
                            bounds.bottom_right(),
                        );
                        window.with_content_mask(
                            Some(gpui::ContentMask {
                                bounds: text_bounds,
                            }),
                            |window| {
                                code::paint_shaped(&shaped, point(text_x, y), window, cx);
                            },
                        );
                        let num = code::shape_plain(
                            window,
                            &(i + 1).to_string(),
                            if i == cursor {
                                hsla(theme.text)
                            } else {
                                hsla(theme.faint)
                            },
                        );
                        code::paint_shaped(
                            &num,
                            point(bounds.left() + px(40.) - num.width, y),
                            window,
                            cx,
                        );
                        hit.rows.push((i, y, shaped));
                    }
                });
                let viewport = (f32::from(bounds.size.height) / line_height()) as usize;
                entity.update(cx, |v, _| {
                    v.hit = hit;
                    v.viewport_rows = viewport.max(1);
                    v.text_w = (f32::from(bounds.size.width) - 56.).max(0.);
                    v.content_w = widest * cw;
                });
            },
        )
        .size_full()
    }
}

impl Focusable for PseudoView {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}

impl Render for PseudoView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = Theme::get(cx);
        let (m, st) = (cx.entity().downgrade(), cx.entity().downgrade());
        let source = Source::custom(
            move |axis, cx| {
                let v = m.upgrade()?;
                let v = v.read(cx);
                match axis {
                    Axis::Vertical => (!v.lines.is_empty()).then_some(Metrics {
                        viewport: v.viewport_rows as f32,
                        content: v.lines.len() as f32 + 2.,
                        offset: v.top,
                    }),
                    Axis::Horizontal => (v.content_w > v.text_w + 1.).then_some(Metrics {
                        viewport: v.text_w,
                        content: v.content_w,
                        offset: v.scroll_x,
                    }),
                }
            },
            move |axis, units, cx| {
                if let Some(v) = st.upgrade() {
                    v.update(cx, |v, cx| {
                        match axis {
                            Axis::Vertical => {
                                v.top = units.clamp(0., (v.lines.len() as f32 - 3.).max(0.))
                            }
                            Axis::Horizontal => {
                                v.scroll_x = units.clamp(0., (v.content_w - v.text_w).max(0.))
                            }
                        }
                        cx.notify();
                    });
                }
            },
        );
        let bar = scrollbar::scrollbar("pseudo-scroll", source, Axes::BOTH, &theme, window, cx);
        let page = self.viewport_rows.max(2) as isize - 1;
        div()
            .id("pseudocode")
            .key_context("CodeView")
            .track_focus(&self.focus)
            .size_full()
            .flex()
            .flex_col()
            .bg(hsla(theme.surface))
            .on_action(cx.listener(|v, _: &CursorUp, _, cx| v.move_cursor(-1, cx)))
            .on_action(cx.listener(|v, _: &CursorDown, _, cx| v.move_cursor(1, cx)))
            .on_action(cx.listener(move |v, _: &PageUp, _, cx| v.move_cursor(-page, cx)))
            .on_action(cx.listener(move |v, _: &PageDown, _, cx| v.move_cursor(page, cx)))
            .on_action(cx.listener(|v, _: &Top, _, cx| v.move_cursor(-(1 << 30), cx)))
            .on_action(cx.listener(|v, _: &Bottom, _, cx| v.move_cursor(1 << 30, cx)))
            .on_action(cx.listener(|v, _: &Follow, _, cx| v.follow(cx)))
            .on_action(cx.listener(|v, _: &Rename, _, cx| v.rename(cx)))
            .on_action(cx.listener(|v, _: &SetType, _, cx| v.set_type(cx)))
            .on_action(cx.listener(|v, _: &Comment, _, cx| {
                v.with_item(cx, |v, func, info, cx| {
                    commands::comment_ea(&v.session, info.ea.unwrap_or(func), false, cx)
                })
            }))
            .on_action(cx.listener(|v, _: &XrefsTo, _, cx| {
                v.with_item(cx, |v, func, info, cx| {
                    commands::show_xrefs(&v.session, info.target.unwrap_or(func), true, cx)
                })
            }))
            .child(
                div()
                    .id("pseudo-body")
                    .flex_1()
                    .min_h_0()
                    .relative()
                    .overflow_hidden()
                    .on_scroll_wheel(cx.listener(Self::on_scroll))
                    .on_mouse_down(MouseButton::Left, cx.listener(Self::on_mouse_down))
                    .child(self.render_canvas(cx))
                    .child(bar)
                    .when_some(self.error.clone(), |d, e| {
                        d.child(
                            div()
                                .absolute()
                                .top(px(10.))
                                .left(px(14.))
                                .right(px(14.))
                                .text_color(hsla(theme.danger))
                                .child(e),
                        )
                    }),
            )
    }
}
