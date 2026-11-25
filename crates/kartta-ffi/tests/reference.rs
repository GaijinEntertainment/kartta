//! `WIRE.md`, held to the code: a method, a field, a value or a bound the code gains fails here until the reference
//! names it, and a name the code loses fails here until the reference drops it.
//!
//! The reference's `###` sections are its objects and value sets, and a section's names are the bullets that open with
//! a backticked name. The code's side is read from `src/` with `syn`: every struct and enum that derives `Serialize` or
//! `Deserialize`, by the serde attributes that decide what the wire spells; every `match` that answers string literals,
//! none with a wildcard arm, so no map answers an existing word for a variant it cannot name; every numeric constant.
//! The methods come from `version` and the params each takes from serde's own refusal of an unknown field, so neither
//! rests on reading the source.
#![expect(
    clippy::expect_used,
    clippy::panic,
    reason = "clippy's allow-expect-in-tests only covers #[cfg(test)] modules, and an integration test crate is built \
              without cfg(test), so the workspace no-panic posture is waived here, where a failed expectation is the \
              assertion"
)]

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Path, PathBuf};

use kartta_ffi::Kartta;
use serde_json::{Value, json};
use syn::visit::{self, Visit};
use syn::{Attribute, Expr, Fields, Lit, LitStr, Token};

fn here(path: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join(path)
}

fn read(path: &Path) -> String {
    fs::read_to_string(path).unwrap_or_else(|error| panic!("{}: {error}", path.display()))
}

/// A name a section lists, and what its line says after the name.
#[derive(Debug)]
struct Row {
    name: String,
    note: String,
}

/// The reference's `###` sections by heading.
fn reference() -> BTreeMap<String, Vec<Row>> {
    let mut sections: BTreeMap<String, Vec<Row>> = BTreeMap::new();
    let mut current = None;
    for line in read(&here("WIRE.md")).lines() {
        if let Some(heading) = line.strip_prefix("### ") {
            let replaced = sections.insert(heading.to_owned(), Vec::new());
            assert!(replaced.is_none(), "two sections are named {heading}");
            current = Some(heading.to_owned());
        } else if line.starts_with("## ") {
            current = None;
        } else if let (Some(section), Some(row)) = (&current, line.strip_prefix("- `"))
            && let Some((name, note)) = row.split_once('`')
        {
            let rows = sections.get_mut(section).expect("the section just opened");
            rows.push(Row {
                name: name.to_owned(),
                note: note.to_owned(),
            });
        } else if let (Some(section), true) = (&current, line.starts_with("  "))
            && let Some(row) = sections.get_mut(section).and_then(|rows| rows.last_mut())
        {
            row.note.push(' ');
            row.note.push_str(line.trim_start());
        }
    }
    sections
}

fn names(rows: &[Row]) -> BTreeSet<&str> {
    rows.iter().map(|row| row.name.as_str()).collect()
}

/// What the wire spells of one struct or enum: each name, and for a field of a struct whether it is always there.
type Object = BTreeMap<String, Option<bool>>;

/// What `src/` puts on the wire.
#[derive(Default)]
struct Surface {
    objects: BTreeMap<String, Object>,
    value_sets: Vec<BTreeSet<String>>,
    bounds: BTreeMap<String, u64>,
    exports: BTreeSet<String>,
    defaults: BTreeMap<String, String>,
}

impl Surface {
    fn of_the_crate() -> Self {
        let mut surface = Self::default();
        let mut pending = vec![here("src")];
        while let Some(path) = pending.pop() {
            if path.is_dir() {
                let entries = fs::read_dir(&path).expect("a source directory");
                pending.extend(entries.map(|entry| entry.expect("an entry").path()));
            } else {
                surface.visit_file(&parsed(&path));
            }
        }
        surface
    }

    fn add_object(&mut self, name: &str, object: Object) {
        let replaced = self.objects.insert(name.to_owned(), object);
        assert!(replaced.is_none(), "two wire types are named {name}");
    }
}

fn parsed(path: &Path) -> syn::File {
    syn::parse_file(&read(path)).unwrap_or_else(|error| panic!("{}: {error}", path.display()))
}

/// The serde keys of `attrs` with their string values. A key this file does not read fails, so a serde attribute that
/// changes what the wire spells cannot pass unread.
fn serde_keys(attrs: &[Attribute]) -> BTreeMap<String, Option<String>> {
    const READ: [&str; 10] = [
        "default",
        "deny_unknown_fields",
        "flatten",
        "remote",
        "rename",
        "rename_all",
        "serialize_with",
        "skip_serializing_if",
        "tag",
        "with",
    ];
    let mut keys = BTreeMap::new();
    for attr in attrs.iter().filter(|attr| attr.path().is_ident("serde")) {
        let read = attr.parse_nested_meta(|meta| {
            let key = meta.path.require_ident()?.to_string();
            assert!(READ.contains(&key.as_str()), "unread serde key {key}");
            let value = if meta.input.peek(Token![=]) {
                Some(meta.value()?.parse::<LitStr>()?.value())
            } else {
                None
            };
            keys.insert(key, value);
            Ok(())
        });
        read.expect("a serde attribute");
    }
    keys
}

