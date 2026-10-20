//! Pure helpers behind Jump > Next / Previous function and Search >
//! Immediate value: no UI, no host.

use ida_proto::{Ea, FuncRow};

/// The function start to jump to from `ea`: the first start after it going
/// forward, or the last start before it going back (from the middle of a
/// function that is the function's own start). `None` at either end.
pub fn adjacent_function(funcs: &[FuncRow], ea: Ea, forward: bool) -> Option<Ea> {
    let starts = funcs.iter().map(|f| f.start);
    if forward {
        starts.filter(|&s| s > ea).min()
    } else {
        starts.filter(|&s| s < ea).max()
    }
}

/// A number as typed in the "Immediate value" prompt: decimal, `0x1F`, `1Fh`
/// or negative (`-1` is the two's-complement value).
pub fn parse_immediate(text: &str) -> Option<u64> {
    let t = text.trim();
    let (neg, t) = match t.strip_prefix('-') {
        Some(rest) => (true, rest.trim_start()),
        None => (false, t),
    };
    let lower = t.to_ascii_lowercase();
    let magnitude = if let Some(h) = lower.strip_prefix("0x") {
        u64::from_str_radix(h, 16).ok()?
    } else if let Some(h) = lower.strip_suffix('h') {
        u64::from_str_radix(h, 16).ok()?
    } else {
        lower.parse::<u64>().ok()?
    };
    Some(if neg {
        magnitude.wrapping_neg()
    } else {
        magnitude
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn funcs(starts: &[Ea]) -> Vec<FuncRow> {
        starts
            .iter()
            .map(|&start| FuncRow {
                start,
                end: start + 0x10,
                name: String::new(),
                library: false,
                thunk: false,
            })
            .collect()
    }

    #[test]
    fn next_and_previous_function_skip_the_one_under_the_cursor() {
        let f = funcs(&[0x1000, 0x1100, 0x1200]);
        assert_eq!(adjacent_function(&f, 0x1100, true), Some(0x1200));
        assert_eq!(adjacent_function(&f, 0x1100, false), Some(0x1000));
        // From the middle of a function, "previous" lands on its own start first.
        assert_eq!(adjacent_function(&f, 0x1108, false), Some(0x1100));
        assert_eq!(adjacent_function(&f, 0x1108, true), Some(0x1200));
    }

    #[test]
    fn there_is_nothing_past_the_ends_and_the_list_order_does_not_matter() {
        let f = funcs(&[0x1200, 0x1000, 0x1100]);
        assert_eq!(adjacent_function(&f, 0x1200, true), None);
        assert_eq!(adjacent_function(&f, 0x1000, false), None);
        assert_eq!(adjacent_function(&[], 0x1000, true), None);
        // Before the first function, forward finds the first.
        assert_eq!(adjacent_function(&f, 0x10, true), Some(0x1000));
    }

    #[test]
    fn immediates_parse_the_ways_people_type_them() {
        assert_eq!(parse_immediate("255"), Some(255));
        assert_eq!(parse_immediate("0xFF"), Some(255));
        assert_eq!(parse_immediate(" 0XffH "), None);
        assert_eq!(parse_immediate("FFh"), Some(255));
        assert_eq!(parse_immediate("-1"), Some(u64::MAX));
        assert_eq!(parse_immediate("- 2"), Some(u64::MAX - 1));
    }

    #[test]
    fn junk_is_not_an_immediate() {
        for bad in ["", "-", "h", "0x", "12 13", "main", "0xZZ", "1e3"] {
            assert_eq!(parse_immediate(bad), None, "{bad:?}");
        }
    }
}
