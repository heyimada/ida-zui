// Maps addresses to a position along the database's segments, the way the hex
// view maps them to rows: gaps between segments take no room, so a sparse
// 64-bit layout still gives a usable scrollbar. Used by the listing's
// scrollbar, which thumbs across the whole address space.
use ida_proto::{Ea, SegRow};

#[derive(Default)]
pub struct SegMap {
    /// Sorted `(start, end)` ranges with no overlap.
    segs: Vec<(Ea, Ea)>,
    /// Bytes before each range.
    before: Vec<u64>,
    total: u64,
}

impl SegMap {
    pub fn new(rows: &[SegRow]) -> Self {
        let mut segs: Vec<(Ea, Ea)> = rows
            .iter()
            .filter(|s| s.end > s.start)
            .map(|s| (s.start, s.end))
            .collect();
        segs.sort_unstable();
        // Overlapping or touching ranges count once.
        let mut merged: Vec<(Ea, Ea)> = Vec::with_capacity(segs.len());
        for (s, e) in segs {
            match merged.last_mut() {
                Some(last) if s <= last.1 => last.1 = last.1.max(e),
                _ => merged.push((s, e)),
            }
        }
        let mut before = Vec::with_capacity(merged.len());
        let mut total = 0;
        for (s, e) in &merged {
            before.push(total);
            total += e - s;
        }
        Self {
            segs: merged,
            before,
            total,
        }
    }

    pub fn total(&self) -> u64 {
        self.total
    }

    /// The position of `ea`; an address in a gap or past the end snaps to the
    /// next range's start (or the end), one before the first to 0.
    pub fn pos(&self, ea: Ea) -> u64 {
        let i = self.segs.partition_point(|&(_, e)| e <= ea);
        match self.segs.get(i) {
            Some(&(s, _)) => self.before[i] + ea.saturating_sub(s),
            None => self.total,
        }
    }

    /// The address at position `pos` (clamped into the segments); `None`
    /// when there are no segments.
    pub fn ea(&self, pos: u64) -> Option<Ea> {
        let last = self.segs.last()?;
        let pos = pos.min(self.total.saturating_sub(1));
        let i = self.before.partition_point(|&b| b <= pos).saturating_sub(1);
        let (s, e) = self.segs.get(i).copied().unwrap_or(*last);
        Some((s + (pos - self.before[i])).min(e - 1))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn seg(start: Ea, end: Ea) -> SegRow {
        SegRow {
            start,
            end,
            name: String::new(),
            class: String::new(),
            perm: String::new(),
            bitness: 64,
        }
    }

    // Failure modes: no segments, an address in a gap or outside, a position
    // past the end, segments given out of order or overlapping, and the two
    // directions not agreeing.
    #[test]
    fn no_segments_means_nothing_to_scroll() {
        let m = SegMap::new(&[]);
        assert_eq!((m.total(), m.pos(0x1000), m.ea(5)), (0, 0, None));
    }

    #[test]
    fn gaps_take_no_room() {
        let m = SegMap::new(&[seg(0x1000, 0x1100), seg(0x4000_0000, 0x4000_0200)]);
        assert_eq!(m.total(), 0x300);
        assert_eq!(m.pos(0x1000), 0);
        assert_eq!(m.pos(0x4000_0010), 0x110);
        // In the gap: the next segment's start. Past everything: the end. Before: 0.
        assert_eq!(m.pos(0x2000), 0x100);
        assert_eq!(m.pos(u64::MAX), 0x300);
        assert_eq!(m.pos(0x10), 0);
    }

    #[test]
    fn positions_map_back_and_clamp() {
        let m = SegMap::new(&[seg(0x4000_0000, 0x4000_0200), seg(0x1000, 0x1100)]);
        assert_eq!(m.ea(0), Some(0x1000));
        assert_eq!(m.ea(0xFF), Some(0x10FF));
        assert_eq!(m.ea(0x100), Some(0x4000_0000));
        assert_eq!(m.ea(u64::MAX), Some(0x4000_01FF));
        for ea in [0x1000, 0x1080, 0x10FF, 0x4000_0000, 0x4000_01FF] {
            assert_eq!(m.ea(m.pos(ea)), Some(ea));
        }
    }

    #[test]
    fn overlapping_segments_count_once() {
        let m = SegMap::new(&[
            seg(0x1000, 0x1100),
            seg(0x1080, 0x1200),
            seg(0x5000, 0x5000),
        ]);
        assert_eq!(m.total(), 0x200);
    }
}
