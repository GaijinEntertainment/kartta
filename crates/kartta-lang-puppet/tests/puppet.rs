//! What a Puppet manifest extracts to, how its class references resolve against the repository, and where the doc
//! comment above a definition starts.
#![expect(
    clippy::panic,
    reason = "clippy's allow-panic-in-tests covers #[test] functions and #[cfg(test)] modules, and the lookup \
              helpers of an integration test crate are neither; a failed lookup is the assertion"
)]

use kartta_lang::{
    CallRef, Extraction, FILE_SCOPE, Language, MemoryRepository, RawExtraction, SymbolId,
    SymbolKind,
};
use kartta_lang_puppet::Puppet;

const SAMPLE: &[u8] = br"
class webserver::apache (
  String $version = 'latest',
) inherits webserver::base {
  package { 'httpd':
    ensure => $version,
  }

  include stdlib

  service { 'httpd':
    ensure => running,
  }
}

define webserver::vhost (
  String $docroot,
) {
  file { $docroot:
    ensure => directory,
  }
}

node 'web01.example.com' {
  include webserver::apache
}

function webserver::port_for(String $env) >> Integer {
  fail('nope')
}

type Port = Integer[1, 65535]

class webserver::tools {
  $config.lookup('key')
  $ports.each |$p| {
    notice($p)
  }
}
";

fn extract(source: &[u8]) -> RawExtraction {
    Puppet::new().extract(source)
}

fn resolve(source: &[u8], path: &str, files: &[&str]) -> Extraction {
    let puppet = Puppet::new();
    let repository = MemoryRepository::of_paths(files);
    let resolver = puppet.resolver(&repository);
    resolver.resolve(puppet.extract(source), path)
}

fn id_of(raw: &RawExtraction, name: &str) -> SymbolId {
    let at = raw
        .symbols
        .iter()
        .position(|symbol| symbol.name == name)
        .unwrap_or_else(|| panic!("no symbol {name}"));
    SymbolId(u32::try_from(at).unwrap_or(u32::MAX))
}

fn call<'r>(calls: &'r [CallRef], callee: &str) -> &'r CallRef {
    calls
        .iter()
        .find(|call| call.callee == callee)
        .unwrap_or_else(|| panic!("no call of {callee}"))
}

fn callees(calls: &[CallRef]) -> Vec<&str> {
    calls.iter().map(|call| call.callee.as_str()).collect()
}

fn includes(extraction: &Extraction) -> Vec<&str> {
    extraction
        .includes
        .iter()
        .map(|include| include.target.as_str())
        .collect()
}

#[test]
fn definitions_take_their_names_as_written_and_their_kinds() {
    let raw = extract(SAMPLE);

    let symbols: Vec<(&str, SymbolKind)> = raw
        .symbols
        .iter()
        .map(|symbol| (symbol.qualified_name.as_str(), symbol.kind))
        .collect();
    assert_eq!(
        symbols,
        [
            ("webserver::apache", SymbolKind::Class),
            ("webserver::vhost", SymbolKind::Function),
            ("'web01.example.com'", SymbolKind::Struct),
            ("webserver::port_for", SymbolKind::Function),
            ("Port", SymbolKind::TypeAlias),
            ("webserver::tools", SymbolKind::Class),
        ]
    );
    assert!(
        raw.symbols
            .iter()
            .all(|symbol| symbol.exported && symbol.owner.is_none())
    );
    assert_eq!(raw.symbols[0].parents, ["webserver::base"]);
}

#[test]
fn a_statement_is_a_call_of_the_definition_around_it() {
    let raw = extract(SAMPLE);
    let apache = id_of(&raw, "webserver::apache");

    for callee in ["package", "service", "stdlib"] {
        assert_eq!(call(&raw.calls, callee).site_of, apache, "{callee}");
    }
    let fail = call(&raw.calls, "fail");
    assert_eq!(fail.site_of, id_of(&raw, "webserver::port_for"));
    assert_eq!(fail.receiver, None);
    assert_eq!(
        call(&raw.calls, "webserver::apache").site_of,
        id_of(&raw, "'web01.example.com'")
    );
}

#[test]
fn a_top_scope_statement_calls_from_file_scope() {
    let resolved = resolve(
        b"include before\nclass a {}\ninclude after\n",
        "site.pp",
        &["before/manifests/init.pp", "after/manifests/init.pp"],
    );

    assert_eq!(callees(&resolved.calls), ["before", "after"]);
    assert!(resolved.calls.iter().all(|call| call.site_of == FILE_SCOPE));
    assert_eq!(
        includes(&resolved),
        ["before/manifests/init.pp", "after/manifests/init.pp"]
    );
}

