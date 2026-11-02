//! What a language crate outside this one builds its records with.

use kartta_lang::{
    Access, CallRef, Callable, CallableRole, ImplementsAssertion, ImportBinding, ImportRef,
    IncludeRef, LocalCall, RawReExport, Receiver, SymbolId, UseRef,
};

#[test]
fn a_call_states_its_site_and_leaves_the_rest_at_a_plain_call() {
    let call = CallRef::new(SymbolId(1), "f".to_owned(), Some("x".to_owned()), 7);

    assert_eq!(call.site_of, SymbolId(1));
    assert_eq!(call.callee, "f");
    assert_eq!(call.receiver.as_deref(), Some("x"));
    assert_eq!(call.line, 7);
    assert!(!call.binds_apart && !call.calls_a_method);
    assert_eq!(call.arguments, 0);
}

#[test]
fn a_use_is_a_read_until_the_language_says_otherwise() {
    let mut site = UseRef::new(SymbolId(1), "f".to_owned(), Receiver::Own, 7);
    assert_eq!(site.access, Access::Read);
    assert_eq!(site.receiver, Receiver::Own);

    site.access = Access::Write;
    assert_eq!(
        (site.site_of, site.name.as_str(), site.access, site.line),
        (SymbolId(1), "f", Access::Write, 7)
    );
}

#[test]
fn the_records_of_every_field_hold_what_they_are_given() {
    let assertion = ImplementsAssertion::new("T".to_owned(), "I".to_owned());
    assert_eq!(
        (
            assertion.type_name.as_str(),
            assertion.interface.as_str(),
            assertion.type_file,
            assertion.interface_file
        ),
        ("T", "I", None, None)
    );

    let local = LocalCall::new(SymbolId(1), SymbolId(2), 3);
    assert_eq!(
        (local.site_of, local.local, local.line),
        (SymbolId(1), SymbolId(2), 3)
    );

    assert_eq!(IncludeRef::new("a.h".to_owned()).target, "a.h");

    let import = ImportRef::new("m".to_owned(), vec!["n".to_owned()], 4);
    assert_eq!(
        (import.module.as_str(), import.names.as_slice(), import.line),
        ("m", ["n".to_owned()].as_slice(), 4)
    );

    let binding = ImportBinding::new(Some("a".to_owned()), Some("d".to_owned()), "l".to_owned());
    assert_eq!(binding.alias.as_deref(), Some("a"));
    assert_eq!(binding.target_dir.as_deref(), Some("d"));
    assert_eq!(binding.last_segment, "l");

    let export = RawReExport::new("a".to_owned(), "b".to_owned(), "c".to_owned());
    assert_eq!(
        (
            export.local_name.as_str(),
            export.source_name.as_str(),
            export.source_path.as_str()
        ),
        ("a", "b", "c")
    );

    let callable = Callable::new(SymbolId(5), "k".to_owned(), CallableRole::Definition);
    assert_eq!(callable.symbol, SymbolId(5));
    assert_eq!(callable.key, "k");
    assert_eq!(callable.role, CallableRole::Definition);
}
