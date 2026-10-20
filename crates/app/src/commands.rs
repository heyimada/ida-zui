// Annotation commands shared by the code views: fetch the current value,
// prompt for a new one, then mutate.
use crate::session::Session;
use gpui::{App, Entity};
use ida_proto::*;

fn fmt_ea(ea: Ea) -> String {
    format!("{ea:X}")
}

/// Rename the named location `ea`.
pub fn rename_ea(session: &Entity<Session>, ea: Ea, cx: &mut App) {
    let fut = session.read(cx).call(AddrInfo { ea });
    let session2 = session.clone();
    cx.spawn(async move |cx| {
        let info = fut.await;
        session2.update(cx, |s, cx| match info {
            Ok(info) => {
                let head = info.head;
                let current = info.name.unwrap_or_default();
                let target = cx.entity();
                s.prompt_name(
                    format!("Rename {}", fmt_ea(head)),
                    current,
                    move |name, cx| {
                        target.update(cx, |s, cx| s.mutate(Rename { ea: head, name }, cx));
                    },
                    cx,
                );
            }
            Err(e) => s.error(e.to_string(), cx),
        });
    })
    .detach();
}

/// Rename whatever `token` names, falling back to `fallback` when the token
/// does not resolve.
pub fn rename_token(session: &Entity<Session>, token: Option<String>, fallback: Ea, cx: &mut App) {
    let Some(token) = token else {
        rename_ea(session, fallback, cx);
        return;
    };
    let fut = session.read(cx).call(Resolve {
        expr: token,
        from: fallback,
    });
    let session2 = session.clone();
    cx.spawn(async move |cx| {
        let ea = fut.await.ok().flatten().unwrap_or(fallback);
        cx.update(|cx| rename_ea(&session2, ea, cx));
    })
    .detach();
}

pub fn comment_ea(session: &Entity<Session>, ea: Ea, repeatable: bool, cx: &mut App) {
    let fut = session.read(cx).call(GetComment { ea, repeatable });
    let session2 = session.clone();
    cx.spawn(async move |cx| {
        let current = fut.await.unwrap_or_default();
        session2.update(cx, |_, cx| {
            cx.emit(crate::session::SessionEvent::EditComment(
                crate::session::CommentRequest {
                    ea,
                    repeatable,
                    text: current,
                },
            ))
        });
    })
    .detach();
}

/// Set the type (prototype for functions) of the item at `ea`.
pub fn set_type_ea(session: &Entity<Session>, ea: Ea, cx: &mut App) {
    let s = session.read(cx);
    let info = s.call(AddrInfo { ea });
    let session2 = session.clone();
    cx.spawn(async move |cx| {
        let target = match info.await {
            Ok(i) => i.func_start.filter(|f| *f == i.head).unwrap_or(i.head),
            Err(_) => ea,
        };
        let fut = session2.read_with(cx, |s, _| s.call(GetType { ea: target }));
        let current = fut.await.unwrap_or_default();
        session2.update(cx, |s, cx| {
            let me = cx.entity();
            s.prompt(
                format!("Type of {}", fmt_ea(target)),
                current,
                move |decl, cx| {
                    me.update(cx, |s, cx| s.mutate(SetType { ea: target, decl }, cx));
                },
                cx,
            );
        });
    })
    .detach();
}

pub fn follow_token(session: &Entity<Session>, token: String, from: Ea, cx: &mut App) {
    let fut = session.read(cx).call(Resolve {
        expr: token.clone(),
        from,
    });
    let session2 = session.clone();
    cx.spawn(async move |cx| {
        let result = fut.await;
        session2.update(cx, |s, cx| match result {
            Ok(Some(ea))
                if s.info
                    .as_ref()
                    .is_none_or(|i| crate::focus::is_database_address(ea, i.min_ea, i.max_ea)) =>
            {
                s.navigate(ea, cx)
            }
            Ok(_) => s.info_msg(format!("'{token}' is not a location"), cx),
            Err(e) => s.error(e.to_string(), cx),
        });
    })
    .detach();
}

/// Simple single-address mutations triggered from any code view.
pub fn edit(session: &Entity<Session>, what: EditKind, ea: Ea, cx: &mut App) {
    session.update(cx, |s, cx| match what {
        EditKind::Code => s.mutate(MakeCode { ea }, cx),
        EditKind::Data => s.mutate(MakeData { ea }, cx),
        EditKind::String => s.mutate(MakeString { ea }, cx),
        EditKind::Undefine => s.mutate(Undefine { ea }, cx),
        EditKind::Function => s.mutate(MakeFunction { ea }, cx),
        EditKind::DeleteFunction => s.mutate(DeleteFunction { ea }, cx),
        EditKind::Collapse => s.mutate(ToggleCollapse { ea }, cx),
    });
}

#[derive(Clone, Copy)]
pub enum EditKind {
    Code,
    Data,
    String,
    Undefine,
    Function,
    DeleteFunction,
    Collapse,
}

pub fn show_xrefs(session: &Entity<Session>, ea: Ea, to: bool, cx: &mut App) {
    session.update(cx, |_, cx| {
        cx.emit(crate::session::SessionEvent::ShowXrefs { ea, to })
    });
}

pub fn token_target(
    session: &Entity<Session>,
    token: Option<String>,
    fallback: Ea,
    cx: &mut App,
    then: impl FnOnce(Ea, &mut App) + 'static,
) {
    let Some(token) = token else {
        then(fallback, cx);
        return;
    };
    let fut = session.read(cx).call(Resolve {
        expr: token,
        from: fallback,
    });
    let (min, max) = session
        .read(cx)
        .info
        .as_ref()
        .map_or((0, u64::MAX), |i| (i.min_ea, i.max_ea));
    cx.spawn(async move |cx| {
        // A stack variable resolves to its frame offset: not an item to look up.
        let ea = fut
            .await
            .ok()
            .flatten()
            .filter(|ea| crate::focus::is_database_address(*ea, min, max))
            .unwrap_or(fallback);
        cx.update(|cx| then(ea, cx));
    })
    .detach();
}
