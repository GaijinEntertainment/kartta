//! The identity a C++ callable is keyed by: what joins a declaration to its definition across files, and what keeps
//! two callables apart.

use kartta_lang::{CallableRole, Language, RawExtraction};
use kartta_lang_cpp::Cpp;

fn extract(source: &str) -> RawExtraction {
    Cpp::new().extract(source.as_bytes())
}

fn keys(raw: &RawExtraction) -> Vec<&str> {
    raw.callables
        .iter()
        .map(|callable| callable.key.as_str())
        .collect()
}

fn keys_with_roles(raw: &RawExtraction) -> Vec<(&str, CallableRole)> {
    raw.callables
        .iter()
        .map(|callable| (callable.key.as_str(), callable.role))
        .collect()
}

fn names(raw: &RawExtraction) -> Vec<&str> {
    raw.symbols
        .iter()
        .map(|symbol| symbol.name.as_str())
        .collect()
}

fn qualified_names(raw: &RawExtraction) -> Vec<&str> {
    raw.symbols
        .iter()
        .map(|symbol| symbol.qualified_name.as_str())
        .collect()
}

#[test]
fn identities_preserve_scope_types_and_member_qualifiers_but_not_parameter_names() {
    let header = extract(
        "namespace ns { struct Widget { void run(const int &value, int count = 1) const &; }; }",
    );
    let source = extract("void ns::Widget::run(const int &input, int size) const & {}");

    assert_eq!(
        keys_with_roles(&header),
        [(
            "ns::Widget::run(const int &,int)const &",
            CallableRole::Declaration
        )]
    );
    assert_eq!(
        keys_with_roles(&source),
        [(
            "ns::Widget::run(const int &,int)const &",
            CallableRole::Definition
        )]
    );
}

#[test]
fn overloads_and_namespace_distinct_owners_keep_distinct_identities() {
    let raw = extract(
        "
namespace first { struct Widget { void run(int value); void run(float value); }; }
namespace second { struct Widget { void run(int value); }; }
",
    );

    assert_eq!(
        keys(&raw),
        [
            "first::Widget::run(int)",
            "first::Widget::run(float)",
            "second::Widget::run(int)"
        ]
    );
    assert_eq!(
        qualified_names(&raw),
        [
            "first::Widget",
            "second::Widget",
            "Widget::run",
            "Widget::run",
            "Widget::run"
        ]
    );
}

#[test]
fn a_definition_does_not_discard_a_different_overload_declaration() {
    let raw = extract("void run(int value); void run(float value); void run(int input) {}");

    assert_eq!(
        keys_with_roles(&raw),
        [
            ("run(int)", CallableRole::Definition),
            ("run(float)", CallableRole::Declaration)
        ]
    );
}

#[test]
fn qualifier_position_does_not_split_one_parameter_type() {
    let declared = extract("void run(const volatile int &value);");
    let defined = extract("void run(int volatile const &input) {}");
    let distinct = extract("void run(const int **); void run(int *const *); void run(int **);");

    assert_eq!(keys(&declared), ["run(const volatile int &)"]);
    assert_eq!(keys(&defined), ["run(const volatile int &)"]);
    assert_eq!(
        keys(&distinct),
        ["run(const int * *)", "run(int * const *)", "run(int * *)"]
    );
}

#[test]
fn same_file_local_and_special_callables_deduplicate_by_identity() {
    let raw = extract(concat!(
        "static void run(int value);\nstatic void run(int input) {}\n",
        "struct Local { ~Local(); Local &operator=(const Local &); };\n",
        "Local::~Local() {}\nLocal &Local::operator=(const Local &input) { return *this; }\n",
    ));
    let repeated = extract(
        "static void run(int value); static void run(int input); static void run(float value);",
    );

    assert_eq!(
        qualified_names(&raw),
        ["run", "Local", "Local::~Local", "Local::operator="]
    );
    assert_eq!(
        keys(&raw),
        ["Local::~Local()", "Local::operator=(const Local &)"]
    );
    let folded: Vec<(&str, u32)> = raw
        .folded_declarations
        .iter()
        .map(|folded| {
            (
                raw.symbols[folded.definition.0 as usize]
                    .qualified_name
                    .as_str(),
                folded.line,
            )
        })
        .collect();
    assert_eq!(folded, [("Local::~Local", 3), ("Local::operator=", 3)]);
    assert_eq!(repeated.symbols.len(), 2);
    assert_eq!(keys(&repeated), Vec::<&str>::new());
}

