use kartta_lang::tree::{signature, text, uncommented};
use kartta_lang::{Symbol, SymbolKind};
use tree_sitter::Node;

use super::name::{function_base_name, is_simple_identifier, sanitize_name, type_base_name};
use super::{child_nodes, exported_symbol};
use crate::repair::{is_macro_name, leading_type_word, macro_annotation_len};

struct DeclaredName {
    name: String,
    scope: Option<String>,
}

pub(super) fn function_symbol(node: Node<'_>, source: &[u8]) -> Option<Symbol> {
    let DeclaredName { name, scope } = match conversion_operator(node, source) {
        Some(declared) => declared,
        None => declared_name(real_function_declarator(node, source)?, source)?,
    };

    let mut symbol = exported_symbol(name, SymbolKind::Function, node);
    if let Some(scope) = scope {
        symbol.kind = SymbolKind::Method;
        symbol.qualified_name = format!("{scope}::{}", symbol.name);
        symbol.owner = Some(scope);
    }
    symbol.signature = trailing_annotation_end(node, source).map_or_else(
        || signature(node, source),
        |end| {
            let written = uncommented(node, node.start_byte()..end, source);
            Some(written.trim_end().to_owned())
        },
    );
    Some(symbol)
}

// constraint: a macro after the parameters of a declaration without a body, `void raise(const char* format, ...)
// PRINTF_ATTR(2, 3);`, ends the declaration at its parameters, and the grammar reads the macro on the same line as a
// declaration or a statement of its own
fn trailing_annotation_end(node: Node<'_>, source: &[u8]) -> Option<usize> {
    if node.child_by_field_name("body").is_some() || !unterminated(node, source) {
        return None;
    }
    let after = node.end_byte() + line_gap_len(&source[node.end_byte()..]);
    macro_annotation_len(&source[after..]).map(|len| after + len)
}

/// Whether the declaration `node` ends without the `;` that closes it.
pub(super) fn unterminated(node: Node<'_>, source: &[u8]) -> bool {
    source[..node.end_byte()].last() != Some(&b';')
}

/// The spaces and tabs `text` starts with.
pub(super) fn line_gap_len(text: &[u8]) -> usize {
    text.iter()
        .take_while(|byte| matches!(byte, b' ' | b'\t'))
        .count()
}

// constraint: a conversion operator, `operator bool() const`, has no function declarator of its own: the grammar
// gives it an `operator_cast` that holds the target type and an abstract declarator of its parameters
fn conversion_operator(node: Node<'_>, source: &[u8]) -> Option<DeclaredName> {
    let declarator = node.child_by_field_name("declarator")?;
    let (cast, scope) = match declarator.kind() {
        "operator_cast" => (declarator, None),
        "qualified_identifier" => match qualified_terminal(declarator) {
            (Some(terminal), scope) if terminal.kind() == "operator_cast" => (terminal, scope),
            _ => return None,
        },
        "function_declarator" => {
            return misread_conversion(declarator, source)
                .or_else(|| conversion_read_as_a_type(node, declarator, source));
        }
        _ => return None,
    };
    let mut abstract_declarator = cast.child_by_field_name("declarator")?;
    while abstract_declarator.kind() != "abstract_function_declarator" {
        let outer = abstract_declarator;
        abstract_declarator = outer
            .child_by_field_name("declarator")
            .or_else(|| child_nodes(outer).find(|child| child.kind().starts_with("abstract_")))?;
    }
    let parameters = abstract_declarator.child_by_field_name("parameters")?;
    let written = String::from_utf8_lossy(&source[cast.start_byte()..parameters.start_byte()]);
    Some(DeclaredName {
        name: sanitize_name(&written),
        scope: scope
            .map(|scope| type_base_name(scope, source))
            .filter(|scope| is_simple_identifier(scope)),
    })
}