#[derive(Clone, Copy)]
enum Direction {
    Answer,
    Params,
}

fn direction(attrs: &[Attribute]) -> Option<Direction> {
    let mut found = None;
    for attr in attrs.iter().filter(|attr| attr.path().is_ident("derive")) {
        let read = attr.parse_nested_meta(|meta| {
            if meta.path.is_ident("Serialize") {
                found = Some(Direction::Answer);
            } else if meta.path.is_ident("Deserialize") {
                found = Some(Direction::Params);
            }
            Ok(())
        });
        read.expect("a derive list");
    }
    found
}

fn snake_case(name: &str) -> String {
    let mut snake = String::new();
    for character in name.chars() {
        if character.is_uppercase() && !snake.is_empty() {
            snake.push('_');
        }
        snake.push(character.to_ascii_lowercase());
    }
    snake
}

fn is_option(ty: &syn::Type) -> bool {
    matches!(ty, syn::Type::Path(path)
        if path.path.segments.last().is_some_and(|segment| segment.ident == "Option"))
}

fn fields_of(item: &syn::ItemStruct, direction: Direction) -> Object {
    let keys = serde_keys(&item.attrs);
    assert!(!keys.contains_key("rename_all"), "{}", item.ident);
    let defaulted = keys.contains_key("default");
    let mut object = Object::new();
    for field in &item.fields {
        let keys = serde_keys(&field.attrs);
        if keys.contains_key("flatten") {
            continue;
        }
        let always = match direction {
            Direction::Answer => !keys.contains_key("skip_serializing_if"),
            Direction::Params => {
                !defaulted && !keys.contains_key("default") && !is_option(&field.ty)
            }
        };
        let ident = field.ident.as_ref().expect("a named field");
        let name = renamed(&field.attrs).unwrap_or_else(|| ident.to_string());
        object.insert(name, Some(always));
    }
    object
}

fn variant_name(item: &syn::ItemEnum, variant: &syn::Variant) -> String {
    let ident = variant.ident.to_string();
    let rename_all = serde_keys(&item.attrs).remove("rename_all").flatten();
    renamed(&variant.attrs).unwrap_or_else(|| match rename_all.as_deref() {
        Some("snake_case") => snake_case(&ident),
        Some("lowercase") => ident.to_lowercase(),
        other => panic!("{}: unread rename_all {other:?}", item.ident),
    })
}

fn variants_of(item: &syn::ItemEnum) -> Object {
    let keys = serde_keys(&item.attrs);
    let mut object: Object = keys
        .get("tag")
        .cloned()
        .flatten()
        .into_iter()
        .map(|tag| (tag, None))
        .collect();
    for variant in &item.variants {
        object.insert(variant_name(item, variant), None);
        if let Fields::Named(fields) = &variant.fields {
            for field in &fields.named {
                let ident = field.ident.as_ref().expect("a named field");
                let name = renamed(&field.attrs).unwrap_or_else(|| ident.to_string());
                object.insert(name, None);
            }
        }
    }
    object
}

fn renamed(attrs: &[Attribute]) -> Option<String> {
    serde_keys(attrs).remove("rename").flatten()
}

fn string_of(expr: &Expr) -> Option<String> {
    let Expr::Lit(literal) = expr else {
        return None;
    };
    let Lit::Str(text) = &literal.lit else {
        return None;
    };
    Some(text.value())
}

fn integer_of(expr: &Expr) -> Option<u64> {
    match expr {
        Expr::Lit(literal) => {
            let Lit::Int(value) = &literal.lit else {
                return None;
            };
            value.base10_parse().ok()
        }
        Expr::Binary(shifted) if matches!(shifted.op, syn::BinOp::Shl(_)) => {
            Some(integer_of(&shifted.left)? << integer_of(&shifted.right)?)
        }
        _ => None,
    }
}