#[test]
fn only_an_in_class_prototype_folds_into_its_out_of_line_definition() {
    let raw = extract(concat!(
        "namespace n { struct W { void f(); }; }\n",
        "void n::W::f();\n",
        "void n::W::f() {}\n",
    ));

    let folded_lines: Vec<u32> = raw
        .folded_declarations
        .iter()
        .map(|folded| folded.line)
        .collect();
    assert_eq!(folded_lines, [1]);
}

#[test]
fn file_local_identity_does_not_hide_an_external_declaration() {
    let raw = extract(concat!(
        "namespace { void run(int value); void run(int input) {} }\n",
        "void run(int value);\n",
    ));

    let symbols: Vec<(&str, u32, bool)> = raw
        .symbols
        .iter()
        .map(|symbol| (symbol.name.as_str(), symbol.line, symbol.exported))
        .collect();
    assert_eq!(symbols, [("run", 1, false), ("run", 2, true)]);
    assert_eq!(
        keys_with_roles(&raw),
        [("run(int)", CallableRole::Declaration)]
    );
}

#[test]
fn an_extern_value_and_its_definition_share_the_key_of_their_qualified_name() {
    let header = extract(concat!(
        "extern int limit;\n",
        "namespace ns { extern const char *names[]; }\n",
        "extern int seeded = 4;\n",
        "static int hidden;\n",
    ));
    let source = extract("int limit = 4;\nnamespace ns { const char *names[] = {\"a\"}; }\n");

    assert_eq!(
        keys_with_roles(&header),
        [
            ("limit", CallableRole::Declaration),
            ("ns::names", CallableRole::Declaration),
            ("seeded", CallableRole::Definition),
        ],
        "an initializer defines what `extern` declares, and a file-local value links to nothing"
    );
    assert_eq!(
        keys_with_roles(&source),
        [
            ("limit", CallableRole::Definition),
            ("ns::names", CallableRole::Definition)
        ]
    );
}

#[test]
fn unsupported_prototypes_are_preserved_instead_of_merged_by_name() {
    let raw = extract(concat!(
        "void callback(void (*fn)(int));\nvoid callback(void (*fn)(int)) {}\n",
        "void callback(void (*fn)(float));\nvoid callback(void (*fn)(float));\n",
    ));

    let signatures: Vec<Option<&str>> = raw
        .symbols
        .iter()
        .map(|symbol| symbol.signature.as_deref())
        .collect();
    assert_eq!(
        signatures,
        [
            Some("void callback(void (*fn)(int))"),
            Some("void callback(void (*fn)(int))"),
            Some("void callback(void (*fn)(float))"),
            Some("void callback(void (*fn)(float))"),
        ]
    );
    assert_eq!(keys(&raw), Vec::<&str>::new());
}

#[test]
fn anonymous_owners_do_not_acquire_free_function_identities() {
    let raw = extract(concat!(
        "class { public: void run(int value) {} } first;\n",
        "struct { void run(int value) {} } second;\n",
        "union { void run(int value) {} } third;\n",
    ));

    assert_eq!(
        names(&raw),
        ["first", "run", "second", "run", "third", "run"]
    );
    assert_eq!(
        keys(&raw),
        ["first", "second", "third"],
        "only the values of the anonymous types take a key"
    );
}

#[test]
fn a_sole_void_parameter_is_an_empty_parameter_list() {
    let declared = extract("void flush(void);");
    let defined = extract("void flush() {}");
    let both = extract("void flush(void);\nvoid flush() {}");

    assert_eq!(keys(&declared), ["flush()"]);
    assert_eq!(keys(&defined), ["flush()"]);
    assert_eq!(names(&both), ["flush"]);
}

#[test]
fn by_value_cv_does_not_change_callable_identity() {
    for (declared, defined, expected) in [
        ("const volatile int", "int", "run(int)"),
        ("int *const volatile", "int *", "run(int *)"),
        ("int *const *const", "int *const *", "run(int * const *)"),
        ("const int *const", "const int *", "run(const int *)"),
    ] {
        let declaration = extract(&format!("void run({declared} value);"));
        let definition = extract(&format!("void run({defined} input) {{}}"));

        assert_eq!(keys(&declaration), [expected], "{declared}");
        assert_eq!(keys(&definition), [expected], "{defined}");
    }
}

#[test]
fn namespace_wrapped_and_root_qualified_definitions_share_the_key() {
    let header = extract("namespace ns { struct Widget { void run(int value); }; }");
    assert_eq!(keys(&header), ["ns::Widget::run(int)"]);

    for definition in [
        "namespace ns { void Widget::run(int input) {} }",
        "namespace ns { void ::ns::Widget::run(int input) {} }",
        "void ::ns::Widget::run(int input) {}",
    ] {
        assert_eq!(
            keys(&extract(definition)),
            ["ns::Widget::run(int)"],
            "{definition}"
        );
    }
}