// constraint: after a macro, `ML_INLINE double4::operator int4() const`, the parser reads the class and the target type
// as one qualified name and the `operator` keyword between them as an error; a target of more than one word can pull
// the body into that name, which then names nothing
fn misread_conversion(function: Node<'_>, source: &[u8]) -> Option<DeclaredName> {
    let mut qualified = function.child_by_field_name("declarator")?;
    while qualified.kind() == "qualified_identifier" {
        let name = qualified.child_by_field_name("name")?;
        let keyword = child_nodes(qualified).find(|child| {
            child.is_error() && child_nodes(*child).any(|word| word.kind() == "operator")
        });
        if let Some(keyword) = keyword {
            let written = &source[keyword.start_byte()..name.end_byte()];
            if written
                .iter()
                .any(|byte| matches!(byte, b'(' | b')' | b'{' | b'}' | b';'))
            {
                return None;
            }
            return Some(DeclaredName {
                name: sanitize_name(&String::from_utf8_lossy(written)),
                scope: qualified
                    .child_by_field_name("scope")
                    .map(|scope| type_base_name(scope, source))
                    .filter(|scope| is_simple_identifier(scope)),
            });
        }
        qualified = name;
    }
    None
}

// constraint: after a specifier, `constexpr C::operator bool() const`, the parser reads the class and the keyword as
// the return type, `C::operator`, and a one-word target as the name of a function of its own, `bool()`
fn conversion_read_as_a_type(
    node: Node<'_>,
    function: Node<'_>,
    source: &[u8],
) -> Option<DeclaredName> {
    let (keyword, scope) = keyword_read_as_a_type(node, source)?;
    let parameters = function.child_by_field_name("parameters")?;
    let written = String::from_utf8_lossy(&source[keyword.start_byte()..parameters.start_byte()]);
    Some(DeclaredName {
        name: sanitize_name(&written),
        scope: scope
            .map(|scope| type_base_name(scope, source))
            .filter(|scope| is_simple_identifier(scope)),
    })
}

/// The `operator` keyword that ends the return type of the function `node`, and the scope written before it.
fn keyword_read_as_a_type<'t>(
    node: Node<'t>,
    source: &[u8],
) -> Option<(Node<'t>, Option<Node<'t>>)> {
    let written = node
        .child_by_field_name("type")
        .filter(|written| written.kind() == "qualified_identifier")?;
    match qualified_terminal(written) {
        (Some(keyword), scope)
            if keyword.kind() == "type_identifier"
                && &source[keyword.byte_range()] == b"operator" =>
        {
            Some((keyword, scope))
        }
        _ => None,
    }
}

fn declared_name(function: Node<'_>, source: &[u8]) -> Option<DeclaredName> {
    declared_name_of(declarator_name(function, source)?, source)
}

// constraint: a macro before the return type reads as the type, so the return type reads as the declarator, and where
// a space or a line break stands between the name and its parameters, `GLM_FUNC_QUALIFIER typename genType::row_type
// row\n(`, the parser evicts the name into an error before the parameters; tree-sitter finds a node's sibling through
// its parent, which it finds by descending from the root, so the error is looked for among the function's own children
pub(super) fn declarator_name<'t>(function: Node<'t>, source: &[u8]) -> Option<Node<'t>> {
    let declarator = function.child_by_field_name("declarator")?;
    let evicted = child_nodes(function)
        .skip_while(|child| *child != declarator)
        .nth(1)
        .filter(Node::is_error)
        .and_then(|error| {
            let mut words = child_nodes(error);
            let name = words.next().filter(|word| {
                word.kind() == "identifier" && !is_macro_name(&source[word.byte_range()])
            })?;
            words.next().is_none().then_some(name)
        });
    Some(evicted.unwrap_or(declarator))
}