/// `ty` as `kartta.h` spells it.
fn c_type(ty: &syn::Type) -> String {
    match ty {
        syn::Type::Ptr(pointer) => match pointer.mutability {
            syn::PointerMutability::Const(_) => format!("const {} *", c_type(&pointer.elem)),
            syn::PointerMutability::Mut(_) => format!("{} *", c_type(&pointer.elem)),
        },
        syn::Type::Path(path) => {
            let name = path.path.segments.last().expect("a type's name");
            match name.ident.to_string().as_str() {
                "c_char" => "char".to_owned(),
                "u8" => "uint8_t".to_owned(),
                "u32" => "uint32_t".to_owned(),
                "usize" => "size_t".to_owned(),
                "Kartta" => "KarttaHandle".to_owned(),
                "KarttaString" => "KarttaString".to_owned(),
                other => panic!("kartta.h has no spelling of {other}"),
            }
        }
        _ => panic!("kartta.h has no spelling of a type the ABI takes"),
    }
}

/// `name` after a C type, which keeps a pointer's star against the name.
fn c_declarator(ty: &str, name: &str) -> String {
    let gap = if ty.ends_with('*') { "" } else { " " };
    format!("{ty}{gap}{name}")
}

/// The declaration `kartta.h` must hold of the exported function `signature`.
fn c_declaration(signature: &syn::Signature) -> String {
    let inputs: Vec<String> = signature
        .inputs
        .iter()
        .map(|input| {
            let syn::FnArg::Typed(input) = input else {
                panic!("{}: a C function takes no self", signature.ident);
            };
            let syn::Pat::Ident(name) = &*input.pat else {
                panic!("{}: a C parameter has a name", signature.ident);
            };
            c_declarator(&c_type(&input.ty), &name.ident.to_string())
        })
        .collect();
    let inputs = if inputs.is_empty() {
        "void".to_owned()
    } else {
        inputs.join(", ")
    };
    let output = match &signature.output {
        syn::ReturnType::Default => "void".to_owned(),
        syn::ReturnType::Type(_, ty) => c_type(ty),
    };
    format!(
        "{}({inputs});",
        c_declarator(&output, &signature.ident.to_string())
    )
}

impl<'ast> Visit<'ast> for Surface {
    fn visit_item_mod(&mut self, module: &'ast syn::ItemMod) {
        if module.ident != "tests" {
            visit::visit_item_mod(self, module);
        }
    }

    fn visit_item_fn(&mut self, function: &'ast syn::ItemFn) {
        if function.sig.abi.is_some() {
            self.exports.insert(c_declaration(&function.sig));
        }
        visit::visit_item_fn(self, function);
    }

    fn visit_item_struct(&mut self, item: &'ast syn::ItemStruct) {
        if let Some(direction) = direction(&item.attrs) {
            let remote = serde_keys(&item.attrs).remove("remote").flatten();
            let name = remote.unwrap_or_else(|| item.ident.to_string());
            self.add_object(&name, fields_of(item, direction));
        }
    }

    fn visit_item_enum(&mut self, item: &'ast syn::ItemEnum) {
        if direction(&item.attrs).is_some() {
            self.add_object(&item.ident.to_string(), variants_of(item));
            let is_default = |variant: &&syn::Variant| {
                variant
                    .attrs
                    .iter()
                    .any(|attr| attr.path().is_ident("default"))
            };
            if let Some(default) = item.variants.iter().find(is_default) {
                let name = variant_name(item, default);
                self.defaults.insert(item.ident.to_string(), name);
            }
        }
    }

    fn visit_item_const(&mut self, item: &'ast syn::ItemConst) {
        if let Some(value) = integer_of(&item.expr) {
            self.bounds.insert(item.ident.to_string(), value);
        }
    }

    fn visit_expr_match(&mut self, matched: &'ast syn::ExprMatch) {
        let values: BTreeSet<String> = matched
            .arms
            .iter()
            .filter_map(|arm| string_of(&arm.body))
            .collect();
        if !values.is_empty() {
            let wildcard = matched
                .arms
                .iter()
                .any(|arm| matches!(arm.pat, syn::Pat::Wild(_) | syn::Pat::Ident(_)));
            assert!(
                !wildcard,
                "a wildcard arm answers an existing word for a variant it cannot name: {values:?}"
            );
            self.value_sets.push(values);
        }
        visit::visit_expr_match(self, matched);
    }

    fn visit_expr_array(&mut self, array: &'ast syn::ExprArray) {
        let values: BTreeSet<String> = array
            .elems
            .iter()
            .filter_map(|element| match element {
                Expr::Tuple(tuple) => string_of(tuple.elems.first()?),
                _ => None,
            })
            .collect();
        if !values.is_empty() {
            self.value_sets.push(values);
        }
        visit::visit_expr_array(self, array);
    }
}

