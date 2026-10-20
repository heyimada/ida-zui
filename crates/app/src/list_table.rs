//! The side lists as tables: what a sorted column compares, how a click on a
//! header changes the sort, and column widths. Pure; the sidebar draws it.
use serde::{Deserialize, Serialize};
use std::cmp::Ordering;

/// One cell's value for sorting: numbers compare as numbers (so 9 < 10 < 100
/// and addresses by value), text compares ignoring case.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SortKey {
    Num(u64),
    Text(String),
}

impl SortKey {
    pub fn text(s: &str) -> Self {
        Self::Text(s.to_lowercase())
    }

    fn compare(&self, other: &Self) -> Ordering {
        match (self, other) {
            (Self::Num(a), Self::Num(b)) => a.cmp(b),
            (Self::Text(a), Self::Text(b)) => a.cmp(b),
            // A column holds one kind; if it ever mixes, numbers come first.
            (Self::Num(_), Self::Text(_)) => Ordering::Less,
            (Self::Text(_), Self::Num(_)) => Ordering::Greater,
        }
    }
}

/// Reorders `order` (row indices) by `keys[row]`. Stable in both directions:
/// rows with equal keys stay in their original (address) order.
pub fn sort_order(keys: &[SortKey], order: &mut [usize], ascending: bool) {
    order.sort_by(|&a, &b| {
        let o = keys[a].compare(&keys[b]);
        if ascending { o } else { o.reverse() }
    });
}

/// A table's persisted state: the sorted column, its direction, and the width
/// of each column in pixels.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct TableState {
    pub sort: Option<usize>,
    pub descending: bool,
    pub widths: Vec<f32>,
}

impl TableState {
    /// A click on column `col`'s header: sort by it ascending, or flip the
    /// direction when it already is the sorted column.
    pub fn click(&mut self, col: usize) {
        if self.sort == Some(col) {
            self.descending = !self.descending;
        } else {
            self.sort = Some(col);
            self.descending = false;
        }
    }

    /// The widths to draw: the saved ones where they fit the column count,
    /// else `defaults`.
    pub fn widths_or(&self, defaults: &[f32]) -> Vec<f32> {
        if self.widths.len() == defaults.len() && self.widths.iter().all(|w| *w >= MIN_COL) {
            self.widths.clone()
        } else {
            defaults.to_vec()
        }
    }
}

/// The narrowest a column can be dragged to.
pub const MIN_COL: f32 = 36.;
pub const MAX_COL: f32 = 600.;

/// The width after dragging a column edge by `delta` pixels.
pub fn resized(width: f32, delta: f32) -> f32 {
    (width + delta).clamp(MIN_COL, MAX_COL)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn order(keys: &[SortKey], ascending: bool) -> Vec<usize> {
        let mut o: Vec<usize> = (0..keys.len()).collect();
        sort_order(keys, &mut o, ascending);
        o
    }

    #[test]
    fn numbers_sort_by_value_not_by_their_text() {
        let keys = [SortKey::Num(100), SortKey::Num(9), SortKey::Num(10)];
        assert_eq!(order(&keys, true), vec![1, 2, 0]);
        // Addresses and lengths above 32 bits too.
        let big = [SortKey::Num(0x1_4000_2850), SortKey::Num(0x1_4000_18C0)];
        assert_eq!(order(&big, true), vec![1, 0]);
    }

    #[test]
    fn names_sort_ignoring_case() {
        let keys = [
            SortKey::text("gamma"),
            SortKey::text("Beta"),
            SortKey::text("alpha"),
            SortKey::text("ALPHA2"),
        ];
        assert_eq!(order(&keys, true), vec![2, 3, 1, 0]);
    }

    #[test]
    fn descending_reverses_everything_except_the_order_of_ties() {
        let keys = [
            SortKey::Num(2),
            SortKey::Num(1),
            SortKey::Num(2),
            SortKey::Num(3),
        ];
        assert_eq!(order(&keys, true), vec![1, 0, 2, 3]);
        // The two 2s stay in address order (0 before 2) in both directions.
        assert_eq!(order(&keys, false), vec![3, 0, 2, 1]);
    }

    #[test]
    fn a_header_click_sorts_ascending_then_flips() {
        let mut t = TableState::default();
        t.click(2);
        assert_eq!((t.sort, t.descending), (Some(2), false));
        t.click(2);
        assert_eq!((t.sort, t.descending), (Some(2), true));
        t.click(0);
        assert_eq!(
            (t.sort, t.descending),
            (Some(0), false),
            "a new column starts ascending"
        );
    }

    #[test]
    fn saved_widths_are_used_only_when_they_match_the_columns() {
        let defaults = [100., 50., 80.];
        let mut t = TableState::default();
        assert_eq!(t.widths_or(&defaults), defaults.to_vec());
        t.widths = vec![120., 60., 90.];
        assert_eq!(t.widths_or(&defaults), vec![120., 60., 90.]);
        t.widths = vec![120., 60.];
        assert_eq!(
            t.widths_or(&defaults),
            defaults.to_vec(),
            "a column was added"
        );
        t.widths = vec![120., 2., 90.];
        assert_eq!(
            t.widths_or(&defaults),
            defaults.to_vec(),
            "a collapsed column is reset"
        );
    }

    #[test]
    fn dragging_a_column_edge_keeps_it_readable() {
        assert_eq!(resized(100., 30.), 130.);
        assert_eq!(resized(100., -500.), MIN_COL);
        assert_eq!(resized(500., 500.), MAX_COL);
    }
}
