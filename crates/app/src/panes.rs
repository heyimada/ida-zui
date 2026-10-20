//! The tab strips' pure rules: dragging a tab to a new place, which tab comes
//! to the front when one closes, and next / previous tab. A tab is an id; the
//! main view is `None` where a strip includes it.

/// Moves `dragged` before `before` (the end of the strip when `None`),
/// inserting it when it was not in `order` (it came from the other pane).
pub fn reorder(order: &mut Vec<u64>, dragged: u64, before: Option<u64>) {
    if before == Some(dragged) {
        return;
    }
    order.retain(|&t| t != dragged);
    let at = before
        .and_then(|b| order.iter().position(|&t| t == b))
        .unwrap_or(order.len());
    order.insert(at, dragged);
}

/// The tab that comes to the front when `closed` goes: the one before it in
/// the strip, else the one after, else none.
pub fn after_close(strip: &[Option<u64>], closed: u64) -> Option<Option<u64>> {
    let at = strip.iter().position(|t| *t == Some(closed))?;
    let rest: Vec<Option<u64>> = strip
        .iter()
        .copied()
        .filter(|t| *t != Some(closed))
        .collect();
    if rest.is_empty() {
        return None;
    }
    Some(rest[at.saturating_sub(1).min(rest.len() - 1)])
}

/// The tab after (`dir` 1) or before (`dir` -1) `current` in the strip,
/// wrapping around.
pub fn cycle(strip: &[Option<u64>], current: Option<u64>, dir: isize) -> Option<u64> {
    let at = strip.iter().position(|t| *t == current)? as isize;
    let n = strip.len() as isize;
    strip[(at + dir).rem_euclid(n) as usize]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_dragged_tab_lands_before_its_target() {
        let mut o = vec![1, 2, 3, 4];
        reorder(&mut o, 4, Some(2));
        assert_eq!(o, vec![1, 4, 2, 3]);
        reorder(&mut o, 1, None);
        assert_eq!(o, vec![4, 2, 3, 1], "no target: the end");
        reorder(&mut o, 3, Some(3));
        assert_eq!(o, vec![4, 2, 3, 1], "dropped on itself: nothing moves");
    }

    #[test]
    fn a_tab_from_the_other_pane_is_inserted() {
        let mut right = vec![7, 8];
        reorder(&mut right, 2, Some(8));
        assert_eq!(right, vec![7, 2, 8]);
        reorder(&mut right, 5, Some(99));
        assert_eq!(right, vec![7, 2, 8, 5], "an unknown target means the end");
    }

    #[test]
    fn closing_brings_the_neighbour_forward() {
        let strip = [None, Some(1), Some(2), Some(3)];
        assert_eq!(after_close(&strip, 2), Some(Some(1)), "the one before");
        assert_eq!(after_close(&strip, 1), Some(None), "back to the main view");
        assert_eq!(after_close(&strip, 3), Some(Some(2)));
        assert_eq!(
            after_close(&[Some(5)], 5),
            None,
            "an emptied strip has no front"
        );
        assert_eq!(
            after_close(&[Some(5), Some(6)], 5),
            Some(Some(6)),
            "first tab: the next"
        );
        assert_eq!(after_close(&strip, 9), None, "a tab that is not here");
    }

    #[test]
    fn next_and_previous_wrap_around_the_strip() {
        let strip = [None, Some(1), Some(2)];
        assert_eq!(cycle(&strip, None, 1), Some(1));
        assert_eq!(cycle(&strip, Some(2), 1), None, "wraps to the main view");
        assert_eq!(cycle(&strip, None, -1), Some(2));
        assert_eq!(cycle(&strip, Some(7), 1), None, "not in the strip");
    }
}