fn declared_name_of(declarator: Node<'_>, source: &[u8]) -> Option<DeclaredName> {
    let mut declarator = declarator;
    let mut stranded_scope = None;
    loop {
        let written = written_name(declarator, source);
        if written.name != declarator {
            stranded_scope = written.stranded_scope.or(stranded_scope);
            declarator = written.name;
        } else if matches!(
            declarator.kind(),
            "pointer_declarator" | "reference_declarator"
        ) {
            let inner = declarator
                .child_by_field_name("declarator")
                .filter(|inner| inner.kind() == "function_declarator")?;
            declarator = declarator_name(inner, source)?;
        } else {
            break;
        }
    }

    let mut declared = written_declared_name(declarator, source)?;
    if declared.scope.is_none() {
        declared.scope = stranded_scope.map(|first| text(first, source));
    }
    Some(declared)
}

fn written_declared_name(declarator: Node<'_>, source: &[u8]) -> Option<DeclaredName> {
    let declared = match declarator.kind() {
        "identifier" | "field_identifier" | "type_identifier" | "operator_name"
        | "destructor_name" | "template_function" | "template_method" => DeclaredName {
            name: function_base_name(declarator, source),
            scope: None,
        },
        "qualified_identifier" => {
            let (terminal, scope) = qualified_terminal(declarator);
            let name = terminal
                .map(|terminal| function_base_name(terminal, source))
                .filter(|name| !name.is_empty())
                .unwrap_or_else(|| sanitize_name(&text(declarator, source)));
            let scope = scope
                .map(|scope| type_base_name(scope, source))
                .filter(|scope| is_simple_identifier(scope));
            DeclaredName { name, scope }
        }
        _ => return None,
    };

    (!declared.name.is_empty()).then_some(declared)
}

pub(super) fn real_function_declarator<'t>(node: Node<'t>, source: &[u8]) -> Option<Node<'t>> {
    if keyword_read_as_a_type(node, source).is_some() {
        return None;
    }
    match misparse_real_declarator(node, source) {
        Some(real) => unwrap_to_function_declarator(real),
        None => unwrap_to_function_declarator(node.child_by_field_name("declarator")?),
    }
}

fn misparse_real_declarator<'t>(node: Node<'t>, source: &[u8]) -> Option<Node<'t>> {
    let field = node.child_by_field_name("declarator")?;
    if leads_with_a_macro(node, field, source) {
        return None;
    }
    let evicted = child_nodes(node)
        .filter(|child| child.is_error() && child.end_byte() <= field.start_byte())
        .find_map(|error| Some((error, declarator_in_error(error)?)));
    if let Some((error, inner)) = evicted {
        if is_calling_convention(error, source) && names_a_function(field, source) {
            return None;
        }
        return Some(first_of_declarator_run(inner));
    }

    let previous = node.prev_sibling()?;
    if node.child_by_field_name("type").is_some()
        || previous.end_position().row != node.start_position().row
    {
        return None;
    }
    if previous.is_error() {
        return declarator_in_error(previous);
    }
    (previous.kind() == "function_declarator"
        && node.parent().is_some_and(|parent| parent.is_error()))
    .then_some(previous)
}

// constraint: a macro before the return type, `JPH_INLINE float Tan(float x)`, parses as the type, or as the word of a
// sized type, `MY_API unsigned char *f()`, and pushes the real type into an error, and a calling convention between the
// return type and the name, `void CALLBACK release()`, parses as an error of its own, where an annotation after the
// name, `int size_ GUARDED_BY(mtx)`, parses as the declarator and pushes the real name there; which of the two is
// spelled as a macro tells them apart, unless a `*` stands right before the macro's spelling, `MY_API T *MD5()`, where
// the annotation's error would stand between them
fn leads_with_a_macro(node: Node<'_>, field: Node<'_>, source: &[u8]) -> bool {
    let macro_type = node
        .child_by_field_name("type")
        .and_then(leading_type_word)
        .is_some_and(|written| {
            written.kind() == "type_identifier" && is_macro_name(&source[written.byte_range()])
        });
    macro_type && names_a_function(field, source)
}

