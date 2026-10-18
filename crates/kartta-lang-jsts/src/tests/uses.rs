use kartta_lang::{Access, Extraction, Receiver};

use super::extracted;

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
        .references
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
