// Lumina: what the dialogs say. The kernel reports success only through its
// log and the number of functions that carry Lumina metadata, so the outcome
// is derived from both. A push uploads the user's analysis to a third-party
// server; its confirmation text names the server and what leaves the machine.
use ida_proto::{LuminaAction, LuminaReply};

/// What a finished request tells the user.
pub struct Outcome {
    pub title: String,
    pub detail: String,
    pub ok: bool,
}

pub fn action_label(action: LuminaAction) -> &'static str {
    match action {
        LuminaAction::PullAll => "Pull metadata for all functions",
        LuminaAction::PullFunc => "Pull metadata for the current function",
        LuminaAction::PushAll => "Push metadata for all functions",
        LuminaAction::PushFunc => "Push metadata for the current function",
    }
}

/// The text of the confirmation dialog for a push.
pub fn push_warning(action: LuminaAction, server: &str, function: Option<&str>) -> String {
    let scope = match (action, function) {
        (LuminaAction::PushFunc, Some(f)) => format!("the metadata of {f}"),
        (LuminaAction::PushFunc, None) => "the metadata of the current function".to_owned(),
        _ => "the metadata of every function in this database".to_owned(),
    };
    let server = if server.is_empty() {
        "the configured Lumina server"
    } else {
        server
    };
    format!(
        "This sends {scope} to the Lumina server {server}, a third party. \
         What is uploaded: function hashes, names, prototypes, comments and variable \
         names and types, and the input file's name and hash. Other Lumina users can then \
         receive it. An upload cannot be taken back from here."
    )
}

fn is_error_line(line: &str) -> bool {
    let l = line.to_lowercase();
    l.starts_with("lumina:") || l.contains("failed") || l.contains("refused") || l.contains("error")
}

/// A reply, turned into what to show.
pub fn outcome(action: LuminaAction, r: &LuminaReply, server: &str) -> Outcome {
    let server = if server.is_empty() {
        "the Lumina server"
    } else {
        server
    };
    let gained = r.lumina_after.saturating_sub(r.lumina_before);
    let log = r.log.join("\n");
    let reason = r.log.iter().find(|l| is_error_line(l)).cloned();
    if reason.is_some() && gained == 0 {
        let network = r.log.iter().any(|l| {
            let l = l.to_lowercase();
            l.contains("recv")
                || l.contains("connect")
                || l.contains("forcibly")
                || l.contains("timed out")
                || l.contains("refused")
        });
        return Outcome {
            title: if network {
                "Lumina server unreachable".into()
            } else {
                format!("{} failed", action_label(action))
            },
            detail: format!(
                "IDA could not complete the request to {server}.\n\n{}\n\nNothing in the database was changed.",
                reason.unwrap_or_default()
            ),
            ok: false,
        };
    }
    let detail = if log.is_empty() {
        String::new()
    } else {
        format!("\n\n{log}")
    };
    match action {
        LuminaAction::PullAll | LuminaAction::PullFunc if gained > 0 => Outcome {
            title: "Lumina metadata applied".into(),
            detail: format!(
                "{gained} function{} updated from {server}; {} of {} functions now carry Lumina metadata.{detail}",
                if gained == 1 { "" } else { "s" },
                r.lumina_after,
                r.functions
            ),
            ok: true,
        },
        LuminaAction::PullAll | LuminaAction::PullFunc => Outcome {
            title: "Nothing new from Lumina".into(),
            detail: format!("{server} had no metadata for the requested functions.{detail}"),
            ok: true,
        },
        _ => Outcome {
            title: "Metadata pushed".into(),
            detail: format!("The request to {server} finished.{detail}"),
            ok: true,
        },
    }
}
