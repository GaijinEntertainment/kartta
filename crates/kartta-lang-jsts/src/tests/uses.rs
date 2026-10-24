use kartta_lang::{Access, Extraction, Language, Receiver, SymbolId, TypeUses};

use super::{extracted, names};
use crate::TypeScript;

const SAMPLE: &str = r"import { LIMIT, Config } from './config';
const DEFAULTS: Config = { timeout: 5 };
let hits = 0;
export class Pool extends Base {
    size = 0;
    constructor(cfg: Config) {
        this.cfg = cfg;
        this.size += 1;
    }
    grow(by: number, other: Pool) {
        const local = new Pool(DEFAULTS);
        hits++;
        const cb = () => { this.size = by; };
        function plain() { return this.size; }
        other.size = LIMIT;
        const opts: Config = { timeout: by };
        return local.size + other.cfg.timeout;
    }
}
";

fn uses(extraction: &Extraction) -> Vec<(&str, &str, Receiver, Access, u32)> {
    extraction
        .uses
        .iter()
        .map(|site| {
            (
                extraction.symbols[site.site_of.0 as usize].name.as_str(),
                site.name.as_str(),
                site.receiver.clone(),
                site.access,
                site.line,
            )
        })
        .collect()
}

fn path(written: &str) -> Receiver {
    Receiver::Path(written.to_owned())
}

#[test]
fn a_member_use_names_its_class_through_this_an_annotation_or_a_constructor() {
    let extraction = extracted("src/pool.ts", SAMPLE);

    assert_eq!(
        uses(&extraction),
        [
            ("constructor", "cfg", Receiver::Own, Access::Write, 7),
            ("constructor", "size", Receiver::Own, Access::Update, 8),
            ("grow", "DEFAULTS", Receiver::Bare, Access::Read, 11),
            ("grow", "hits", Receiver::Bare, Access::Update, 12),
            ("cb", "size", path("Pool"), Access::Write, 13),
            ("plain", "size", path(""), Access::Read, 14),
            ("grow", "size", path("Pool"), Access::Write, 15),
            ("grow", "LIMIT", Receiver::Bare, Access::Read, 15),
            ("grow", "timeout", path("Config"), Access::Write, 16),
            ("grow", "size", path("Pool"), Access::Read, 17),
            ("grow", "timeout", path(""), Access::Read, 17),
            ("grow", "cfg", path("Pool"), Access::Read, 17),
        ]
    );
}

#[test]
fn a_member_use_names_the_type_a_parameter_s_context_spells() {
    let extraction = extracted(
        "src/replay.ts",
        "type Replay = (pool: Pool) => number;\nconst size: Replay = (pool) => pool.size;\n",
    );

    assert_eq!(
        uses(&extraction),
        [("size", "size", path("Pool"), Access::Read, 2)]
    );
}

#[test]
fn a_shorthand_a_default_and_an_early_use_read_a_module_value_and_a_cast_literal_writes() {
    let extraction = extracted(
        "src/limits.ts",
        "function apply(by: number = LIMIT, { cap = LIMIT } = {}) {\n    count = by;\n    \
         return { LIMIT, LATER, t: opts.timeout };\n}\nfunction typed() {\n    \
         const a = { \"timeout\": 1 } as Config;\n    return { timeout: 2 } satisfies Config;\n}\nlet count = 0;\n\
         const LIMIT = 5;\nexport const LATER = 6;\nconst opts: Config = { timeout: 3 };\n",
    );

    assert_eq!(
        uses(&extraction),
        [
            ("apply", "LIMIT", Receiver::Bare, Access::Read, 1),
            ("apply", "LIMIT", Receiver::Bare, Access::Read, 1),
            ("apply", "count", Receiver::Bare, Access::Write, 2),
            ("apply", "LIMIT", Receiver::Bare, Access::Read, 3),
            ("apply", "LATER", Receiver::Bare, Access::Read, 3),
            ("apply", "timeout", path("Config"), Access::Read, 3),
            ("apply", "opts", Receiver::Bare, Access::Read, 3),
            ("typed", "timeout", path("Config"), Access::Write, 6),
            ("typed", "timeout", path("Config"), Access::Write, 7),
        ]
    );
}

