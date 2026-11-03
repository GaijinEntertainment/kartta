//! Whether a member function overrides a base class's, read from the two declarations' signatures.

// constraint: a member function overrides a base's virtual one whose declaration it corresponds to, the same name,
// parameter-type-list, cv- and ref-qualifiers (`source/classes.tex`, [class.virtual]; `source/basic.tex`,
// [basic.scope.scope]); one of another parameter list does not override, and a function is virtual only where it
// is declared `virtual` or overrides one that is; a parameter type's top-level cv-qualifiers are no part of the
// parameter-type-list (`source/declarations.tex`, [dcl.fct])
/// Whether the member function `method` spells overrides the base class's member `base` spells: `base` is virtual,
/// written with `virtual`, `override` or `final`, `method` is not deleted, and the two have one parameter-type-list
/// and the same cv- and ref-qualifiers, each parameter read without its name, its default argument, its top-level
/// `const` and `volatile`, an elaborated `class`, `struct`, `enum` or `typename`, and the qualifiers of its type's
/// name. A signature this cannot read overrides nothing.
pub(crate) fn overrides(method: &str, base: &str) -> bool {
    is_virtual(base)
        && !is_deleted(method)
        && declarator(method).is_some_and(|method| declarator(base) == Some(method))
}

fn is_virtual(signature: &str) -> bool {
    words(signature).any(|word| matches!(word, "virtual" | "override" | "final"))
}

fn is_deleted(signature: &str) -> bool {
    signature
        .rsplit_once('=')
        .is_some_and(|(_, after)| after.trim() == "delete")
}

fn words(text: &str) -> impl Iterator<Item = &str> {
    text.split(|c: char| !is_identifier(c))
        .filter(|word| !word.is_empty())
}

const fn is_identifier(c: char) -> bool {
    c.is_ascii_alphanumeric() || c == '_'
}

/// The parameter types and the cv- and ref-qualifiers of the function a signature declares.
fn declarator(signature: &str) -> Option<(Vec<String>, Vec<&str>)> {
    let open = parameter_list(signature)?;
    let mut depth = 0_usize;
    let mut close = None;
    for (at, c) in signature[open..].char_indices() {
        match c {
            '(' | '[' | '{' => depth += 1,
            ')' | ']' | '}' => {
                depth = depth.checked_sub(1)?;
                if depth == 0 {
                    close = Some(open + at);
                    break;
                }
            }
            _ => {}
        }
    }
    let close = close?;
    let parameters = parameter_types(&signature[open + 1..close]);
    let trailing = signature[close + 1..]
        .split(['=', '{', ':'])
        .next()
        .unwrap_or_default();
    Some((parameters, qualifiers(trailing)))
}

/// The cv- and ref-qualifiers that open the text after a parameter list, sorted.
fn qualifiers(trailing: &str) -> Vec<&str> {
    let mut qualifiers = Vec::new();
    let mut rest = trailing.trim_start();
    loop {
        let token = if rest.starts_with("&&") {
            "&&"
        } else if rest.starts_with('&') {
            "&"
        } else {
            &rest[..rest.find(|c: char| !is_identifier(c)).unwrap_or(rest.len())]
        };
        match token {
            "const" | "volatile" | "&" | "&&" => qualifiers.push(token),
            "" | "override" | "final" | "noexcept" | "throw" => break,
            _ => {}
        }
        rest = rest[token.len()..].trim_start();
    }
    qualifiers.sort_unstable();
    qualifiers
}

/// The byte offset of the `(` that opens the parameter list: the first at the top level that follows a name, past
/// the parentheses of an attribute or a specifier and the `()` that names the call operator.
fn parameter_list(signature: &str) -> Option<usize> {
    let (mut angles, mut skipped) = (0_usize, 0_usize);
    for (at, c) in signature.char_indices() {
        match c {
            '<' if skipped == 0 => angles += 1,
            '>' if skipped == 0 => angles = angles.saturating_sub(1),
            '(' if skipped > 0 => skipped += 1,
            ')' if skipped > 0 => skipped -= 1,
            '(' if angles == 0 => {
                let name = signature[..at]
                    .trim_end()
                    .rsplit(|c: char| !is_identifier(c) && c != '~')
                    .next()
                    .unwrap_or_default();
                match name {
                    "" => return None,
                    "operator" if signature[at..].starts_with("()") => {
                        return signature[at + 2..].find('(').map(|open| at + 2 + open);
                    }
                    "__declspec" | "__attribute__" | "alignas" | "decltype" | "noexcept"
                    | "throw" => skipped = 1,
                    _ => return Some(at),
                }
            }
            _ => {}
        }
    }
    None
}