/// The value sets of the `label` functions of the engine's enums, which the wire spells through.
fn engine_value_sets() -> Vec<BTreeSet<String>> {
    struct Labels(Surface);

    impl<'ast> Visit<'ast> for Labels {
        fn visit_impl_item_fn(&mut self, function: &'ast syn::ImplItemFn) {
            if function.sig.ident == "label" {
                self.0.visit_block(&function.block);
            }
        }
    }

    let mut labels = Labels(Surface::default());
    for file in [
        "../kartta-fs/src/build.rs",
        "../kartta-fs/src/list.rs",
        "../kartta-graph/src/analyze.rs",
        "../kartta-graph/src/graph.rs",
        "../kartta-graph/src/resolution.rs",
        "../kartta-lang/src/extraction.rs",
        "../kartta-lang/src/symbol.rs",
        "../kartta/src/reading.rs",
    ] {
        labels.visit_file(&parsed(&here(file)));
    }
    labels.0.value_sets
}

/// The section each value set of the crate and the engine is, which fails naming a set no section lists.
fn value_sections(sections: &BTreeMap<String, Vec<Row>>, surface: &Surface) -> BTreeSet<String> {
    let mut sets = surface.value_sets.clone();
    sets.extend(engine_value_sets());
    sets.iter()
        .map(|values| {
            let values: BTreeSet<&str> = values.iter().map(String::as_str).collect();
            let section = sections.iter().find(|(_, rows)| names(rows) == values);
            let (name, _) =
                section.unwrap_or_else(|| panic!("no section of WIRE.md lists exactly {values:?}"));
            name.clone()
        })
        .collect()
}

fn says_always(row: &Row) -> Option<bool> {
    let always = [", always)", ", required)"].map(|mark| row.note.find(mark));
    let always = always.into_iter().flatten().min();
    let optional = row.note.find(", optional)");
    match (always, optional) {
        (Some(always), Some(optional)) => Some(always < optional),
        (Some(_), None) => Some(true),
        (None, Some(_)) => Some(false),
        (None, None) => None,
    }
}

#[test]
fn every_struct_and_enum_of_the_wire_has_a_section_that_names_each_of_its_fields() {
    let sections = reference();
    for (name, object) in Surface::of_the_crate().objects {
        let rows = sections
            .get(&name)
            .unwrap_or_else(|| panic!("WIRE.md has no section {name}"));
        let wire: BTreeSet<&str> = object.keys().map(String::as_str).collect();
        assert_eq!(names(rows), wire, "the names of section {name}");
        for row in rows {
            assert_eq!(
                says_always(row),
                object[&row.name],
                "whether {name}.{} is always there",
                row.name
            );
        }
    }
}

#[test]
fn every_value_the_wire_spells_is_named_by_a_section() {
    value_sections(&reference(), &Surface::of_the_crate());
}

#[test]
fn every_section_answers_to_a_type_or_a_value_set_the_code_holds() {
    let sections = reference();
    let surface = Surface::of_the_crate();
    let mut claimed = value_sections(&sections, &surface);
    claimed.extend(surface.objects.into_keys());
    claimed.extend(["Bounds", "Languages", "Methods"].map(str::to_owned));

    let stale: Vec<&String> = sections
        .keys()
        .filter(|name| !claimed.contains(*name))
        .collect();

    assert!(
        stale.is_empty(),
        "sections the code does not hold: {stale:?}"
    );
}

#[test]
fn a_section_marks_the_variant_its_enum_defaults_to_and_no_other() {
    let sections = reference();
    let marked: Vec<(String, String)> = sections
        .iter()
        .flat_map(|(section, rows)| rows.iter().map(move |row| (section, row)))
        .filter(|(_, row)| row.note.starts_with(" (default)"))
        .map(|(section, row)| (section.clone(), row.name.clone()))
        .collect();
    let defaults: Vec<(String, String)> = Surface::of_the_crate().defaults.into_iter().collect();

    assert_eq!(marked, defaults);
}

#[test]
fn every_numeric_constant_is_a_bound_at_its_value() {
    let sections = reference();
    let bounds = &sections["Bounds"];
    let surface = Surface::of_the_crate();
    let held: BTreeSet<&str> = surface.bounds.keys().map(String::as_str).collect();
    assert_eq!(names(bounds), held, "the names of section Bounds");

    for row in bounds {
        let written: String = row
            .note
            .trim_start_matches(": ")
            .chars()
            .take_while(|&c| c != '.')
            .collect();
        let value: u64 = written.replace(',', "").parse().expect("a bound's value");
        assert_eq!(value, surface.bounds[&row.name], "{}", row.name);
    }
}

#[test]
fn every_constant_the_reference_cites_is_a_bound() {
    let sections = reference();
    let bounds = names(&sections["Bounds"]);
    let text = read(&here("WIRE.md"));

    for quoted in text.split('`').skip(1).step_by(2) {
        let constant =
            quoted.len() > 2 && quoted.chars().all(|c| c.is_ascii_uppercase() || c == '_');
        assert!(!constant || bounds.contains(quoted), "{quoted} is no bound");
    }
}

