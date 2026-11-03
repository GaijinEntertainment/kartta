//! A method's type as its signature spells it, for telling whether a type's method is the one an interface declares.

// constraint: two function types are identical where their parameter and result types are, pairwise, and both or
// neither are variadic, whatever the names (`doc/go_spec.html`, Type identity); a parameter list names every
// parameter or none (`doc/go_spec.html`, Function types), so a lone identifier in a named list is a name
/// Whether the method `method` spells, `func (r T) Name(...) ...`, and the interface method `declared` spells,
/// `Name(...) ...`, read as one type: the same parameter and result types in order, each written with its package
/// qualifiers dropped, since the two may sit in different packages. A signature this cannot read matches nothing.
pub(crate) fn same_method(method: &str, declared: &str) -> bool {
    method_type(method).is_some_and(|method| method_type(declared) == Some(method))
}

/// The parameter and result types of a method's signature, each normalized.
fn method_type(signature: &str) -> Option<(Vec<String>, Vec<String>)> {
    let mut rest = signature.trim_start();
    if let Some(after) = rest.strip_prefix("func") {
        let (_, after) = balanced(after.trim_start())?;
        rest = after;
    }
    let rest = rest.trim_start();
    let name_end = rest.find(|c: char| !is_identifier(c))?;
    if name_end == 0 {
        return None;
    }
    let (parameters, results) = balanced(rest[name_end..].trim_start())?;
    let results = results.trim();
    let results = if results.starts_with('(') {
        let (list, after) = balanced(results)?;
        if !after.trim().is_empty() {
            return None;
        }
        types_of(list)
    } else if results.is_empty() {
        Vec::new()
    } else {
        vec![normalized(results)]
    };
    Some((types_of(parameters), results))
}

/// The text inside the parenthesized group `text` opens with, and the text after it.
fn balanced(text: &str) -> Option<(&str, &str)> {
    if !text.starts_with('(') {
        return None;
    }
    let mut depth = 0_usize;
    for (at, c) in text.char_indices() {
        match c {
            '(' | '[' | '{' => depth += 1,
            ')' | ']' | '}' => {
                depth = depth.checked_sub(1)?;
                if depth == 0 {
                    return Some((&text[1..at], &text[at + 1..]));
                }
            }
            _ => {}
        }
    }
    None
}

/// The types of a parameter or result list, one per item, a name's group taking the type that ends it.
fn types_of(list: &str) -> Vec<String> {
    let entries = top_level_entries(list);
    let named = entries.iter().any(|entry| named_entry(entry).is_some());
    if !named {
        return entries.into_iter().map(normalized).collect();
    }
    let mut types = Vec::with_capacity(entries.len());
    let mut pending = 0;
    for entry in entries {
        match named_entry(entry) {
            Some(ty) => {
                let ty = normalized(ty);
                types.extend(std::iter::repeat_n(ty, pending + 1));
                pending = 0;
            }
            None => pending += 1,
        }
    }
    types
}

/// The entries of a list split at its top-level commas, a trailing comma's empty entry left out.
fn top_level_entries(list: &str) -> Vec<&str> {
    let mut entries = Vec::new();
    let (mut depth, mut start) = (0_usize, 0);
    for (at, c) in list.char_indices() {
        match c {
            '(' | '[' | '{' => depth += 1,
            ')' | ']' | '}' => depth = depth.saturating_sub(1),
            ',' if depth == 0 => {
                entries.push(list[start..at].trim());
                start = at + 1;
            }
            _ => {}
        }
    }
    entries.push(list[start..].trim());
    entries.retain(|entry| !entry.is_empty());
    entries
}

/// The type of an entry that names its parameter, `r io.Reader`, or `None` for an entry that is a lone type or name.
fn named_entry(entry: &str) -> Option<&str> {
    let end = entry.find(|c: char| !is_identifier(c))?;
    let (name, rest) = entry.split_at(end);
    let starts_a_type = matches!(name, "chan" | "func" | "map" | "struct" | "interface");
    let rest = rest.trim_start();
    (!name.is_empty() && !starts_a_type && rest.len() < entry.len() - end && !rest.is_empty())
        .then_some(rest)
}

/// A type with its spaces and package qualifiers dropped, and `interface{}` read as `any`.
fn normalized(ty: &str) -> String {
    let mut out = String::with_capacity(ty.len());
    let mut rest = ty;
    while let Some(c) = rest.chars().next() {
        if c.is_whitespace() {
            rest = &rest[c.len_utf8()..];
        } else if let Some(after) = rest.strip_prefix("...") {
            out.push_str("...");
            rest = after;
        } else if is_identifier(c) {
            let end = rest.find(|c: char| !is_identifier(c)).unwrap_or(rest.len());
            let (word, after) = rest.split_at(end);
            match after.strip_prefix('.') {
                Some(qualified) if !after.starts_with("...") => rest = qualified,
                _ => {
                    out.push_str(word);
                    rest = after;
                }
            }
        } else {
            out.push(c);
            rest = &rest[c.len_utf8()..];
        }
    }
    out.replace("interface{}", "any")
}

fn is_identifier(c: char) -> bool {
    c.is_alphanumeric() || c == '_'
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_method_matches_an_interface_method_of_its_parameter_and_result_types_whatever_their_names()
    {
        let matching = [
            ("func (Kernel) Boot()", "Boot()"),
            (
                "func (c *Client) Do(req *http.Request) (*http.Response, error)",
                "Do(r *http.Request) (*http.Response, error)",
            ),
            ("func (s *S) Set(a, b int) error", "Set(int, int) error"),
            (
                "func (s *S) Read(p []byte) (n int, err error)",
                "Read([]byte) (int, error)",
            ),
            ("func (s *S) Use(v interface{})", "Use(value any)"),
            ("func (s *S) Wrap(r Request)", "Wrap(r api.Request)"),
            (
                "func (s *S) Log(f string, args ...any)",
                "Log(string, ...any)",
            ),
            (
                "func (s *S) On(f func(int) error, c chan int)",
                "On(func(int) error, chan int)",
            ),
        ];
        for (method, declared) in matching {
            assert!(same_method(method, declared), "{method} is {declared}");
        }
    }

    #[test]
    fn a_method_of_other_parameter_or_result_types_matches_nothing() {
        let differing = [
            (
                "func (c *JobsQueryCall) Do(opts ...googleapi.CallOption) (*QueryResponse, error)",
                "Do(req *Request) (*Response, error)",
            ),
            ("func (re RawElement) Value() RawValue", "Value() []any"),
            (
                "func (n *MemoryStorer) Set(key any, value any) error",
                "Set(key string, value *Bucket) error",
            ),
            (
                "func (s *S) Do(ctx context.Context) error",
                "Do(req *http.Request) error",
            ),
            ("func (s *S) Log(args []any)", "Log(args ...any)"),
            ("func (s *S) Close()", "Close() error"),
            ("func (s *S) Close(", "Close()"),
        ];
        for (method, declared) in differing {
            assert!(!same_method(method, declared), "{method} is not {declared}");
        }
    }
}
