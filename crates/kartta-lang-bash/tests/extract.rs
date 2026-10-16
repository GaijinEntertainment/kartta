//! What a Bash file extracts to from its bytes alone: its symbols, the calls its functions make, and the directives
//! its `source` commands leave for the resolver.
#![expect(
    clippy::panic,
    reason = "clippy's allow-panic-in-tests covers #[test] functions and #[cfg(test)] modules, and the lookup \
              helpers of an integration test crate are neither; a failed lookup is the assertion"
)]

use kartta_lang::{Language, NO_CALLER_ID, RawExtraction, Symbol, SymbolId, SymbolKind};
use kartta_lang_bash::Bash;

const SAMPLE: &[u8] = br#"#!/usr/bin/env bash

TOP_LEVEL="value"
export EXPORTED_VAR="x"
declare -r CONST_VAR=5
alias gs='git status'

greet() {
    local name="$1"
    echo "hello $name"
}

deploy() {
    greet "world"
    rsync -a ./src/ ./dst/
}

greet "startup"
"#;

fn extract(source: &[u8]) -> RawExtraction {
    Bash::new().extract(source)
}

fn names(raw: &RawExtraction) -> Vec<&str> {
    raw.symbols
        .iter()
        .map(|symbol| symbol.name.as_str())
        .collect()
}

fn named<'r>(raw: &'r RawExtraction, name: &str) -> &'r Symbol {
    raw.symbols
        .iter()
        .find(|symbol| symbol.name == name)
        .unwrap_or_else(|| panic!("no symbol {name}"))
}

fn id_of(raw: &RawExtraction, name: &str) -> SymbolId {
    let at = raw
        .symbols
        .iter()
        .position(|symbol| symbol.name == name)
        .unwrap_or_else(|| panic!("no symbol {name}"));
    SymbolId(u32::try_from(at).unwrap_or(u32::MAX))
}

fn specs(raw: &RawExtraction) -> Vec<&str> {
    raw.directives
        .iter()
        .map(|directive| directive.spec.as_str())
        .collect()
}

#[test]
fn top_level_globals_aliases_and_functions_are_symbols_in_source_order() {
    let raw = extract(SAMPLE);

    assert_eq!(
        names(&raw),
        [
            "TOP_LEVEL",
            "EXPORTED_VAR",
            "CONST_VAR",
            "gs",
            "greet",
            "deploy"
        ]
    );
    assert_eq!(named(&raw, "TOP_LEVEL").kind, SymbolKind::Variable);
    assert_eq!(named(&raw, "EXPORTED_VAR").kind, SymbolKind::Variable);
    let greet = named(&raw, "greet");
    assert_eq!(
        (greet.kind, greet.exported, greet.owner.as_deref()),
        (SymbolKind::Function, true, None)
    );
    assert_eq!(greet.qualified_name, "greet");
    assert!(raw.symbols.iter().all(|symbol| symbol.exported));
}

#[test]
fn an_alias_defines_a_function_and_is_no_call() {
    let raw = extract(SAMPLE);

    assert_eq!(named(&raw, "gs").kind, SymbolKind::Function);
    assert!(raw.calls.iter().all(|call| call.callee != "alias"));
}

#[test]
fn a_bare_alias_lookup_stays_a_call() {
    let raw = extract(b"run() {\n    alias ll\n}\n");

    assert!(raw.calls.iter().any(|call| call.callee == "alias"));
}

#[test]
fn a_function_local_assignment_is_no_symbol() {
    let raw = extract(SAMPLE);

    assert!(raw.symbols.iter().all(|symbol| symbol.name != "name"));
}

#[test]
fn declare_defines_a_global_only_at_the_top_of_the_script() {
    let raw =
        extract(b"declare -r TOP=1\nsetup() {\n    declare -r inner=2\n    local also=3\n}\n");

    assert_eq!(names(&raw), ["TOP", "setup"]);
}

#[test]
fn an_assignment_in_a_top_level_compound_command_is_a_global_and_a_command_prefix_is_not() {
    let raw = extract(
        b"if [ -z \"$MODE\" ]; then\n    MODE=default\nfi\nfor f in a b; do COUNT=1; done\n\
          LANG=C sort file\nrun() {\n    if true; then INNER=1; fi\n}\n",
    );

    assert_eq!(names(&raw), ["MODE", "COUNT", "run"]);
}

#[test]
fn a_command_is_a_call_from_the_function_around_it() {
    let raw = extract(SAMPLE);
    let deploy = id_of(&raw, "deploy");

    let greet = raw
        .calls
        .iter()
        .find(|call| call.callee == "greet" && call.caller_id == deploy)
        .expect("greet inside deploy");
    let rsync = raw
        .calls
        .iter()
        .find(|call| call.callee == "rsync")
        .expect("rsync");

    assert_eq!(greet.receiver, None);
    assert_eq!(rsync.caller_id, deploy);
}

#[test]
fn a_command_outside_every_function_is_made_from_file_scope() {
    let raw = extract(SAMPLE);

    assert!(
        raw.calls
            .iter()
            .any(|call| call.callee == "greet" && call.caller_id == NO_CALLER_ID)
    );
}

#[test]
fn a_command_before_the_first_definition_is_dropped() {
    let raw = extract(b"echo early\nrun() {\n    :\n}\n");

    assert!(raw.calls.iter().all(|call| call.callee != "echo"));
}

