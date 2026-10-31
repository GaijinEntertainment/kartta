//! What a PHP type names as a parent: the class name PHP compiles the written name to, through the file's `use` map and
//! its namespace.
#![expect(
    clippy::expect_used,
    reason = "clippy's allow-expect-in-tests only covers #[cfg(test)] modules, and the helpers of an integration test \
              crate are neither; a missing symbol is the assertion"
)]

use kartta_lang::Language;
use kartta_lang_php::Php;

fn parents_of(source: &str, name: &str) -> Vec<String> {
    Php::new()
        .extract(source.as_bytes())
        .symbols
        .into_iter()
        .find(|symbol| symbol.name == name)
        .expect("the type is declared")
        .parents
}

#[test]
fn an_alias_names_the_class_its_use_imports() {
    let source = "<?php\nnamespace N;\nuse A\\B as C;\nclass D extends C {}\n";

    assert_eq!(parents_of(source, "D"), ["A\\B"]);
}

#[test]
fn a_plain_use_names_the_class_it_imports() {
    let source = "<?php\nnamespace N;\nuse A\\B;\nclass D extends B {}\n";

    assert_eq!(parents_of(source, "D"), ["A\\B"]);
}

#[test]
fn a_grouped_use_names_the_class_under_its_prefix() {
    let source = "<?php\nnamespace N;\nuse A\\{B, C\\D as E};\nclass F extends E implements B {}\n";

    assert_eq!(parents_of(source, "F"), ["A\\C\\D", "A\\B"]);
}

#[test]
fn a_use_written_from_the_root_names_the_class_without_the_root() {
    let source = "<?php\nnamespace N;\nuse \\A\\B as C;\nclass D extends C {}\n";

    assert_eq!(parents_of(source, "D"), ["A\\B"]);
}

#[test]
fn an_alias_replaces_the_first_segment_of_a_qualified_name() {
    let source = "<?php\nnamespace N;\nuse A\\B as C;\nclass D extends C\\E {}\n";

    assert_eq!(parents_of(source, "D"), ["A\\B\\E"]);
}

#[test]
fn an_alias_reads_in_any_case() {
    let source = "<?php\nnamespace N;\nuse A\\B as Cee;\nclass D extends CEE {}\n";

    assert_eq!(parents_of(source, "D"), ["A\\B"]);
}

#[test]
fn a_name_no_use_imports_is_in_the_file_s_namespace() {
    let source = "<?php\nnamespace N\\M;\nclass D extends Base {}\nclass E extends Sub\\Base {}\n";

    assert_eq!(parents_of(source, "D"), ["N\\M\\Base"]);
    assert_eq!(parents_of(source, "E"), ["N\\M\\Sub\\Base"]);
}

#[test]
fn a_name_outside_any_namespace_is_global() {
    let source = "<?php\nclass D extends Base {}\n";

    assert_eq!(parents_of(source, "D"), ["Base"]);
}

#[test]
fn a_fully_qualified_name_takes_neither_use_nor_namespace() {
    let source =
        "<?php\nnamespace N;\nuse A\\B as C;\nclass D extends \\C {}\nclass E extends \\X\\Y {}\n";

    assert_eq!(parents_of(source, "D"), ["C"]);
    assert_eq!(parents_of(source, "E"), ["X\\Y"]);
}

#[test]
fn a_relative_name_is_in_the_file_s_namespace_past_any_use() {
    let source = "<?php\nnamespace N;\nuse A\\Sub;\nclass D extends namespace\\Sub {}\n";

    assert_eq!(parents_of(source, "D"), ["N\\Sub"]);
}

#[test]
fn a_function_or_const_use_imports_no_class() {
    let source = "<?php\nnamespace N;\nuse function A\\f as C;\nuse const A\\X as E;\nclass D extends C {}\n\
                  class G extends E {}\n";

    assert_eq!(parents_of(source, "D"), ["N\\C"]);
    assert_eq!(parents_of(source, "G"), ["N\\E"]);
}

#[test]
fn a_mixed_group_imports_a_class_only_from_its_class_clause() {
    let source = "<?php\nnamespace N;\nuse A\\{function f as C, B as E};\nclass D extends C {}\n\
                  class G extends E {}\n";

    assert_eq!(parents_of(source, "D"), ["N\\C"]);
    assert_eq!(parents_of(source, "G"), ["A\\B"]);
}

#[test]
fn a_namespace_declaration_ends_the_uses_before_it() {
    let source = "<?php\nnamespace N { use A\\B as C; class D extends C {} }\n\
                  namespace M { class E extends C {} }\n";

    assert_eq!(parents_of(source, "D"), ["A\\B"]);
    assert_eq!(parents_of(source, "E"), ["M\\C"]);
}

#[test]
fn a_use_reaches_only_the_declarations_after_it() {
    let source =
        "<?php\nnamespace N;\nclass D extends C {}\nuse A\\B as C;\nclass E extends C {}\n";

    assert_eq!(parents_of(source, "D"), ["N\\C"]);
    assert_eq!(parents_of(source, "E"), ["A\\B"]);
}

#[test]
fn every_parent_form_reads_the_use_map() {
    let source = "<?php\nnamespace N;\nuse A\\I as J;\nuse A\\T as U;\nuse A\\K as L;\n\
                  interface M extends J {}\n\
                  enum P implements J {}\n\
                  class Q { use U; }\n\
                  $o = new class extends L implements J { use U; };\n";

    assert_eq!(parents_of(source, "M"), ["A\\I"]);
    assert_eq!(parents_of(source, "P"), ["A\\I"]);
    assert_eq!(parents_of(source, "Q"), ["A\\T"]);
    assert_eq!(parents_of(source, "{o}"), ["A\\K", "A\\I", "A\\T"]);
}

#[test]
fn a_trait_used_twice_under_two_spellings_is_one_parent() {
    let source = "<?php\nnamespace N;\nuse A\\T as U;\nclass Q { use U; use \\A\\T; }\n";

    assert_eq!(parents_of(source, "Q"), ["A\\T"]);
}
