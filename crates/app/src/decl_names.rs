// Which types a C declaration text defines, so the Local types header can
// say what the editor holds rather than what the list has selected.
//
// Failure modes the tests cover: comments (line and block) hiding or faking
// names, braces nested inside a body, a typedef of an anonymous struct, a
// function-pointer typedef, several declarations in one text, forward
// declarations, empty text.

/// The names of the top-level declarations in `text`, in order.
pub fn declared_names(text: &str) -> Vec<String> {
    let clean = strip_comments(text);
    let mut names = Vec::new();
    let mut depth = 0usize;
    let mut stmt = String::new();
    for ch in clean.chars() {
        match ch {
            '{' => {
                depth += 1;
                if depth == 1 {
                    stmt.push('{');
                }
            }
            '}' => {
                depth = depth.saturating_sub(1);
                if depth == 0 {
                    stmt.push('}');
                }
            }
            ';' if depth == 0 => {
                if let Some(n) = statement_name(&stmt) {
                    names.push(n);
                }
                stmt.clear();
            }
            _ if depth == 0 => stmt.push(ch),
            _ => {}
        }
    }
    // A last declaration without a semicolon (an enum or struct being typed).
    if let Some(n) = statement_name(&stmt) {
        names.push(n);
    }
    names
}

fn strip_comments(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut it = text.chars().peekable();
    while let Some(c) = it.next() {
        match (c, it.peek()) {
            ('/', Some('/')) => {
                for n in it.by_ref() {
                    if n == '\n' {
                        out.push('\n');
                        break;
                    }
                }
            }
            ('/', Some('*')) => {
                it.next();
                let mut prev = ' ';
                for n in it.by_ref() {
                    if prev == '*' && n == '/' {
                        break;
                    }
                    prev = n;
                }
                out.push(' ');
            }
            _ => out.push(c),
        }
    }
    out
}

fn is_ident(c: char) -> bool {
    c.is_alphanumeric() || c == '_'
}

/// The identifiers of `s`, in order.
fn idents(s: &str) -> Vec<&str> {
    s.split(|c: char| !is_ident(c))
        .filter(|w| !w.is_empty() && !w.starts_with(|c: char| c.is_ascii_digit()))
        .collect()
}

fn statement_name(stmt: &str) -> Option<String> {
    let s = stmt.trim();
    if s.is_empty() {
        return None;
    }
    let words = idents(s);
    let first = *words.first()?;
    if first == "typedef" {
        // `typedef ... NAME` / `typedef void (*NAME)(int)` / `typedef struct {..} NAME`
        let head = match s.find('{') {
            Some(open) => {
                let close = s.rfind('}').unwrap_or(s.len() - 1);
                format!("{}{}", &s[..open], &s[close + 1..])
            }
            None => s.to_owned(),
        };
        if let Some(at) = head.find("(*") {
            return idents(&head[at..]).first().map(|w| (*w).to_owned());
        }
        return idents(&head)
            .into_iter()
            .rfind(|w| *w != "typedef")
            .map(str::to_owned);
    }
    // `struct NAME {` / `union NAME` / `enum NAME`; a forward declaration counts too.
    let at = words
        .iter()
        .position(|w| matches!(*w, "struct" | "union" | "enum"))?;
    words
        .get(at + 1)
        .filter(|w| !matches!(**w, "const" | "volatile"))
        .map(|w| (*w).to_owned())
}

#[cfg(test)]
mod tests {
    use super::declared_names;

    #[test]
    fn plain_and_typedef_declarations() {
        assert_eq!(declared_names("struct Foo { int x; };"), ["Foo"]);
        assert_eq!(declared_names("typedef unsigned int UINT;"), ["UINT"]);
        assert_eq!(
            declared_names("typedef struct _GUID { int a; } GUID;"),
            ["GUID"]
        );
        assert_eq!(declared_names("typedef struct { int a; } Anon;"), ["Anon"]);
        assert_eq!(declared_names("enum Color { RED, GREEN }"), ["Color"]);
    }

    #[test]
    fn function_pointer_typedef_names_the_pointer() {
        assert_eq!(
            declared_names("typedef void (*Callback)(int, struct Foo *);"),
            ["Callback"]
        );
    }

    #[test]
    fn several_declarations_come_in_order() {
        assert_eq!(
            declared_names("struct A { int x; };\nunion B { int y; };\nenum C { Z };"),
            ["A", "B", "C"]
        );
    }

    #[test]
    fn comments_neither_hide_nor_fake_names() {
        assert_eq!(
            declared_names("// struct Fake { };\nstruct Real { int x; /* struct Inner { } */ };"),
            ["Real"]
        );
    }

    #[test]
    fn nested_bodies_do_not_leak_names() {
        assert_eq!(
            declared_names("struct Outer { struct Inner { int a; } i; int b; };"),
            ["Outer"]
        );
    }

    #[test]
    fn forward_declarations_and_empty_text() {
        assert_eq!(declared_names("struct Later;"), ["Later"]);
        assert!(declared_names("").is_empty());
        assert!(declared_names("   // nothing\n").is_empty());
    }
}