#[test]
fn an_include_is_a_call_of_the_class_not_of_the_keyword() {
    let raw = extract(SAMPLE);

    let callees = callees(&raw.calls);
    assert!(callees.contains(&"stdlib"), "{callees:?}");
    assert!(callees.contains(&"webserver::apache"), "{callees:?}");
    assert!(!callees.contains(&"include"), "{callees:?}");
}

#[test]
fn a_call_on_a_value_records_the_value_as_its_receiver() {
    let raw = extract(SAMPLE);

    assert_eq!(
        call(&raw.calls, "lookup").receiver.as_deref(),
        Some("$config")
    );
    assert_eq!(call(&raw.calls, "each").receiver.as_deref(), Some("$ports"));
    assert_eq!(call(&raw.calls, "notice").receiver, None);
}

#[test]
fn a_chained_receiver_is_the_whole_head_of_the_field_expression() {
    let raw = extract(b"\nclass chained {\n  $a.b.c()\n}\n");

    assert_eq!(call(&raw.calls, "c").receiver.as_deref(), Some("$a.b"));
}

#[test]
fn a_type_reference_is_no_class_reference() {
    let resolved = resolve(
        b"\nclass deps {\n  require Class['foo']\n}\n",
        "site.pp",
        &[],
    );

    assert_eq!(resolved.calls, []);
    assert_eq!(includes(&resolved), Vec::<&str>::new());
}

#[test]
fn contain_names_its_class_in_both_forms() {
    let resolved = resolve(
        b"\nclass apache {\n  contain apache::service\n  contain('apache::config')\n}\n",
        "site.pp",
        &[
            "apache/manifests/service.pp",
            "apache/manifests/config.pp",
            "site.pp",
        ],
    );

    assert_eq!(
        callees(&resolved.calls),
        ["apache::service", "apache::config"]
    );
    assert_eq!(
        includes(&resolved),
        ["apache/manifests/service.pp", "apache/manifests/config.pp"]
    );
}

#[test]
fn a_statement_names_its_class_bare_or_quoted() {
    let resolved = resolve(
        b"class apache {\n  include apache::params, 'apache::mod'\n  require 'apache::package'\n  \
          contain 'apache::service'\n  include \"apache::${name}\"\n}\n",
        "site.pp",
        &[
            "apache/manifests/params.pp",
            "apache/manifests/mod.pp",
            "apache/manifests/package.pp",
            "apache/manifests/service.pp",
        ],
    );

    assert_eq!(
        callees(&resolved.calls),
        [
            "apache::params",
            "apache::mod",
            "apache::package",
            "apache::service"
        ]
    );
    assert_eq!(
        includes(&resolved),
        [
            "apache/manifests/params.pp",
            "apache/manifests/mod.pp",
            "apache/manifests/package.pp",
            "apache/manifests/service.pp",
        ]
    );
}

#[test]
fn an_inherited_class_is_a_parent_a_call_and_an_include() {
    let resolved = resolve(
        b"class apache::ssl inherits apache::params {\n}\n",
        "modules/apache/manifests/ssl.pp",
        &["modules/apache/manifests/params.pp"],
    );

    assert_eq!(resolved.symbols[0].parents, ["apache::params"]);
    assert_eq!(callees(&resolved.calls), ["apache::params"]);
    assert_eq!(resolved.calls[0].site_of, SymbolId(0));
    assert_eq!(includes(&resolved), ["modules/apache/manifests/params.pp"]);
}

#[test]
fn the_function_form_of_include_or_require_parses_to_nothing() {
    let resolved = resolve(
        b"class apache {\n  include('apache::mod')\n}\nclass b {\n  require(apache::mod)\n}\n",
        "site.pp",
        &["apache/manifests/mod.pp"],
    );

    assert_eq!(resolved.calls, []);
    assert_eq!(includes(&resolved), Vec::<&str>::new());
}

#[test]
fn an_unquoted_class_in_the_function_form_is_one_reference() {
    let raw = extract(b"class apache {\n  contain(apache::service)\n  contain(stdlib)\n}\n");

    assert_eq!(callees(&raw.calls), ["apache::service", "stdlib"]);
    assert_eq!(raw.directives.len(), 2);
}

