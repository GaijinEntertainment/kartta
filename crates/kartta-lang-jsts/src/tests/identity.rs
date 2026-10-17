use kartta_lang::{CallableRole, Extraction, Language};

use super::{Files, extracted};
use crate::TypeScript;

fn identities(source: &str) -> Vec<(String, Option<String>)> {
    (extracted("api.ts", source).symbols.into_iter())
        .map(|symbol| (symbol.qualified_name, symbol.owner))
        .collect()
}

fn identity(qualified_name: &str, owner: Option<&str>) -> (String, Option<String>) {
    (qualified_name.to_owned(), owner.map(str::to_owned))
}

fn callables(extraction: &Extraction) -> Vec<(u32, &str, CallableRole)> {
    (extraction.callables.iter())
        .map(|callable| (callable.symbol.0, callable.key.as_str(), callable.role))
        .collect()
}

const OVERLOADED: &str = "\
export function run(x: number): number;
export function run(x: string): string;
export function run(x: number | string): number | string {
  return x;
}
export function consume() { return run(1); }
";

#[test]
fn every_member_kind_takes_the_chain_of_its_owner() {
    let got = identities(
        "\
namespace Outer {
  export namespace Inner {
    export interface Shape { area(): number; onDraw: () => void; label: string }
    export abstract class Base {
      abstract size(): number;
      handler = () => {};
      count = 0;
      run() {}
    }
    export enum Color { Red }
    export type Alias = number;
  }
}
namespace Dotted.Path {
  export class Worker { run() {} }
}
declare module \"ext\" {
  export class Client { send(): void }
}
",
    );

    assert_eq!(
        got,
        [
            identity("Outer", None),
            identity("Outer.Inner", Some("Outer")),
            identity("Outer.Inner.Shape", Some("Outer.Inner")),
            identity("Outer.Inner.Shape.area", Some("Outer.Inner.Shape")),
            identity("Outer.Inner.Shape.onDraw", Some("Outer.Inner.Shape")),
            identity("Outer.Inner.Shape.label", Some("Outer.Inner.Shape")),
            identity("Outer.Inner.Base", Some("Outer.Inner")),
            identity("Outer.Inner.Base.size", Some("Outer.Inner.Base")),
            identity("Outer.Inner.Base.handler", Some("Outer.Inner.Base")),
            identity("Outer.Inner.Base.count", Some("Outer.Inner.Base")),
            identity("Outer.Inner.Base.run", Some("Outer.Inner.Base")),
            identity("Outer.Inner.Color", Some("Outer.Inner")),
            identity("Outer.Inner.Alias", Some("Outer.Inner")),
            identity("Dotted.Path", None),
            identity("Dotted.Path.Worker", Some("Dotted.Path")),
            identity("Dotted.Path.Worker.run", Some("Dotted.Path.Worker")),
            identity("ext", None),
            identity("ext.Client", Some("ext")),
            identity("ext.Client.send", Some("ext.Client")),
        ]
    );
}

#[test]
fn a_function_body_or_a_class_expression_starts_a_new_chain() {
    let got = identities(
        "\
namespace A {
  export function make() {
    class Local { run() {} }
    return new Local();
  }
  export const Impl = class Named { run() {} };
  export const Anon = class { run() {} };
}
",
    );

    assert_eq!(
        got,
        [
            identity("A", None),
            identity("A.make", Some("A")),
            identity("Local", None),
            identity("Local.run", Some("Local")),
            identity("Impl", None),
            identity("Named.run", Some("Named")),
            identity("Anon", None),
            identity("Anon.run", Some("Anon")),
        ]
    );
}

