//! The status bar message: it expires on its own, and the next command
//! clears it. Errors stay readable in the Output panel (`Session::error`).

use std::time::{Duration, Instant};

use gpui::SharedString;

/// How long a message stays in the status bar.
pub const STATUS_TTL: Duration = Duration::from_secs(6);

#[derive(Default)]
pub struct StatusLine {
    current: Option<Message>,
}

struct Message {
    text: SharedString,
    error: bool,
    at: Instant,
}

impl StatusLine {
    pub fn set(&mut self, text: SharedString, error: bool, now: Instant) {
        self.current = Some(Message {
            text,
            error,
            at: now,
        });
    }

    /// The message to show at `now`, with whether it is an error.
    pub fn get(&self, now: Instant) -> Option<(SharedString, bool)> {
        self.current
            .as_ref()
            .filter(|m| now.saturating_duration_since(m.at) < STATUS_TTL)
            .map(|m| (m.text.clone(), m.error))
    }

    pub fn clear(&mut self) {
        self.current = None;
    }

    /// Drops the message once it is older than the TTL; true if one was dropped.
    pub fn expire(&mut self, now: Instant) -> bool {
        if self.current.is_some() && self.get(now).is_none() {
            self.current = None;
            return true;
        }
        false
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn text(s: &StatusLine, now: Instant) -> Option<(String, bool)> {
        s.get(now).map(|(t, e)| (t.to_string(), e))
    }

    #[test]
    fn message_shows_until_the_ttl_then_goes() {
        let t0 = Instant::now();
        let mut s = StatusLine::default();
        s.set("no such name".into(), true, t0);
        assert_eq!(
            text(&s, t0 + STATUS_TTL - Duration::from_millis(1)),
            Some(("no such name".into(), true))
        );
        assert_eq!(text(&s, t0 + STATUS_TTL), None);
        assert!(s.expire(t0 + STATUS_TTL));
        assert!(!s.expire(t0 + STATUS_TTL));
    }

    #[test]
    fn a_newer_message_restarts_the_clock_so_the_old_timer_cannot_cut_it_short() {
        let t0 = Instant::now();
        let mut s = StatusLine::default();
        s.set("first".into(), false, t0);
        let t1 = t0 + Duration::from_secs(5);
        s.set("second".into(), false, t1);
        // The first message's timer fires: the second must survive it.
        assert!(!s.expire(t0 + STATUS_TTL));
        assert_eq!(text(&s, t0 + STATUS_TTL), Some(("second".into(), false)));
        assert!(s.expire(t1 + STATUS_TTL));
    }

    #[test]
    fn the_next_command_clears_it() {
        let t0 = Instant::now();
        let mut s = StatusLine::default();
        s.set("done".into(), false, t0);
        s.clear();
        assert_eq!(text(&s, t0), None);
    }
}