#[test]
fn a_bare_module_includes_its_init_manifest() {
    let resolved = resolve(
        b"node 'n' { include foo }\n",
        "manifest.pp",
        &["modules/foo/manifests/init.pp", "manifest.pp"],
    );

    assert_eq!(includes(&resolved), ["modules/foo/manifests/init.pp"]);
    assert!(callees(&resolved.calls).contains(&"foo"));
}

#[test]
fn a_qualified_class_includes_its_nested_manifest() {
    let resolved = resolve(
        b"node 'n' { include foo::bar::baz }\n",
        "manifest.pp",
        &["modules/foo/manifests/bar/baz.pp", "manifest.pp"],
    );

    assert_eq!(includes(&resolved), ["modules/foo/manifests/bar/baz.pp"]);
    assert!(callees(&resolved.calls).contains(&"foo::bar::baz"));
}

#[test]
fn a_defined_type_resource_includes_its_manifest() {
    let resolved = resolve(
        b"node 'n' { apache::vhost { 'example.com': } }\n",
        "manifest.pp",
        &["modules/apache/manifests/vhost.pp", "manifest.pp"],
    );

    assert_eq!(includes(&resolved), ["modules/apache/manifests/vhost.pp"]);
    assert!(callees(&resolved.calls).contains(&"apache::vhost"));
}

#[test]
fn a_resource_form_class_parses_to_nothing() {
    let resolved = resolve(
        b"node 'n' { class { 'foo': } }\n",
        "manifest.pp",
        &["foo/manifests/init.pp", "manifest.pp"],
    );

    assert_eq!(includes(&resolved), Vec::<&str>::new());
    assert!(!callees(&resolved.calls).contains(&"class"));
}

#[test]
fn a_top_scope_class_drops_its_leading_colons_to_find_the_manifest() {
    let resolved = resolve(
        b"node 'n' { include ::foo }\n",
        "manifest.pp",
        &["modules/foo/manifests/init.pp", "manifest.pp"],
    );

    assert_eq!(includes(&resolved), ["modules/foo/manifests/init.pp"]);
}

#[test]
fn a_variable_class_name_resolves_nothing() {
    let resolved = resolve(
        b"node 'n' { include $var }\n",
        "manifest.pp",
        &["modules/foo/manifests/init.pp", "manifest.pp"],
    );

    assert_eq!(includes(&resolved), Vec::<&str>::new());
}

#[test]
fn a_built_in_resource_type_names_no_manifest() {
    let resolved = resolve(
        b"class c { file { '/tmp/x': ensure => present, } }\n",
        "manifest.pp",
        &["file/manifests/init.pp", "manifest.pp"],
    );

    assert!(callees(&resolved.calls).contains(&"file"));
    assert_eq!(includes(&resolved), Vec::<&str>::new());
}

#[test]
fn a_module_in_two_environments_includes_neither() {
    let resolved = resolve(
        b"node 'n' { include foo }\n",
        "manifest.pp",
        &[
            "environments/production/modules/foo/manifests/init.pp",
            "environments/staging/modules/foo/manifests/init.pp",
            "manifest.pp",
        ],
    );

    assert_eq!(includes(&resolved), Vec::<&str>::new());
    assert!(callees(&resolved.calls).contains(&"foo"));
}

#[test]
fn a_conventional_module_root_wins_over_an_environment_copy() {
    for root in ["", "modules/", "site/", "site-modules/", "dist/"] {
        let conventional = format!("{root}foo/manifests/init.pp");
        let resolved = resolve(
            b"node 'n' { include foo }\n",
            "manifest.pp",
            &[
                &conventional,
                "environments/staging/modules/foo/manifests/init.pp",
            ],
        );

        assert_eq!(includes(&resolved), [conventional.as_str()], "{root}");
    }
}

#[test]
fn a_module_in_one_environment_is_found_by_its_tail() {
    let resolved = resolve(
        b"node 'n' { include foo }\n",
        "manifest.pp",
        &[
            "environments/production/modules/foo/manifests/init.pp",
            "manifest.pp",
        ],
    );

    assert_eq!(
        includes(&resolved),
        ["environments/production/modules/foo/manifests/init.pp"]
    );
}

#[test]
fn resolution_adds_includes_and_keeps_every_call() {
    let resolved = resolve(
        SAMPLE,
        "site.pp",
        &["modules/webserver/manifests/apache.pp", "site.pp"],
    );

    assert_eq!(
        callees(&resolved.calls),
        [
            "webserver::base",
            "package",
            "stdlib",
            "service",
            "file",
            "webserver::apache",
            "fail",
            "lookup",
            "each",
            "notice",
        ]
    );
    assert_eq!(
        includes(&resolved),
        ["modules/webserver/manifests/apache.pp"]
    );
}