/// What `version` answers of a handle on an empty tree that has built.
fn built() -> (tempfile::TempDir, Kartta, Value) {
    let tree = tempfile::tempdir().expect("a temporary directory");
    let params = json!({ "root": tree.path() }).to_string();
    let kartta = Kartta::open(params.as_bytes()).expect("the tree opens");
    let answer = kartta.call("build", br#"{"languages":[]}"#);
    let answer: Value = serde_json::from_slice(&answer).expect("JSON");
    assert!(answer.get("ok").is_some(), "{answer}");
    let version: Value = serde_json::from_slice(&kartta.call("version", b"")).expect("JSON");
    (tree, kartta, version["ok"].clone())
}

fn listed(version: &Value, key: &str) -> BTreeSet<String> {
    serde_json::from_value(version[key].clone()).expect("names")
}

/// The section a row of `Methods` links for the method's params.
fn params_section(row: &Row) -> &str {
    let (_, params) = row.note.split_once("params [`").expect("the params' link");
    let (params, _) = params.split_once('`').expect("the params' name");
    params
}

/// The fields `method` takes, read from serde's refusal of one it does not.
fn params_of(kartta: &Kartta, method: &str) -> BTreeSet<String> {
    let answer = kartta.call(method, br#"{"reference_probe": 1}"#);
    let answer: Value = serde_json::from_slice(&answer).expect("JSON");
    let message = answer["err"]["message"].as_str().unwrap_or_else(|| {
        panic!("{method} took a field it does not know: {answer}");
    });
    if message.contains("there are no fields") {
        return BTreeSet::new();
    }
    let (_, expected) = message.split_once("expected").expect("serde's list");
    expected
        .split('`')
        .skip(1)
        .step_by(2)
        .map(str::to_owned)
        .collect()
}

#[test]
fn every_method_has_a_row_whose_params_section_names_what_it_takes() {
    let sections = reference();
    let (_tree, kartta, version) = built();
    let methods = listed(&version, "methods");
    let rows = &sections["Methods"];
    assert_eq!(
        names(rows),
        methods.iter().map(String::as_str).collect(),
        "the names of section Methods"
    );

    for row in rows {
        let params = params_section(row);
        let took = params_of(&kartta, &row.name);
        assert_eq!(
            names(&sections[params]),
            took.iter().map(String::as_str).collect(),
            "{} takes {params}",
            row.name
        );
    }
}

/// The names of `section` and of the section whose fields it says it holds.
fn fields_of_section<'s>(
    sections: &'s BTreeMap<String, Vec<Row>>,
    section: &str,
) -> BTreeSet<&'s str> {
    let mut fields = names(&sections[section]);
    let text = read(&here("WIRE.md"));
    let opening = format!("### {section}\n\nThe fields of [`");
    if let Some((_, rest)) = text.split_once(&opening)
        && let Some((flattened, _)) = rest.split_once('`')
    {
        fields.extend(names(&sections[flattened]));
    }
    fields
}

fn assert_keys_within(object: &Value, allowed: &BTreeSet<&str>, method: &str) {
    let object = object.as_object().expect("an object");
    for key in object.keys() {
        assert!(
            allowed.contains(key.as_str()),
            "{method} answers {key}, which its row's sections do not name"
        );
    }
}

#[test]
fn every_method_s_row_links_the_sections_its_golden_answer_holds() {
    let sections = reference();
    for entry in fs::read_dir(here("tests/golden")).expect("the golden directory") {
        let golden: Value =
            serde_json::from_str(&read(&entry.expect("an entry").path())).expect("JSON");
        let Some(answer) = golden["answer"].get("ok") else {
            continue;
        };
        let method = golden["method"].as_str().expect("a method");
        let row = sections["Methods"]
            .iter()
            .find(|row| row.name == method)
            .expect("a row");
        let (_, linked) = row.note.split_once("answer ").expect("the answer's link");
        let (linked, _) = linked.split_once(". ").unwrap_or((linked, ""));
        let linked: Vec<&str> = linked
            .split("[`")
            .skip(1)
            .filter_map(|link| link.split_once('`').map(|(name, _)| name))
            .collect();
        let fields = |of: &[&str]| -> BTreeSet<&str> {
            of.iter()
                .flat_map(|section| fields_of_section(&sections, section))
                .collect()
        };
        if linked.first() == Some(&"Matches") {
            assert_keys_within(answer, &names(&sections["Matches"]), method);
            let mut part = fields(&linked[1..]);
            part.extend(names(&sections["Graphed"]));
            for answered in answer["matches"].as_array().expect("the parts") {
                assert_keys_within(answered, &part, method);
            }
        } else {
            assert_keys_within(answer, &fields(&linked), method);
        }
    }
}