#[test]
fn a_relative_namespace_name_that_can_shadow_an_enclosing_scope_is_not_guessed() {
    for source in [
        "namespace ns { struct Widget { void run(int); }; void ns::Widget::run(int) {} }",
        concat!(
            "namespace ns { struct Widget { void run(int); }; ",
            "namespace ns { struct Widget { void run(int); }; } ",
            "void ns::Widget::run(int) {} }",
        ),
    ] {
        let raw = extract(source);

        let definitions: Vec<&str> = raw
            .callables
            .iter()
            .filter(|callable| callable.role == CallableRole::Definition)
            .map(|callable| callable.key.as_str())
            .collect();
        assert_eq!(definitions, Vec::<&str>::new(), "{source}");
        assert!(names(&raw).contains(&"run"), "{source}");
    }
}

#[test]
fn variadic_markers_distinguish_callable_identities() {
    let header = extract("void run(int value, ...); void run(int value); void run(...);");
    let source = extract("void run(int input, ...) {} void run(int input) {} void run(...) {}");

    assert_eq!(keys(&header), ["run(int,...)", "run(int)", "run(...)"]);
    assert_eq!(keys(&source), ["run(int,...)", "run(int)", "run(...)"]);
}

#[test]
fn pure_virtual_declarations_share_the_key_of_their_out_of_line_definitions() {
    let header = extract(
        "struct Widget { virtual int run(int value) = 0; int header_user() { return run(1); } };",
    );
    let source = extract(concat!(
        "#include \"api.h\"\nint Widget::run(int input) { return 0; }\n",
        "int source_user(Widget &object) { return object.run(2); }",
    ));

    let run_identity = |raw: &RawExtraction| -> Vec<(String, CallableRole)> {
        raw.callables
            .iter()
            .filter(|callable| raw.symbols[callable.symbol.0 as usize].name == "run")
            .map(|callable| (callable.key.clone(), callable.role))
            .collect()
    };
    assert_eq!(
        run_identity(&header),
        [("Widget::run(int)".to_owned(), CallableRole::Declaration)]
    );
    assert_eq!(
        run_identity(&source),
        [("Widget::run(int)".to_owned(), CallableRole::Definition)]
    );
}

#[test]
fn nested_type_qualifiers_do_not_split_callable_identity() {
    for (declared, defined, expected) in [
        (
            "int *const volatile *",
            "int *volatile const *",
            "run(int * const volatile *)",
        ),
        (
            "Wrapper<const volatile int *> &",
            "Wrapper<int volatile const *> &",
            "run(Wrapper < const volatile int * > &)",
        ),
    ] {
        let declaration = extract(&format!("void run({declared} value);"));
        let definition = extract(&format!("void run({defined} input) {{}}"));

        assert_eq!(keys(&declaration), [expected], "{declared}");
        assert_eq!(keys(&definition), [expected], "{defined}");
    }
}

#[test]
fn member_cv_order_does_not_change_callable_identity() {
    let header = extract("struct Widget { void run() const volatile &; };");
    let source = extract("void Widget::run() volatile const & {}");

    assert_eq!(keys(&header), ["Widget::run()const volatile &"]);
    assert_eq!(keys(&source), ["Widget::run()const volatile &"]);
}

#[test]
fn a_file_scope_lambda_does_not_export_its_local_class_identity() {
    let raw = extract("auto make = [] { struct Local { void run(int) {} }; return 0; };");

    assert_eq!(qualified_names(&raw), ["make", "Local", "Local::run"]);
    assert_eq!(
        keys(&raw),
        ["make"],
        "only the value the lambda initializes takes a key"
    );
}

#[test]
fn virtual_specifiers_do_not_change_callable_identity() {
    let header = extract(concat!(
        "struct Base { virtual void run(int) noexcept = 0; };\n",
        "struct Widget : Base { void run(int value) noexcept override final; };",
    ));
    let source = extract("void Widget::run(int input) noexcept {}");

    assert_eq!(
        keys_with_roles(&header),
        [
            ("Base::run(int)", CallableRole::Declaration),
            ("Widget::run(int)", CallableRole::Declaration)
        ]
    );
    assert_eq!(
        keys_with_roles(&source),
        [("Widget::run(int)", CallableRole::Definition)]
    );
}

#[test]
fn local_and_unsupported_identities_are_not_guessed() {
    let raw = extract(
        r"
static void local(int value) {}
namespace { void hidden(int value) {} }
template<class T> void generic(T value) {}
void callback(void (*value)(int)) {}
void outer() { struct Local { void run(int value) {} }; }
",
    );

    assert_eq!(keys(&raw), ["outer()"]);
}