#[test]
fn a_test_directory_beside_manifests_marks_test_code() {
    let source = b"\nclass widget {\n  notice('hi')\n}\n";
    let files = [
        "manifests/site.pp",
        "mymod/manifests/init.pp",
        "modules/test/manifests/init.pp",
        "environments/spec/modules/foo/manifests/init.pp",
    ];

    for path in [
        "mymod/spec/fixtures/x.pp",
        "mymod/examples/init.pp",
        "mymod/tests/usage.pp",
        "mymod/test/smoke.pp",
        "test/site_smoke.pp",
    ] {
        let resolved = resolve(source, path, &files);
        assert!(!resolved.symbols.is_empty(), "{path}");
        assert!(
            resolved.symbols.iter().all(|symbol| symbol.flags.is_test()),
            "{path}"
        );
    }
    for path in [
        "mymod/manifests/init.pp",
        "mymod/manifests/test/foo.pp",
        "mymod/manifests/spec/bar.pp",
        "mymod/manifests/tests/baz.pp",
        "mymod/manifests/examples/qux.pp",
        "modules/test/manifests/init.pp",
        "spec/manifests/init.pp",
        "environments/spec/modules/foo/manifests/init.pp",
        "tools/examples/x.pp",
    ] {
        let resolved = resolve(source, path, &files);
        assert!(!resolved.symbols.is_empty(), "{path}");
        assert!(
            resolved
                .symbols
                .iter()
                .all(|symbol| !symbol.flags.is_test()),
            "{path}"
        );
    }
}

#[test]
fn a_marker_in_the_leading_comments_marks_generated_code() {
    let generated =
        extract(b"# @generated by tooling - DO NOT EDIT\n\nclass widget {\n  notice('hi')\n}\n");
    let plain = extract(b"# A plain widget module.\nclass widget {\n  notice('hi')\n}\n");
    let late = extract(b"class widget {\n  # @generated\n  notice('hi')\n}\n");
    let marked = extract(b"\xEF\xBB\xBF# @generated\nclass widget {\n  notice('hi')\n}\n");

    for generated in [generated, marked] {
        assert_eq!(generated.symbols.len(), 1);
        assert!(
            generated
                .symbols
                .iter()
                .all(|symbol| symbol.flags.is_generated())
        );
    }
    assert!(
        plain
            .symbols
            .iter()
            .all(|symbol| !symbol.flags.is_generated())
    );
    assert!(
        late.symbols
            .iter()
            .all(|symbol| !symbol.flags.is_generated())
    );
}

#[test]
fn no_symbol_is_an_entry_point() {
    let site = resolve(
        b"\nnode 'web01.example.com' {\n  include webserver::apache\n}\n",
        "manifests/site.pp",
        &[],
    );

    assert_eq!(site.symbols.len(), 1);
    assert!(site.symbols.iter().all(|symbol| !symbol.flags.is_entry()));
    assert!(
        extract(SAMPLE)
            .symbols
            .iter()
            .all(|symbol| !symbol.flags.is_entry())
    );
}

#[test]
fn a_manifest_is_claimed_by_its_extension_in_any_case() {
    let puppet = Puppet::new();

    assert!(puppet.claims("site.pp"));
    assert!(puppet.claims("modules/a/manifests/INIT.PP"));
    assert!(!puppet.claims("site.ppx"));
    assert!(!puppet.claims("pp"));
    assert_eq!(puppet.name(), "puppet");
}

#[test]
fn the_fingerprint_names_the_crate_and_is_computed_once() {
    let (one, other) = (Puppet::new(), Puppet::new());

    assert!(one.fingerprint().starts_with(b"kartta-lang-puppet "));
    assert!(std::ptr::eq(one.fingerprint(), other.fingerprint()));
}

const DOCUMENTED: &str = "# Installs nginx.
# Twice.
class nginx {
}

/*
 * Configures a site.
 */
define nginx::site() {
}
";

fn doc_line(name: &str) -> u32 {
    let lines: Vec<&[u8]> = DOCUMENTED.lines().map(str::as_bytes).collect();
    let raw = extract(DOCUMENTED.as_bytes());
    let line = raw.symbols[id_of(&raw, name).0 as usize].line;
    Puppet::new().doc_comments().first_line(&lines, line)
}

#[test]
fn hash_and_block_comments_above_a_class_or_a_define_document_it() {
    assert_eq!(doc_line("nginx"), 1);
    assert_eq!(doc_line("nginx::site"), 6);
}
