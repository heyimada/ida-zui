// What the caret is on, fetched once for every panel that shows it: the
// inspector (function facts, type, comments) and the live xrefs panel. The
// views publish the cursor and the token under the caret through `Session`;
// this entity resolves them against the database after a short pause, so
// holding an arrow key does not flood the host.
use crate::session::{Session, SessionEvent};
use gpui::{Context, Entity, EventEmitter, SharedString, Subscription};
use ida_proto::{
    AddrInfo, AddrInfoReply, Ea, FuncSummary, FuncSummaryReply, GetComment, GetType, Resolve,
    XrefRow, XrefsTo,
};
use std::time::Duration;

const SETTLE: Duration = Duration::from_millis(90);

pub enum FocusEvent {
    Updated,
}

#[derive(Default)]
pub struct Focus {
    /// Facts about the address under the caret.
    pub addr: Option<AddrInfoReply>,
    /// The enclosing function, when there is one.
    pub func: Option<FuncSummaryReply>,
    /// Callers and references to the enclosing function.
    pub func_xrefs: Vec<XrefRow>,
    /// What the live xrefs refer to: the name under the caret, else the item.
    pub target: Option<Ea>,
    pub target_name: SharedString,
    pub target_xrefs: Vec<XrefRow>,
    pub comment: String,
    pub repeatable_comment: String,
    /// Declaration of the function or item.
    pub type_text: String,
    session: Option<Entity<Session>>,
    _sub: Option<Subscription>,
    key: Option<(Ea, Option<String>, u64)>,
    generation: u64,
}

impl EventEmitter<FocusEvent> for Focus {}

impl Focus {
    pub fn new(session: Entity<Session>, cx: &mut Context<Self>) -> Self {
        let sub = cx.subscribe(&session, |this, _, ev, cx| match ev {
            SessionEvent::Navigated { .. }
            | SessionEvent::CursorMoved { .. }
            | SessionEvent::CaretChanged
            | SessionEvent::Changed
            | SessionEvent::Opened => this.schedule(cx),
            SessionEvent::Closed => this.clear(cx),
            _ => {}
        });
        Self {
            session: Some(session),
            _sub: Some(sub),
            ..Self::default()
        }
    }

    fn clear(&mut self, cx: &mut Context<Self>) {
        let (session, sub) = (self.session.take(), self._sub.take());
        *self = Self {
            session,
            _sub: sub,
            ..Self::default()
        };
        cx.emit(FocusEvent::Updated);
        cx.notify();
    }

    fn schedule(&mut self, cx: &mut Context<Self>) {
        let Some(session) = self.session.clone() else {
            return;
        };
        let s = session.read(cx);
        if !s.is_ready() {
            return;
        }
        let key = (s.cursor, s.caret.token.clone(), s.revision);
        if self.key.as_ref() == Some(&key) {
            return;
        }
        self.key = Some(key.clone());
        self.generation += 1;
        let generation = self.generation;
        cx.spawn(async move |this, cx| {
            cx.background_executor().timer(SETTLE).await;
            let live = this
                .update(cx, |f, _| f.generation == generation)
                .unwrap_or(false);
            if live {
                this.update(cx, |f, cx| f.refresh(key, generation, cx)).ok();
            }
        })
        .detach();
    }

    fn refresh(&mut self, key: (Ea, Option<String>, u64), generation: u64, cx: &mut Context<Self>) {
        let Some(session) = self.session.clone() else {
            return;
        };
        let (cursor, token, _) = key;
        let s = session.read(cx);
        let addr = s.call(AddrInfo { ea: cursor });
        let func = s.call(FuncSummary { ea: cursor });
        let resolved = token.map(|t| {
            s.call(Resolve {
                expr: t,
                from: cursor,
            })
        });
        let comment = s.call(GetComment {
            ea: cursor,
            repeatable: false,
        });
        let rpt = s.call(GetComment {
            ea: cursor,
            repeatable: true,
        });
        let session2 = session.clone();
        cx.spawn(async move |this, cx| {
            let addr = addr.await.ok();
            let func = func.await.ok().flatten();
            let resolved = match resolved {
                Some(f) => f.await.ok().flatten(),
                None => None,
            };
            let comment = comment.await.unwrap_or_default();
            let rpt = rpt.await.unwrap_or_default();
            // The target of the live xrefs: the resolved name, else the item under the caret.
            let target = resolved
                .or_else(|| addr.as_ref().map(|a| a.head))
                .unwrap_or(cursor);
            let (target_info, target_xrefs, func_xrefs, type_text) = {
                let (ti, tx, fx, ty) = session2.read_with(cx, |s, _| {
                    let func_start = func.as_ref().map(|f| f.start);
                    (
                        s.call(AddrInfo { ea: target }),
                        s.call(XrefsTo { ea: target }),
                        func_start.map(|f| s.call(XrefsTo { ea: f })),
                        s.call(GetType {
                            ea: func_start.unwrap_or(target),
                        }),
                    )
                });
                let fx = match fx {
                    Some(f) => f.await.unwrap_or_default(),
                    None => Vec::new(),
                };
                (
                    ti.await.ok(),
                    tx.await.unwrap_or_default(),
                    fx,
                    ty.await.unwrap_or_default(),
                )
            };
            this.update(cx, |f, cx| {
                if f.generation != generation {
                    return;
                }
                f.target_name = target_info
                    .as_ref()
                    .and_then(|i| i.name.clone())
                    .unwrap_or_else(|| format!("{target:X}"))
                    .into();
                f.addr = addr;
                f.func = func;
                f.func_xrefs = func_xrefs;
                f.target = Some(target);
                f.target_xrefs = target_xrefs;
                f.comment = comment;
                f.repeatable_comment = rpt;
                f.type_text = type_text;
                cx.emit(FocusEvent::Updated);
                cx.notify();
            })
            .ok();
        })
        .detach();
    }
}
