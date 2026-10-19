//! Parsing and formatting for the editing views' text cells.

fn is_ident(c: char) -> bool {
    c.is_alphanumeric() || c == '_'
}

/// Splits a C field declaration (`char name[16]`, `void *next`,
/// `int (*cb)(int)`) into the type text and the field name, the way the host
/// wants them: `("char [16]", "name")`.
pub fn split_field(text: &str) -> Option<(String, String)> {
    let text = text.trim().trim_end_matches(';').trim();
    if let Some(open) = text.find('(') {
        // A function pointer carries its name inside the first group.
        let close = open + text[open..].find(')')?;
        let group = &text[open + 1..close];
        if !group.contains('*') {
            return None;
        }
        let name = trailing_ident(group);
        if name.is_empty() || name.starts_with(|c: char| c.is_ascii_digit()) {
            return None;
        }
        let head = text[..open].trim_end();
        if head.is_empty() {
            return None;
        }
        let group = &group[..group.len() - name.len()];
        return Some((
            format!("{head} ({group}){}", &text[close + 1..]),
            name.to_owned(),
        ));
    }
    let (head, dims) = match text.find('[') {
        Some(br) => (text[..br].trim_end(), text[br..].trim()),
        None => (text, ""),
    };
    let name = trailing_ident(head);
    if name.is_empty() || name.starts_with(|c: char| c.is_ascii_digit()) {
        return None;
    }
    let ty = head[..head.len() - name.len()].trim_end();
    if !ty.ends_with(|c: char| is_ident(c) || c == '*') {
        return None;
    }
    let ty = if dims.is_empty() {
        ty.to_owned()
    } else {
        format!("{ty} {dims}")
    };
    Some((ty, name.to_owned()))
}

fn trailing_ident(s: &str) -> &str {
    let start = s
        .char_indices()
        .rev()
        .take_while(|&(_, c)| is_ident(c))
        .last()
        .map_or(s.len(), |(i, _)| i);
    &s[start..]
}

/// An integer as typed into a value cell: decimal, `0x` hex, `0b` binary,
/// a leading `-`, or `1<<n`.
pub fn parse_value(text: &str) -> Option<u64> {
    let text = text.trim();
    let (negative, body) = match text.strip_prefix('-') {
        Some(rest) => (true, rest.trim_start()),
        None => (false, text),
    };
    let value = match body.split_once("<<") {
        Some((a, b)) => {
            let shift = u32::try_from(parse_unsigned(b.trim())?)
                .ok()
                .filter(|&s| s < 64)?;
            parse_unsigned(a.trim())?.checked_mul(1u64 << shift)?
        }
        None => parse_unsigned(body)?,
    };
    Some(if negative {
        value.wrapping_neg()
    } else {
        value
    })
}

fn parse_unsigned(text: &str) -> Option<u64> {
    let lower = text.to_ascii_lowercase();
    let (digits, radix) = match (lower.strip_prefix("0x"), lower.strip_prefix("0b")) {
        (Some(h), _) => (h.to_owned(), 16),
        (_, Some(b)) => (b.to_owned(), 2),
        _ => (lower, 10),
    };
    if digits.is_empty() || !digits.chars().all(|c| c.is_digit(radix)) {
        return None;
    }
    u64::from_str_radix(&digits, radix).ok()
}

/// A value for display.
pub fn format_value(value: u64, hex: bool) -> String {
    if hex {
        format!("0x{value:X}")
    } else {
        value.to_string()
    }
}

#[cfg(test)]
mod tests;
