// The Python console's command history: a capped list of entered sources
// (multi-line ones included), Up/Down recall that keeps the half-typed text,
// and a JSON file in the settings directory. Pure logic, no gpui.
use std::path::Path;

pub const DEFAULT_CAP: usize = 500;

#[derive(Debug, Default)]
pub struct History {
    entries: Vec<String>,
    cap: usize,
    /// Index of the entry on screen while navigating.
    at: Option<usize>,
    /// What was in the input when navigation started.
    draft: String,
}

impl History {
    pub fn new(cap: usize) -> Self {
        Self {
            cap: cap.max(1),
            ..Self::default()
        }
    }

    /// Reads the file; a missing or damaged one is an empty history.
    pub fn load(path: &Path, cap: usize) -> Self {
        let mut h = Self::new(cap);
        let list: Vec<String> = std::fs::read_to_string(path)
            .ok()
            .and_then(|t| serde_json::from_str(&t).ok())
            .unwrap_or_default();
        for e in list {
            h.push(&e);
        }
        h
    }

    pub fn save(&self, path: &Path) -> std::io::Result<()> {
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        let tmp = path.with_extension("json.tmp");
        std::fs::write(
            &tmp,
            serde_json::to_vec(&self.entries).map_err(std::io::Error::other)?,
        )?;
        std::fs::rename(&tmp, path)
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    #[cfg(test)]
    fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// Records an entered source. Blank input and a repeat of the newest
    /// entry are not kept; navigation starts over.
    pub fn push(&mut self, text: &str) {
        self.at = None;
        let text = text.trim_end();
        if text.trim().is_empty() || self.entries.last().is_some_and(|l| l == text) {
            return;
        }
        self.entries.push(text.to_owned());
        self.trim();
    }

    pub fn set_cap(&mut self, cap: usize) {
        self.cap = cap.max(1);
        self.at = None;
        self.trim();
    }

    pub fn clear(&mut self) {
        self.entries.clear();
        self.at = None;
    }

    /// Up: the next older entry. The first step remembers `current` so that
    /// walking back down restores it. At the oldest entry it stays there.
    pub fn prev(&mut self, current: &str) -> Option<&str> {
        if self.entries.is_empty() {
            return None;
        }
        let at = match self.at {
            None => {
                self.draft = current.to_owned();
                self.entries.len() - 1
            }
            Some(i) => i.saturating_sub(1),
        };
        self.at = Some(at);
        Some(&self.entries[at])
    }

    /// Down: the next newer entry, then the remembered draft. Nothing when
    /// not navigating.
    pub fn next(&mut self) -> Option<&str> {
        let i = self.at?;
        if i + 1 < self.entries.len() {
            self.at = Some(i + 1);
            Some(&self.entries[i + 1])
        } else {
            self.at = None;
            Some(&self.draft)
        }
    }

    fn trim(&mut self) {
        if self.entries.len() > self.cap {
            let extra = self.entries.len() - self.cap;
            self.entries.drain(..extra);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn filled(items: &[&str]) -> History {
        let mut h = History::new(100);
        for i in items {
            h.push(i);
        }
        h
    }

    #[test]
    fn the_oldest_entries_fall_off_at_the_cap() {
        let mut h = History::new(3);
        for i in 1..=5 {
            h.push(&format!("x = {i}"));
        }
        assert_eq!(h.len(), 3);
        assert_eq!(h.prev("").unwrap(), "x = 5");
        h.prev("");
        assert_eq!(h.prev("").unwrap(), "x = 3");
        assert_eq!(
            h.prev("").unwrap(),
            "x = 3",
            "nothing older than the oldest kept entry"
        );
    }

    #[test]
    fn blank_input_and_repeats_of_the_newest_are_not_kept() {
        let mut h = filled(&["a", "a", "  ", "", "\n", "a\n", "b", "a"]);
        assert_eq!(h.len(), 3, "a, b, a: only consecutive repeats collapse");
        assert_eq!(h.prev("").unwrap(), "a");
    }

    #[test]
    fn walking_down_restores_what_was_being_typed() {
        let mut h = filled(&["one", "two"]);
        assert_eq!(h.prev("half typed").unwrap(), "two");
        assert_eq!(h.prev("ignored while navigating").unwrap(), "one");
        assert_eq!(h.next().unwrap(), "two");
        assert_eq!(h.next().unwrap(), "half typed");
        assert!(h.next().is_none(), "past the draft there is nothing");
    }

    #[test]
    fn down_without_up_and_up_on_empty_do_nothing() {
        let mut h = History::new(10);
        assert!(h.prev("x").is_none());
        assert!(h.next().is_none());
        let mut h = filled(&["a"]);
        assert!(h.next().is_none());
    }

    #[test]
    fn entering_a_command_starts_navigation_over() {
        let mut h = filled(&["one", "two"]);
        h.prev("");
        h.prev("");
        h.push("three");
        assert_eq!(h.prev("").unwrap(), "three");
        assert_eq!(h.prev("").unwrap(), "two");
    }

    #[test]
    fn a_recalled_entry_that_runs_again_is_not_duplicated() {
        let mut h = filled(&["one", "two"]);
        assert_eq!(h.prev("").unwrap(), "two");
        h.push("two");
        assert_eq!(h.len(), 2);
    }

    #[test]
    fn multi_line_entries_survive_a_save_and_load_and_the_cap_applies() {
        let dir = std::env::temp_dir().join(format!("ida-zui-hist-{}", std::process::id()));
        let path = dir.join("python_history.json");
        let h = filled(&["for i in range(3):\n    print(i)", "x = 'é\"\\'", "last"]);
        h.save(&path).unwrap();
        let mut back = History::load(&path, 2);
        assert_eq!(
            back.len(),
            2,
            "loading keeps the newest entries within the cap"
        );
        assert_eq!(back.prev("").unwrap(), "last");
        assert_eq!(back.prev("").unwrap(), "x = 'é\"\\'");
        let mut all = History::load(&path, 10);
        all.prev("");
        all.prev("");
        assert_eq!(all.prev("").unwrap(), "for i in range(3):\n    print(i)");
    }

    #[test]
    fn a_missing_or_damaged_file_is_an_empty_history() {
        let dir = std::env::temp_dir().join(format!("ida-zui-hist-bad-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        assert!(History::load(&dir.join("nope.json"), 10).is_empty());
        std::fs::write(dir.join("bad.json"), "{ not a list").unwrap();
        assert!(History::load(&dir.join("bad.json"), 10).is_empty());
    }
}