#[test]
fn a_command_substitution_opens_no_scope() {
    let raw = extract(b"build() {\n    out=$(compile src)\n}\n");
    let build = id_of(&raw, "build");

    let callers: Vec<SymbolId> = raw
        .calls
        .iter()
        .filter(|call| call.callee == "compile")
        .map(|call| call.caller_id)
        .collect();
    assert_eq!(callers, [build]);
}

#[test]
fn source_and_dot_leave_directives_and_no_calls() {
    let raw = extract(b"anchor() { :; }\nsource lib/present.sh\n. lib/absent.sh\n");

    assert_eq!(specs(&raw), ["lib/present.sh", "lib/absent.sh"]);
    assert!(
        raw.calls
            .iter()
            .all(|call| call.callee != "source" && call.callee != ".")
    );
}

#[test]
fn a_source_before_any_definition_still_leaves_its_directive() {
    let raw = extract(b"source lib/util.sh\n");

    assert_eq!(specs(&raw), ["lib/util.sh"]);
}

#[test]
fn a_literal_tail_after_an_expansion_is_salvaged() {
    let raw = extract(
        b"anchor() { :; }\nsource \"$DIR/lib/x.sh\"\nsource $ROOT/lib/y.sh\nsource 'lib/raw.sh'\n",
    );

    assert_eq!(specs(&raw), ["lib/x.sh", "lib/y.sh", "lib/raw.sh"]);
}

#[test]
fn a_tail_after_leading_expansions_names_a_file_and_an_interior_expansion_names_nothing() {
    let raw = extract(
        b"anchor() { :; }\nsource \"$DIR\"/lib/q.sh\nsource \"$A$B/lib/two.sh\"\n\
          source \"prefix$DIR/lib/x.sh\"\nsource prefix$DIR/lib/y.sh\nsource \"$DIR/a/$NAME/z.sh\"\n",
    );

    assert_eq!(specs(&raw), ["lib/q.sh", "lib/two.sh"]);
}

#[test]
fn a_specifier_computed_at_run_time_alone_names_nothing() {
    let raw =
        extract(b"anchor() { :; }\nsource $CONFIG\nsource \"$ONLY_VAR\"\nsource $'lib/c.sh'\n");

    assert_eq!(specs(&raw), Vec::<&str>::new());
    assert!(raw.calls.iter().all(|call| call.callee != "source"));
}

#[test]
fn only_a_top_level_main_function_is_the_entry_point() {
    let raw = extract(
        b"outer() {\n    main() {\n        :\n    }\n}\n\n\
          main() {\n    helper\n}\n\n\
          helper() {\n    :\n}\n",
    );
    let mains: Vec<&Symbol> = raw
        .symbols
        .iter()
        .filter(|symbol| symbol.name == "main")
        .collect();

    assert_eq!(mains.len(), 2);
    assert!(!mains[0].flags.is_entry());
    assert!(mains[1].flags.is_entry());
    assert!(!named(&raw, "helper").flags.is_entry());
}

#[test]
fn a_main_variable_or_alias_is_no_entry_point() {
    let variable = extract(b"main=\"value\"\n");
    let alias = extract(b"alias main='echo hi'\n");

    assert_eq!(named(&variable, "main").kind, SymbolKind::Variable);
    assert!(!named(&variable, "main").flags.is_entry());
    assert_eq!(named(&alias, "main").kind, SymbolKind::Function);
    assert!(!named(&alias, "main").flags.is_entry());
}

#[test]
fn a_marker_in_the_opening_comments_marks_every_symbol_generated() {
    for source in [
        b"#!/usr/bin/env bash\n# @generated by codegen\n\nrun() {\n    :\n}\n".as_slice(),
        b"#!/usr/bin/env bash\n# DO NOT EDIT - regenerate with make\n\nrun() {\n    :\n}\n",
        b"# generated by protoc\nrun() {\n    :\n}\n",
        b"\xEF\xBB\xBF# @generated\nrun() {\n    :\n}\n",
    ] {
        let raw = extract(source);

        assert_eq!(raw.symbols.len(), 1);
        assert!(raw.symbols.iter().all(|symbol| symbol.flags.is_generated()));
    }
}

#[test]
fn an_ordinary_header_a_late_marker_or_a_quoted_one_marks_nothing() {
    for source in [
        b"#!/usr/bin/env bash\n# A plain deploy script.\n\nrun() {\n    :\n}\n".as_slice(),
        b"run() {\n    # @generated\n    :\n}\n",
        b"# a `@generated` / `DO NOT EDIT` marker\nf() { :; }\n",
    ] {
        let raw = extract(source);

        assert_eq!(raw.symbols.len(), 1);
        assert!(
            raw.symbols
                .iter()
                .all(|symbol| !symbol.flags.is_generated())
        );
    }
}

#[test]
fn bash_claims_its_extensions_in_any_case() {
    let bash = Bash::new();

    assert!(bash.claims("deploy.sh"));
    assert!(bash.claims("scripts/Run.BASH"));
    assert!(!bash.claims("notes.md"));
    assert!(!bash.claims("sh"));
}

#[test]
fn the_fingerprint_names_the_crate_and_stays_put() {
    let first = Bash::new().fingerprint().to_vec();

    assert!(first.starts_with(b"kartta-lang-bash\0"));
    assert_eq!(Bash::new().fingerprint(), first.as_slice());
}