/// The types of a parameter list, one per parameter, `(void)` read as no parameter.
fn parameter_types(list: &str) -> Vec<String> {
    let mut types: Vec<String> = top_level_entries(list)
        .into_iter()
        .map(parameter_type)
        .collect();
    if types.len() == 1 && types[0] == "void" {
        types.clear();
    }
    types
}

fn top_level_entries(list: &str) -> Vec<&str> {
    let mut entries = Vec::new();
    let (mut depth, mut start) = (0_usize, 0);
    for (at, c) in list.char_indices() {
        match c {
            '(' | '[' | '{' | '<' => depth += 1,
            ')' | ']' | '}' | '>' => depth = depth.saturating_sub(1),
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

/// One parameter's type: its tokens without the name it declares, inside the parentheses of a pointer or a reference
/// to a function or an array too, and the default argument it takes.
fn parameter_type(entry: &str) -> String {
    let declared = top_level_cut(entry, '=').trim();
    let mut tokens = tokens(declared);
    tokens.retain(|token| !matches!(token.as_str(), "class" | "struct" | "enum" | "typename"));
    let pointer_to_a_function = (tokens.windows(2))
        .position(|pair| pair[0] == "(" && matches!(pair[1].as_str(), "*" | "&"))
        .and_then(|open| {
            (tokens[open..].iter())
                .position(|token| token == ")")
                .map(|close| open + close)
        });
    let name_at = pointer_to_a_function
        .or_else(|| tokens.iter().position(|token| token == "["))
        .unwrap_or(tokens.len())
        .checked_sub(1);
    if let Some(at) = name_at {
        let typed = tokens[..at]
            .iter()
            .any(|token| !matches!(token.as_str(), "const" | "volatile"));
        if typed && is_name(&tokens[at]) {
            tokens.remove(at);
        }
    }
    let top_level = if pointer_to_a_function.is_some() {
        tokens.len()
    } else {
        (tokens.iter())
            .rposition(|token| matches!(token.as_str(), "*" | "&" | "["))
            .map_or(0, |at| at + 1)
    };
    let mut at = 0;
    tokens.retain(|token| {
        at += 1;
        at <= top_level || !matches!(token.as_str(), "const" | "volatile")
    });
    let mut out = String::with_capacity(declared.len());
    for token in &tokens {
        if out.ends_with(is_identifier) && token.starts_with(is_identifier) {
            out.push(' ');
        }
        out.push_str(token);
    }
    out
}

fn top_level_cut(entry: &str, separator: char) -> &str {
    let mut depth = 0_usize;
    for (at, c) in entry.char_indices() {
        match c {
            '(' | '[' | '{' | '<' => depth += 1,
            ')' | ']' | '}' | '>' => depth = depth.saturating_sub(1),
            c if c == separator && depth == 0 => return &entry[..at],
            _ => {}
        }
    }
    entry
}

fn is_name(token: &str) -> bool {
    token.starts_with(|c: char| c.is_ascii_alphabetic() || c == '_')
        && token.chars().all(is_identifier)
        && !matches!(
            token,
            "void"
                | "bool"
                | "char"
                | "wchar_t"
                | "char8_t"
                | "char16_t"
                | "char32_t"
                | "short"
                | "int"
                | "long"
                | "float"
                | "double"
                | "signed"
                | "unsigned"
                | "auto"
        )
}

/// The tokens of a parameter: names with their template arguments, the qualifiers of a name dropped, and each other
/// character on its own, spaces left out.
fn tokens(text: &str) -> Vec<String> {
    let mut tokens = Vec::new();
    let mut rest = text;
    while let Some(c) = rest.chars().next() {
        if c.is_whitespace() {
            rest = &rest[c.len_utf8()..];
        } else if is_identifier(c) {
            let (name, after) = name(rest);
            tokens.push(name);
            rest = after;
        } else {
            tokens.push(c.to_string());
            rest = &rest[c.len_utf8()..];
        }
    }
    tokens
}

/// The name `text` opens with, its last segment with its template arguments, and the text after it.
fn name(text: &str) -> (String, &str) {
    let identifier = |text: &str| text.find(|c: char| !is_identifier(c)).unwrap_or(text.len());
    let end = identifier(text);
    let (mut name, mut rest) = (text[..end].to_owned(), &text[end..]);
    loop {
        let after = rest.trim_start();
        if after.starts_with('<') {
            let close = closing_angle(after);
            let arguments: Vec<String> = top_level_entries(&after[1..close])
                .into_iter()
                .map(parameter_type)
                .collect();
            name.push('<');
            name.push_str(&arguments.join(","));
            name.push('>');
            rest = &after[(close + 1).min(after.len())..];
            continue;
        }
        let Some(qualified) = after.strip_prefix("::").map(str::trim_start) else {
            return (name, rest);
        };
        let end = identifier(qualified);
        if end == 0 {
            return (name, rest);
        }
        qualified[..end].clone_into(&mut name);
        rest = &qualified[end..];
    }
}

fn closing_angle(text: &str) -> usize {
    let mut depth = 0_usize;
    for (at, c) in text.char_indices() {
        match c {
            '<' => depth += 1,
            '>' => {
                depth = depth.saturating_sub(1);
                if depth == 0 {
                    return at;
                }
            }
            _ => {}
        }
    }
    text.len()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_member_with_the_virtual_members_parameter_types_and_qualifiers_overrides_it() {
        for (method, base) in [
            (
                "void release(void) override",
                "virtual void release(void) = 0",
            ),
            ("void release()", "virtual void release(void) = 0"),
            (
                "virtual NvParameterized::ErrorType getParameterHandle(const char* long_name, NvParameterized::Handle& handle) const",
                "virtual ErrorType getParameterHandle(const char *longName, Handle  &handle) const = 0",
            ),
            (
                "void ApexEmitterAssetParameters_0p4::getVarPtr(const Handle& handle, void*& ptr, size_t& offset) const",
                "virtual void getVarPtr(const Handle &, void *&ptr, size_t &offset) const",
            ),
            (
                "bool visit(AstStatReturn* node) override",
                "virtual bool visit(class AstStatReturn* node)",
            ),
            (
                "int pick(unsigned int count = 3)",
                "virtual int pick(unsigned int)",
            ),
            (
                "void put(const eastl::vector<int, Alloc>& items)",
                "virtual void put(const vector<int,Alloc> &)",
            ),
            ("void on(int) &&", "void on(int x) && override"),
            (
                "void set(unsigned int type, float p)",
                "virtual void set(unsigned int, const float)",
            ),
            ("void put(char* const p)", "virtual void put(char *)"),
            (
                "void on(void (*handler)(int))",
                "virtual void on(void (*cb)(int))",
            ),
            ("void on(void (*)(int))", "virtual void on(void (*cb)(int))"),
            ("void at(int (&row)[4])", "virtual void at(int (&)[4])"),
            (
                "void operator()(int x) override",
                "virtual void operator()(int)",
            ),
        ] {
            assert!(overrides(method, base), "{method} overrides {base}");
        }
    }

    #[test]
    fn a_member_overrides_no_member_that_is_not_virtual_differs_in_its_parameters_or_qualifiers_or_is_deleted()
     {
        for (method, base) in [
            ("float health() const", "float Unit::health() const"),
            (
                "virtual void preVisit ( ExprCopy * expr ) override",
                "virtual void preVisit ( TypeDecl * td )",
            ),
            ("int size()", "virtual int size() const"),
            ("void put(int x)", "virtual void put(long x)"),
            (
                "T &push_back(...) = delete",
                "virtual T& push_back(const T& value)",
            ),
            ("void on(int) &", "virtual void on(int) &&"),
            ("void put(const char* p)", "virtual void put(char *)"),
            (
                "void on(void (*cb)(long))",
                "virtual void on(void (*cb)(int))",
            ),
            ("void operator()(int x)", "virtual void operator()(long x)"),
        ] {
            assert!(
                !overrides(method, base),
                "{method} does not override {base}"
            );
        }
    }
}
