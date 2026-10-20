//! Back / forward for one view (a pinned tab): the places it has been to.
//! The database-wide history in `Session` serves the views that follow each
//! other; this one is the same thing for a view of its own.

use ida_proto::Ea;

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct NavHistory {
    items: Vec<Ea>,
    pos: usize,
}

impl NavHistory {
    /// A history that starts at `at`.
    pub fn new(at: Ea) -> Self {
        Self {
            items: vec![at],
            pos: 0,
        }
    }

    /// The view was at `from` and jumped to `to`: forward history is dropped
    /// and `to` becomes the present.
    pub fn navigate(&mut self, from: Ea, to: Ea) {
        if self.items.get(self.pos) != Some(&from) {
            self.items.truncate(self.pos + 1);
            self.items.push(from);
            self.pos = self.items.len() - 1;
        }
        self.items.truncate(self.pos + 1);
        if self.items.last() != Some(&to) {
            self.items.push(to);
        }
        self.pos = self.items.len() - 1;
    }

    pub fn back(&mut self) -> Option<Ea> {
        let p = self.pos.checked_sub(1)?;
        self.pos = p;
        Some(self.items[p])
    }

    pub fn forward(&mut self) -> Option<Ea> {
        (self.pos + 1 < self.items.len()).then(|| {
            self.pos += 1;
            self.items[self.pos]
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn back_and_forward_walk_the_jumps_made() {
        let mut h = NavHistory::new(0x10);
        h.navigate(0x10, 0x20);
        h.navigate(0x20, 0x30);
        assert_eq!(h.back(), Some(0x20));
        assert_eq!(h.back(), Some(0x10));
        assert_eq!(h.back(), None, "nothing before the start");
        assert_eq!(h.forward(), Some(0x20));
        assert_eq!(h.forward(), Some(0x30));
        assert_eq!(h.forward(), None);
    }

    #[test]
    fn a_jump_after_going_back_drops_the_way_forward() {
        let mut h = NavHistory::new(1);
        h.navigate(1, 2);
        h.navigate(2, 3);
        h.back();
        h.back();
        h.navigate(1, 9);
        assert_eq!(h.forward(), None);
        assert_eq!(h.back(), Some(1));
    }

    #[test]
    fn jumping_to_where_you_are_adds_nothing() {
        let mut h = NavHistory::new(5);
        h.navigate(5, 5);
        assert_eq!(h.back(), None);
    }

    #[test]
    fn a_caret_that_moved_since_the_last_jump_is_remembered_as_a_place() {
        // Scrolled to 0x15 inside the view, then jumped on: Back returns there.
        let mut h = NavHistory::new(0x10);
        h.navigate(0x15, 0x40);
        assert_eq!(h.back(), Some(0x15));
        assert_eq!(h.back(), Some(0x10));
    }
}