#[test]
fn a_default_parameter_value_of_a_script_reads_a_module_value() {
    let extraction = extracted(
        "src/limits.js",
        "function apply(by = LIMIT) {\n    const { cap, limit: [first] } = by;\n    \
         ({ prop: count, cap: LIMIT } = by);\n    [first, ...rest] = by;\n    return { cap, first };\n}\n\
         const LIMIT = 5;\nlet count = 0, first = 0, rest = [];\n",
    );

    assert_eq!(
        uses(&extraction),
        [
            ("apply", "LIMIT", Receiver::Bare, Access::Read, 1),
            ("apply", "count", Receiver::Bare, Access::Write, 3),
            ("apply", "LIMIT", Receiver::Bare, Access::Write, 3),
            ("apply", "rest", Receiver::Bare, Access::Write, 4),
        ],
        "a destructuring assignment sets the module values it names, and a local one hides them"
    );
}

#[test]
fn a_called_member_a_local_and_a_use_outside_every_definition_record_no_use() {
    let extraction = extracted(
        "src/app.js",
        "import api from './api';\nconst state = { ready: false };\nstate.ready = true;\n\
         function boot(config) {\n    api.start(config.port);\n    let state = 1;\n    return api.ready && state;\n}\n",
    );

    assert_eq!(
        uses(&extraction),
        [
            ("boot", "api", Receiver::Bare, Access::Read, 5),
            ("boot", "port", path(""), Access::Read, 5),
            ("boot", "ready", path("api"), Access::Read, 7),
            ("boot", "api", Receiver::Bare, Access::Read, 7),
        ]
    );
}

#[test]
fn a_variable_or_a_constant_a_body_declares_is_local_to_it_and_a_call_of_its_name_calls_it() {
    let extraction = extracted(
        "src/load.ts",
        "const limit = 2;\n\
         function f() {\n    const resp = g();\n    let run = make(), Box = class {};\n    \
         const cfg: Config = load();\n    const done = () => resp.ok;\n    run();\n    new Box();\n    \
         return resp.ok && limit && cfg.port;\n}\n",
    );

    let locals: Vec<&str> = (extraction.local_symbols.iter())
        .map(|id| extraction.symbols[id.0 as usize].name.as_str())
        .collect();
    assert_eq!(
        locals,
        ["resp", "run", "cfg"],
        "a function or a class value is no local, nor is the module's constant"
    );
    let typed: Vec<_> = (uses(&extraction).into_iter())
        .filter(|&(_, name, ..)| name == "port")
        .collect();
    assert_eq!(
        typed,
        [("f", "port", path("Config"), Access::Read, 9)],
        "a member of a local names the type its declaration spells"
    );
    let local_calls: Vec<(&str, &str, u32)> = (extraction.local_calls.iter())
        .map(|call| {
            let name = |id: SymbolId| extraction.symbols[id.0 as usize].name.as_str();
            (name(call.site_of), name(call.local), call.line)
        })
        .collect();
    assert_eq!(local_calls, [("f", "run", 7)]);
    assert!(!(extraction.calls.iter()).any(|call| call.callee == "run"));
    let reads: Vec<&str> = (uses(&extraction).into_iter())
        .filter(|(_, _, receiver, ..)| *receiver == Receiver::Bare)
        .map(|(_, name, ..)| name)
        .collect();
    assert_eq!(reads, ["limit"], "no use binds a local");
}

#[test]
fn a_call_before_a_local_s_declaration_or_after_a_var_s_block_calls_the_local() {
    let extraction = extracted(
        "src/later.js",
        "function outer() {\n    const wrap = () => later();\n    if (ready) { var held = make(); }\n    \
         held();\n    const later = make();\n    function inner(later) { later(); }\n    \
         { const later = make(); }\n}\nfunction later() {}\nfunction after() { later(); }\n",
    );

    let local_calls: Vec<(&str, &str, u32)> = (extraction.local_calls.iter())
        .map(|call| {
            let name = |id: SymbolId| extraction.symbols[id.0 as usize].name.as_str();
            let local = &extraction.symbols[call.local.0 as usize];
            (name(call.site_of), local.name.as_str(), local.line)
        })
        .collect();
    assert_eq!(
        local_calls,
        [("wrap", "later", 5), ("outer", "held", 3)],
        "a closure calls the later local of its scope, and a `var` is its function's"
    );
    let calls: Vec<(&str, u32)> = (extraction.calls.iter())
        .filter(|call| call.receiver.is_none())
        .map(|call| (call.callee.as_str(), call.line))
        .collect();
    assert_eq!(
        calls,
        [
            ("make", 3),
            ("make", 5),
            ("later", 6),
            ("make", 7),
            ("later", 10)
        ],
        "a parameter and a block's local hide nothing outside their scope"
    );
}

