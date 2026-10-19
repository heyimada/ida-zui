//! Maps IDA's type-parser messages to lines of the declaration text.
//!
//! `parse_decls` prints messages such as `Syntax error near: foo_t` or
//! `Field 'a' has already been defined` and says nothing about where they
//! happened. Errors come in source order, so each one is looked up after the
//! previous one.

/// A parser message and the 0-based line it most likely belongs to.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DeclError {
    pub line: usize,
    pub message: String,
}

const SYNTAX: &str = "Syntax error near: ";

/// Locates each message in `messages` (one per line) inside `decl`.
pub fn locate(decl: &str, messages: &str) -> Vec<DeclError> {
    let masked = mask_comments(decl);
    let starts = line_starts(decl);
    let line_of = |off: usize| starts.partition_point(|&s| s <= off).saturating_sub(1);
    let last_line = decl
        .lines()
        .enumerate()
        .filter(|(_, l)| !l.trim().is_empty())
        .map(|(i, _)| i)
        .last()
        .unwrap_or(0);
    let mut cursor = 0;
    let mut out = Vec::new();
    for message in messages
        .lines()
        .map(str::trim_end)
        .filter(|m| !m.trim().is_empty())
    {
        let found = if let Some(token) = message.strip_prefix(SYNTAX) {
            if token == "<END>" {
                out.push(DeclError {
                    line: last_line,
                    message: message.to_owned(),
                });
                continue;
            }
            if token == "}" {
                unterminated_brace(&masked, cursor)
                    .or_else(|| find_token(&masked, cursor, token, false))
            } else {
                find_token(&masked, cursor, token, false)
            }
        } else if let Some(name) = quoted(message) {
            let skip_decl = message.starts_with("Undefined type");
            let first = find_token(&masked, cursor, name, skip_decl);
            match first {
                // The second definition is the offending one.
                Some((at, len)) if message.contains("already been defined") => {
                    find_token(&masked, at + len, name, false).or(first)
                }
                other => other,
            }
        } else {
            None
        };
        let line = match found {
            Some((at, len)) => {
                cursor = at + len;
                line_of(at)
            }
            None => line_of(cursor.min(decl.len())),
        };
        out.push(DeclError {
            line,
            message: message.to_owned(),
        });
    }
    out
}

/// First `'name'` in a message.
fn quoted(message: &str) -> Option<&str> {
    let start = message.find('\'')? + 1;
    let len = message[start..].find('\'')?;
    Some(&message[start..start + len]).filter(|n| !n.is_empty())
}

fn is_ident(c: char) -> bool {
    c.is_alphanumeric() || c == '_'
}

/// `(offset, len)` of the first whole-token occurrence of `token` at or after
/// `from`. With `skip_decls`, occurrences right after `struct`/`union`/`enum`
/// (the type being declared, not used) don't count.
fn find_token(text: &str, from: usize, token: &str, skip_decls: bool) -> Option<(usize, usize)> {
    if token.is_empty() || from > text.len() {
        return None;
    }
    let first_ident = token.chars().next().is_some_and(is_ident);
    let last_ident = token.chars().last().is_some_and(is_ident);
    text[from..]
        .match_indices(token)
        .map(|(i, _)| from + i)
        .find_map(|at| {
            let before = text[..at].chars().next_back();
            let after = text[at + token.len()..].chars().next();
            if first_ident && before.is_some_and(is_ident)
                || last_ident && after.is_some_and(is_ident)
            {
                return None;
            }
            if skip_decls {
                let head = text[..at].trim_end();
                if ["struct", "union", "enum"]
                    .iter()
                    .any(|k| head.ends_with(k) && !head[..head.len() - k.len()].ends_with(is_ident))
                {
                    return None;
                }
            }
            Some((at, token.len()))
        })
}

/// A `}` that follows a member with no `;`: the usual cause of a syntax error
/// at a closing brace, and not the brace of an earlier, valid statement.
fn unterminated_brace(text: &str, from: usize) -> Option<(usize, usize)> {
    text.get(from..)?
        .match_indices('}')
        .map(|(i, _)| from + i)
        .find_map(|at| {
            let prev = text[..at].trim_end().chars().next_back()?;
            (!matches!(prev, ';' | '{' | '}')).then_some((at, 1))
        })
}

fn line_starts(text: &str) -> Vec<usize> {
    std::iter::once(0)
        .chain(text.match_indices('\n').map(|(i, _)| i + 1))
        .collect()
}

/// `text` with comments blanked out (same byte offsets, newlines kept).
fn mask_comments(text: &str) -> String {
    let b = text.as_bytes();
    let mut out = b.to_vec();
    let mut i = 0;
    while i < b.len() {
        let end = if b[i] == b'/' && b.get(i + 1) == Some(&b'/') {
            b[i..]
                .iter()
                .position(|&c| c == b'\n')
                .map_or(b.len(), |p| i + p)
        } else if b[i] == b'/' && b.get(i + 1) == Some(&b'*') {
            text[i + 2..].find("*/").map_or(b.len(), |p| i + 2 + p + 2)
        } else {
            i += 1;
            continue;
        };
        for c in &mut out[i..end] {
            if *c != b'\n' {
                *c = b' ';
            }
        }
        i = end;
    }
    String::from_utf8(out).unwrap_or_else(|_| text.to_owned())
}

#[cfg(test)]
mod tests;