#[test]
fn a_comment_and_a_decorator_between_members_keep_the_set() {
    let extraction = extracted(
        "api.ts",
        "\
export class Api {
  run(x: number): number;
  // The catch-all.
  @logged({ level: 'debug' })
  run(x: any) { return x; }
}
/** Formats a number. */
function format(x: number): string;
/* Formats a string. */
function format(x: any) { return String(x); }
",
    );

    assert_eq!(
        callables(&extraction),
        [
            (1, "api.ts#2", CallableRole::Declaration),
            (2, "api.ts#2", CallableRole::Definition),
            (3, "api.ts#4", CallableRole::Declaration),
            (4, "api.ts#4", CallableRole::Definition),
        ]
    );
}

#[test]
fn a_generator_implementation_defines_its_set() {
    let extraction = extracted(
        "api.ts",
        "\
export function run(x: number): Generator<number>;
export function run(x: string): Generator<string>;
export function* run(x: number | string) { yield x; }
",
    );

    assert_eq!(
        callables(&extraction),
        [
            (0, "api.ts#2", CallableRole::Declaration),
            (1, "api.ts#2", CallableRole::Declaration),
            (2, "api.ts#2", CallableRole::Definition),
        ]
    );
}

#[test]
fn a_signature_inside_a_parameter_type_does_not_end_the_set() {
    let extraction = extracted(
        "api.ts",
        "export class Api {\n  run(done: { run(): void }): void;\n  run(done: any) {}\n}\n",
    );

    assert_eq!(
        callables(&extraction),
        [
            (1, "api.ts#3", CallableRole::Declaration),
            (3, "api.ts#3", CallableRole::Definition)
        ]
    );
}

#[test]
fn a_signature_of_another_name_stays_out_of_the_set() {
    let extraction = extracted(
        "api.ts",
        "\
export declare function first(x: number): number;
export function second(x: number): number;
export function second(x: any) { return x; }
",
    );

    assert_eq!(
        callables(&extraction),
        [
            (1, "api.ts#2", CallableRole::Declaration),
            (2, "api.ts#2", CallableRole::Definition)
        ]
    );
}

#[test]
fn a_signature_with_no_implementation_next_to_it_joins_nothing() {
    for source in [
        "declare namespace Legacy { function run(x: number): number; }\nexport function run(x: number) { return x; }\n",
        "type Runner = { run(x: number): number }\nexport function run(x: number) { return x; }\n",
        "interface Runner { run(x: number): number }\nclass Local implements Runner { run(x: number) { return x; } }\n",
        "abstract class Base { abstract run(x: number): number; }
class Local extends Base { run(x: number) { return x; } }\n",
        "function run(x: number): number;\nconst between = 1;\nfunction run(x: any) { return x; }\n",
        "function run(x: number): number;\nfunction walk(x: any) { return x; }\n",
        "function run(x: number): number;\nfunction* walk(x: any) { yield x; }\n",
    ] {
        let extraction = extracted("api.ts", source);
        assert_eq!(callables(&extraction), [], "{source}");
    }
}

#[test]
fn a_key_names_the_implementation_and_the_resolver_adds_the_file() {
    let language = TypeScript::new();
    let files = Files::paths(&["a/api.ts", "b/api.ts"]);
    let resolver = language.resolver(&files);
    let raw = language.extract(OVERLOADED.as_bytes());

    let raw_keys: Vec<_> = (raw.callables.iter())
        .map(|callable| (callable.symbol.0, callable.key.as_str(), callable.role))
        .collect();
    assert_eq!(
        raw_keys,
        [
            (0, "2", CallableRole::Declaration),
            (1, "2", CallableRole::Declaration),
            (2, "2", CallableRole::Definition)
        ]
    );
    for path in ["a/api.ts", "b/api.ts"] {
        let extraction = resolver.resolve(raw.clone(), path);
        let key = format!("{path}#2");
        assert_eq!(
            callables(&extraction),
            [
                (0, key.as_str(), CallableRole::Declaration),
                (1, key.as_str(), CallableRole::Declaration),
                (2, key.as_str(), CallableRole::Definition),
            ]
        );
    }
}