#[test]
fn every_language_has_a_row_with_its_family() {
    let sections = reference();
    let (_tree, _kartta, version) = built();
    let rows = &sections["Languages"];
    let languages = listed(&version, "languages");
    assert_eq!(
        names(rows),
        languages.iter().map(String::as_str).collect(),
        "the names of section Languages"
    );

    for language in kartta_languages::all() {
        let row = rows
            .iter()
            .find(|row| row.name == language.name())
            .expect("a row");
        assert_eq!(
            row.note,
            format!(" (family `{}`)", language.family()),
            "{}",
            row.name
        );
    }
}

/// The text between the first two `quote`s of each of `lines`.
fn quoted_in<'t>(lines: impl Iterator<Item = &'t str>, quote: char) -> BTreeSet<&'t str> {
    lines.filter_map(|line| line.split(quote).nth(1)).collect()
}

/// A binding compares a code by the string it holds, so a code it spells otherwise never matches.
#[test]
fn the_bindings_spell_every_error_code_and_no_other() {
    let sections = reference();
    let mut codes = names(&sections["ErrorCode"]);
    codes.append(&mut names(&sections["ListErrorCode"]));
    let go = read(&here("../../bindings/go/wire/errors.go"));
    let ts = read(&here("../../bindings/node/src/wire.ts"));
    let (_, ts) = ts.split_once("export type Code =").expect("the type");
    let (ts, _) = ts.split_once(';').expect("the type's end");

    let spelled_by_go = quoted_in(go.lines().filter(|line| line.contains(" Code = ")), '"');
    let mut spelled_by_ts = quoted_in(ts.lines(), '\'');
    assert!(spelled_by_ts.remove("closed"), "the binding's own code");

    assert_eq!(spelled_by_go, codes, "the Go binding");
    assert_eq!(spelled_by_ts, codes, "the Node binding");
}

/// A cgo build links only the functions Go code names and reads each signature from the header alone, so a function
/// the library lacks and a signature that drifted pass it.
#[test]
fn the_header_declares_each_exported_function_with_its_signature_and_no_other() {
    let header = read(&here("include/kartta.h"));
    let declared: BTreeSet<String> = header
        .lines()
        .map(str::trim)
        .filter(|line| line.ends_with(");") && !line.starts_with(['*', '/']))
        .map(str::to_owned)
        .collect();

    assert_eq!(Surface::of_the_crate().exports, declared);
}

/// A consumer of the release archive holds the header and no ADR, so each promise it must rely on is the header's.
#[test]
fn the_header_states_what_a_consumer_of_the_archive_relies_on() {
    let header = read(&here("include/kartta.h"));
    let comment: Vec<&str> = header
        .lines()
        .map(|line| line.trim_start_matches([' ', '*']))
        .collect();
    let comment = comment.join(" ");

    for promise in [
        "WIRE.md",
        "ignores the keys it does not know",
        "kartta_call blocks its thread",
        "a thread the call starts, with 8 MiB of stack",
        "stack needs about 2 KiB",
        "call the cancel method with {}",
        "then join every thread",
        "A string outlives the handle",
        "the JSON null each read as {}",
    ] {
        assert!(
            comment.contains(promise),
            "the header leaves out {promise:?}"
        );
    }
}

fn collect_keys(value: &Value, keys: &mut BTreeSet<String>) {
    match value {
        Value::Object(object) => {
            for (key, value) in object {
                keys.insert(key.clone());
                collect_keys(value, keys);
            }
        }
        Value::Array(items) => items.iter().for_each(|item| collect_keys(item, keys)),
        _ => {}
    }
}

/// The goldens reach what a hand-written `Serialize` or a flattened field puts on the wire, which no derive states.
#[test]
fn every_key_of_a_golden_is_a_name_of_the_reference() {
    let sections = reference();
    let named: BTreeSet<&str> = sections.values().flat_map(|rows| names(rows)).collect();
    let mut keys = BTreeSet::new();
    for entry in fs::read_dir(here("tests/golden")).expect("the golden directory") {
        let golden: Value =
            serde_json::from_str(&read(&entry.expect("an entry").path())).expect("JSON");
        collect_keys(&golden["params"], &mut keys);
        collect_keys(&golden["answer"]["ok"], &mut keys);
        collect_keys(&golden["answer"]["err"], &mut keys);
    }

    let unnamed: Vec<&String> = keys
        .iter()
        .filter(|key| !named.contains(key.as_str()))
        .collect();

    assert!(
        unnamed.is_empty(),
        "keys WIRE.md does not name: {unnamed:?}"
    );
}