fn names_a_function(field: Node<'_>, source: &[u8]) -> bool {
    unwrap_to_function_declarator(field).is_some_and(|function| {
        function
            .child_by_field_name("declarator")
            .is_some_and(|name| !is_macro_name(&source[name.byte_range()]))
            || function
                .prev_sibling()
                .is_some_and(|before| before.kind() == "*")
    })
}

// constraint: a calling convention is a macro, `CALLBACK`, or a word that starts with `_`, which the compiler
// reserves, `_stdcall`
fn is_calling_convention(error: Node<'_>, source: &[u8]) -> bool {
    let mut words = child_nodes(error);
    words.next().is_some_and(|word| {
        let spelled = &source[word.byte_range()];
        word.kind() == "identifier" && (is_macro_name(spelled) || spelled.starts_with(b"_"))
    }) && words.next().is_none()
}

fn declarator_in_error(error: Node<'_>) -> Option<Node<'_>> {
    child_nodes(error).rfind(|inner| {
        matches!(
            inner.kind(),
            "function_declarator" | "init_declarator" | "identifier" | "field_identifier"
        )
    })
}

fn first_of_declarator_run(last: Node<'_>) -> Node<'_> {
    let mut first = last;
    while first.kind() == "function_declarator"
        && let Some(previous) = first.prev_sibling()
        && previous.kind() == "function_declarator"
    {
        first = previous;
    }
    first
}

// constraint: a pointer return type after a macro, `IMGUI_API ImGuiTabItem* FindTab()`, parses as a scope joined by a
// `::` the source does not hold to a type's pointer declarator, whose function's name reads as a type identifier; a
// scoped type, `PX_INLINE physx::PxPhysics* f()`, parses as the real scope around that seam
fn unwrap_to_function_declarator(mut node: Node<'_>) -> Option<Node<'_>> {
    loop {
        match node.kind() {
            "function_declarator" => return Some(node),
            "pointer_declarator" | "reference_declarator" | "pointer_type_declarator" => {
                node = inner_declarator(node)?;
            }
            "qualified_identifier" => node = node.child_by_field_name("name")?,
            _ => return None,
        }
    }
}

fn inner_declarator(node: Node<'_>) -> Option<Node<'_>> {
    node.child_by_field_name("declarator").or_else(|| {
        child_nodes(node).find(|child| {
            matches!(
                child.kind(),
                "function_declarator" | "pointer_declarator" | "reference_declarator"
            )
        })
    })
}

#[derive(Clone, Copy)]
pub(super) struct WrittenName<'t> {
    pub(super) name: Node<'t>,
    pub(super) stranded_scope: Option<Node<'t>>,
}

// constraint: words before the name parse as its scope: a macro between the return type and the name, `XMMATRIX
// XM_CALLCONV XMMatrixRotationZ()`, or a return type written with a scope after a macro, `INLINE std::string f()`,
// joins the name by a `::` the source does not hold, and where the source writes a scope of its own, `LRESULT CALLBACK
// Window::wndProc()`, the scope's first name is stranded in an error before its `::`; a calling convention after a
// scoped return type, `rtm_impl::vector4f RTM_SIMD_CALL get_x()`, stands in an error with the type's last name right
// before the name, which the scope then does not own; the declared name follows the last such seam
pub(super) fn written_name<'t>(declarator: Node<'t>, source: &[u8]) -> WrittenName<'t> {
    let mut written = WrittenName {
        name: declarator,
        stranded_scope: None,
    };
    let mut qualified = declarator;
    while qualified.kind() == "qualified_identifier"
        && let Some(name) = qualified.child_by_field_name("name")
    {
        let convention_before_the_name = child_nodes(qualified).nth_back(1).is_some_and(|error| {
            error.is_error()
                && child_nodes(error)
                    .next_back()
                    .is_some_and(|word| is_macro_name(&source[word.byte_range()]))
        });
        if convention_before_the_name || child_nodes(qualified).any(|child| child.is_missing()) {
            written = WrittenName {
                name,
                stranded_scope: None,
            };
        } else if let Some(first) = stranded_scope(qualified) {
            written = WrittenName {
                name,
                stranded_scope: Some(first),
            };
        }
        qualified = name;
    }
    written
}

fn stranded_scope(qualified: Node<'_>) -> Option<Node<'_>> {
    let error = child_nodes(qualified).find(Node::is_error)?;
    let names_only = child_nodes(error).all(|child| child.kind() == "identifier");
    let before_a_scope = error
        .next_sibling()
        .is_some_and(|next| next.kind() == "::" && !next.is_missing());
    (names_only && before_a_scope)
        .then(|| child_nodes(error).next_back())
        .flatten()
}

pub(super) fn qualified_terminal(qualified: Node<'_>) -> (Option<Node<'_>>, Option<Node<'_>>) {
    let mut name = None;
    let mut scope = None;
    let mut current = Some(qualified);

    while let Some(node) = current.filter(|node| node.kind() == "qualified_identifier") {
        scope = node.child_by_field_name("scope");
        name = node.child_by_field_name("name");
        current = name;
    }

    (name, scope)
}

pub(super) fn is_pure_virtual(node: Node<'_>) -> bool {
    node.child_by_field_name("declarator")
        .is_some_and(|declarator| declarator.kind() == "function_declarator")
        && node
            .child_by_field_name("default_value")
            .is_some_and(|value| value.kind() == "number_literal")
}

pub(super) fn is_macro_invocation(node: Node<'_>, symbol: &Symbol) -> bool {
    if node.child_by_field_name("type").is_some() {
        return false;
    }

    symbol.owner.as_deref().is_none_or(|owner| {
        !(symbol.name == owner
            || symbol.name.strip_prefix('~') == Some(owner)
            || symbol.name.starts_with("operator"))
    })
}

// constraint: a calling convention after the return type, `virtual HRESULT STDMETHODCALLTYPE find(int a);`, can parse
// as a declaration of the convention closed by a `;` the source does not hold, and the function after it on its line
// as a declaration or a definition without a type
/// The declaration that holds the return type and the calling convention of the typeless function `node`, where the
/// parser split them apart.
pub(super) fn split_return_type<'t>(node: Node<'t>, source: &[u8]) -> Option<Node<'t>> {
    let previous = node.prev_sibling()?;
    let convention = previous.child_by_field_name("declarator")?;
    (matches!(previous.kind(), "field_declaration" | "declaration")
        && previous.end_position().row == node.start_position().row
        && previous.child_by_field_name("type").is_some()
        && matches!(convention.kind(), "field_identifier" | "identifier")
        && is_macro_name(&source[convention.byte_range()])
        && child_nodes(previous)
            .next_back()
            .is_some_and(|last| last.is_missing()))
    .then_some(previous)
}

/// The scope a function's declarator writes before its name where it holds more than one name, each by its base
/// name: `A::N` of `void A::N::f() {}`.
pub(super) fn nested_scope(node: Node<'_>, source: &[u8]) -> Option<String> {
    let declarator = declarator_name(real_function_declarator(node, source)?, source)?;
    let WrittenName {
        name,
        stranded_scope,
    } = written_name(declarator, source);
    let scopes = std::iter::successors(Some(name), |qualified| {
        qualified.child_by_field_name("name")
    })
    .take_while(|qualified| qualified.kind() == "qualified_identifier")
    .filter_map(|qualified| qualified.child_by_field_name("scope"));
    let scopes = stranded_scope.into_iter().chain(scopes);
    scopes.clone().nth(1)?;
    Some(
        scopes
            .map(|scope| type_base_name(scope, source))
            .collect::<Vec<_>>()
            .join("::"),
    )
}