#[test]
fn a_function_or_a_class_the_module_declares_taken_as_a_value_is_used() {
    let extraction = extracted(
        "src/jobs.ts",
        "export function onDone() {}\n\
         function schedule() {\n    setTimeout(tick, 10);\n    register(Store, onDone, ids);\n    return run();\n}\n\
         function tick() { schedule(); }\n\
         class Store { static open() {} }\n\
         function run() { return Store.open(); }\n\
         function* ids() {}\n\
         function local() {\n    function onDone() {}\n    class Store {}\n    return [onDone, Store];\n}\n\
         abstract class Shape {}\n\
         function shapes() { return [Shape]; }\n",
    );

    assert_eq!(
        uses(&extraction),
        [
            ("schedule", "tick", Receiver::Bare, Access::Read, 3),
            ("schedule", "Store", Receiver::Bare, Access::Read, 4),
            ("schedule", "onDone", Receiver::Bare, Access::Read, 4),
            ("schedule", "ids", Receiver::Bare, Access::Read, 4),
            ("run", "Store", Receiver::Bare, Access::Read, 9),
            ("shapes", "Shape", Receiver::Bare, Access::Read, 17),
        ],
        "a call of the name, its declaration and a later body's call record no use; a value taken before the \
         declaration does; a body's own function or class of the name hides the module's"
    );
}

const BLOCKS: &str = r#"var top = 1;
try {
  var config = load();
} catch (e) {}
if (typeof window !== "undefined") {
  var root = window, { width } = root;
  function helper() { return width; }
  let scoped = 1;
  class Local {}
}
for (var key in config) {}
for (var [item] of [config]) {}
function read() {
  config.load();
  return top + config + root + helper + key + item + scoped + Local + width;
}
"#;

fn bare_reads<'e>(extraction: &'e Extraction, site: &str) -> Vec<&'e str> {
    (uses(extraction).into_iter())
        .filter(|&(of, _, ref receiver, _, _)| of == site && *receiver == Receiver::Bare)
        .map(|(_, name, ..)| name)
        .collect()
}

#[test]
fn a_var_a_top_level_block_declares_is_the_module_s_value() {
    let extraction = extracted("lib.js", BLOCKS);

    assert_eq!(
        bare_reads(&extraction, "read"),
        [
            "config", "top", "config", "root", "helper", "key", "item", "width"
        ],
        "a `let` and a class stay their block's; a script's block function is the module's"
    );
    assert_eq!(bare_reads(&extraction, "helper"), ["width"]);
}

#[test]
fn a_strict_file_keeps_a_block_s_function_to_its_block() {
    for prefix in [
        "\"use strict\";\n",
        "// a script\n'use strict';\n",
        "export {};\n",
    ] {
        let extraction = extracted("lib.js", &format!("{prefix}{BLOCKS}"));

        assert_eq!(
            bare_reads(&extraction, "read"),
            ["config", "top", "config", "root", "key", "item", "width"],
            "{prefix}"
        );
    }
}

#[test]
fn a_var_in_a_block_keeps_its_name_as_a_receiver_after_the_block_and_names_no_type_for_a_member() {
    let extraction = extracted(
        "lib.js",
        "if (ready) {\n  var config = load();\n  function early() { config.load(); }\n}\n\
         function late() { config.load(); return config.port; }\n",
    );

    let receivers: Vec<_> = (extraction.calls.iter())
        .filter(|call| call.callee == "load" && call.receiver.is_some())
        .map(|call| call.receiver.as_deref())
        .collect();
    assert_eq!(receivers, [Some(""), Some("config")]);
    let members: Vec<_> = (uses(&extraction).into_iter())
        .filter(|&(_, name, ..)| name == "port")
        .collect();
    assert_eq!(members, [("late", "port", path(""), Access::Read, 5)]);
}

#[test]
fn a_type_named_as_one_is_no_use_of_it() {
    let extraction = extracted(
        "src/shapes.ts",
        "import { Shape } from './shape';\n\
         interface Drawable { draw(): void; }\n\
         type Size = { width: number };\n\
         enum Kind { Round }\n\
         class Circle implements Drawable { size: Size; draw(): void {} }\n\
         function make<T extends Drawable>(size: Size, kind: Kind): Circle {\n    \
         const shapes: Array<Shape> = [];\n    return {} as Circle;\n}\n",
    );

    let declared = names(&extraction);
    for name in ["Drawable", "Size", "Kind", "Circle", "make"] {
        assert!(declared.contains(&name), "{name} in {declared:?}");
    }
    assert_eq!(uses(&extraction), []);
    assert_eq!(TypeScript::new().conventions().type_uses, TypeUses::Nowhere);
}