/// The keys no golden holds, because what answers them is no file of a fixture: an unreadable or removed file, a
/// symlink a Windows checkout does not make, a machine's own path, a watcher that failed. Each comes with the file
/// whose test pins it.
const PINNED_ELSEWHERE: [(&str, &str, &str); 26] = [
    ("BuildParams", "content_cache", "tests/dispatch.rs"),
    ("ContentIndexView", "unindexed", "tests/dispatch.rs"),
    ("FileView", "special", "tests/dispatch.rs"),
    ("FileView", "symlink", "tests/dispatch.rs"),
    ("FoundView", "error", "tests/dispatch.rs"),
    ("FuzzyView", "stopped_build", "src/handle.rs"),
    ("GlobView", "stopped_build", "src/handle.rs"),
    ("GrepFailureView", "message", "tests/dispatch.rs"),
    ("GrepFailureView", "path", "tests/dispatch.rs"),
    ("GrepView", "stopped_build", "src/handle.rs"),
    ("ListView", "stopped_build", "src/handle.rs"),
    ("ListedView", "target", "tests/dispatch.rs"),
    ("StatsAnswer", "content_index_failure", "tests/dispatch.rs"),
    ("StatsAnswer", "pruned_worktrees", "tests/dispatch.rs"),
    ("StatsAnswer", "stopped_build", "src/handle.rs"),
    ("StoppedView", "entries", "src/handle.rs"),
    ("StoppedView", "unvisited", "src/handle.rs"),
    ("StoredError", "file", "tests/dispatch.rs"),
    ("StoredError", "kind", "tests/dispatch.rs"),
    ("StoredError", "message", "tests/dispatch.rs"),
    ("StoredError", "path", "tests/dispatch.rs"),
    ("SymlinkView", "kind", "tests/dispatch.rs"),
    ("SymlinkView", "path", "tests/dispatch.rs"),
    ("SymlinkView", "target", "tests/dispatch.rs"),
    ("WatchView", "reason", "src/wire/build.rs"),
    ("WatchView", "restarts", "src/wire/build.rs"),
];

fn object_keys(value: &Value, objects: &mut Vec<BTreeSet<String>>) {
    match value {
        Value::Object(object) => {
            objects.push(object.keys().cloned().collect());
            object
                .values()
                .for_each(|value| object_keys(value, objects));
        }
        Value::Array(items) => items.iter().for_each(|item| object_keys(item, objects)),
        _ => {}
    }
}

/// What the golden files spell: every object's keys with the method whose golden holds it, and the keys each section
/// of params was asked with. Beside them, the structs each method's row names as its answer.
struct Spelled<'s> {
    objects: Vec<(&'s str, BTreeSet<String>)>,
    params: BTreeMap<&'s str, BTreeSet<String>>,
    answers: BTreeSet<(&'s str, &'s str)>,
}

impl<'s> Spelled<'s> {
    fn by_the_goldens(sections: &'s BTreeMap<String, Vec<Row>>) -> Self {
        let mut spelled = Self {
            objects: Vec::new(),
            params: BTreeMap::new(),
            answers: BTreeSet::new(),
        };
        for row in &sections["Methods"] {
            let (_, answer) = row.note.split_once(", answer ").expect("the answer's link");
            let answer = answer.split_once(". ").map_or(answer, |(answer, _)| answer);
            let linked = answer
                .split("[`")
                .skip(1)
                .filter_map(|part| part.split_once("`](#").map(|(name, _)| name));
            spelled
                .answers
                .extend(linked.map(|name| (row.name.as_str(), name)));
        }
        for entry in fs::read_dir(here("tests/golden")).expect("the golden directory") {
            let golden: Value =
                serde_json::from_str(&read(&entry.expect("an entry").path())).expect("JSON");
            let row = sections["Methods"]
                .iter()
                .find(|row| Some(row.name.as_str()) == golden["method"].as_str())
                .expect("a row");
            let mut objects = Vec::new();
            object_keys(&golden["params"], &mut objects);
            object_keys(&golden["answer"], &mut objects);
            let method = row.name.as_str();
            spelled
                .objects
                .extend(objects.into_iter().map(|keys| (method, keys)));
            let asked = spelled.params.entry(params_section(row)).or_default();
            collect_keys(&golden["params"], asked);
        }
        spelled
    }

    /// Whether the object `keys` of a golden of `method` is one of the struct `name`. A struct with a key it always
    /// holds is told apart by those `always` keys; one with none, only by a method whose row names it as its answer.
    fn places(
        &self,
        name: &str,
        (method, keys): &(&str, BTreeSet<String>),
        always: &BTreeSet<&String>,
    ) -> bool {
        if always.is_empty() {
            return self.answers.contains(&(*method, name));
        }
        always.iter().all(|&key| keys.contains(key))
    }

    /// Whether a golden spells `key` of the struct `name`, whose `always` keys tell its objects from another's.
    fn hold(&self, name: &str, key: &String, always: &BTreeSet<&String>) -> bool {
        if let Some(asked) = self.params.get(name) {
            return asked.contains(key);
        }
        self.objects
            .iter()
            .any(|object| object.1.contains(key) && self.places(name, object, always))
    }
}

/// A rename that reaches `WIRE.md` too fails no name check, so a key stays pinned only where an answer or a request
/// spells it. A key counts for a struct only in an object of that struct, so a name two structs share does not pin the
/// one no golden reaches; a struct of params has its method's requests alone to read.
#[test]
fn every_key_of_the_wire_is_in_a_golden_or_listed_with_the_test_that_pins_it() {
    let sections = reference();
    let spelled = Spelled::by_the_goldens(&sections);
    let mut unpinned = BTreeSet::new();
    for (name, object) in Surface::of_the_crate().objects {
        let always: BTreeSet<&String> = object
            .iter()
            .filter(|(_, always)| **always == Some(true))
            .map(|(key, _)| key)
            .collect();
        for key in object
            .iter()
            .filter_map(|(key, always)| always.map(|_| key))
        {
            if !spelled.hold(&name, key, &always) {
                unpinned.insert((name.clone(), key.clone()));
            }
        }
    }
    let listed: BTreeSet<(String, String)> = PINNED_ELSEWHERE
        .iter()
        .map(|&(object, key, _)| (object.to_owned(), key.to_owned()))
        .collect();

    assert_eq!(unpinned, listed);
}

/// The Go and the Node type of each object [`PINNED_ELSEWHERE`] names.
const BINDING_TYPES: [(&str, &str, &str); 15] = [
    ("BuildParams", "BuildOptions", "BuildParams"),
    ("ContentIndexView", "ContentIndexStats", "ContentIndexStats"),
    ("FileView", "fileState", "FileState"),
    ("FoundView", "Found", "Found"),
    ("FuzzyView", "Fuzzy", "Fuzzy"),
    ("GlobView", "Glob", "Glob"),
    ("GrepFailureView", "GrepFailure", "Grep"),
    ("GrepView", "Grep", "Grep"),
    ("ListView", "Listing", "Listing"),
    ("ListedView", "Listed", "Listed"),
    ("StatsAnswer", "Stats", "Stats"),
    ("StoppedView", "StoppedBuild", "StoppedBuild"),
    ("StoredError", "BuildFailure", "BuildFailure"),
    ("SymlinkView", "Symlink", "Symlink"),
    ("WatchView", "Watch", "Watch"),
];

/// The text of `source` from `opening` to the first closing brace at the start of a line.
fn type_body<'s>(source: &'s str, opening: &str) -> &'s str {
    let start = source
        .find(opening)
        .unwrap_or_else(|| panic!("no {opening}"));
    let body = &source[start..];
    &body[..body.find("\n}").expect("a closing brace")]
}

/// A listed key meets no binding's type through a golden, so its spelling is compared with theirs here.
#[test]
fn every_key_pinned_elsewhere_is_in_the_file_that_pins_it_and_in_the_bindings() {
    let mut go = String::new();
    for entry in fs::read_dir(here("../../bindings/go/wire")).expect("the Go wire package") {
        go.push_str(&read(&entry.expect("an entry").path()));
    }
    let ts = read(&here("../../bindings/node/src/wire.ts"));

    for (object, key, file) in PINNED_ELSEWHERE {
        let quoted = format!("\"{key}\"");
        assert!(
            read(&here(file)).contains(&quoted),
            "{file} pins {object}.{key}"
        );
        let &(_, go_type, ts_type) = BINDING_TYPES
            .iter()
            .find(|(named, ..)| *named == object)
            .unwrap_or_else(|| panic!("{object} names its binding types"));
        let go_body = type_body(&go, &format!("type {go_type} struct {{"));
        assert!(
            ["\"", ","]
                .iter()
                .any(|after| go_body.contains(&format!("json:\"{key}{after}"))),
            "Go's {go_type} spells {object}.{key}"
        );
        let ts_body = type_body(&ts, &format!("export type {ts_type} = {{"));
        assert!(
            ["\t", " ", "{"]
                .iter()
                .any(|before| ts_body.contains(&format!("{before}{key}:"))
                    || ts_body.contains(&format!("{before}{key}?:"))),
            "Node's {ts_type} spells {object}.{key}"
        );
    }
}
