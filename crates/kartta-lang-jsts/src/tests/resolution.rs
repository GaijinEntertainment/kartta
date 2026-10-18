use kartta_lang::{Extraction, MemoryRepository};

use super::{SAMPLE_TS, imports, includes, resolved};

const APP_ALIASES: &str =
    r#"{ "compilerOptions": { "baseUrl": ".", "paths": { "@/*": ["./src/*"] } } }"#;

const PANEL_UI: &str = r#"{ "name": "@panel/ui", "exports": { ".": "./src/index.ts" } }"#;

const COMMON: &str = r#"{
    "name": "@scope/common",
    "exports": {
        ".": { "import": "./src/index.ts" },
        "./utils": "./src/utils.ts"
    }
}"#;

fn re_export_files(extraction: &Extraction) -> Vec<(&str, Option<&str>)> {
    (extraction.re_exports.iter())
        .map(|re_export| {
            (
                re_export.local_name.as_str(),
                re_export.source_file.as_deref(),
            )
        })
        .collect()
}

#[test]
fn a_hash_specifier_stops_at_the_nearest_package_even_without_imports() {
    let files = MemoryRepository::new([
        (
            "package.json",
            r##"{ "name": "root", "imports": { "#lib": "./lib/index.ts" } }"##,
        ),
        ("lib/index.ts", ""),
        ("tools/package.json", r#"{ "name": "tools" }"#),
        ("tools/main.ts", ""),
        ("main.ts", ""),
    ]);
    let source = "import { x } from \"#lib\";\n";

    let in_root = resolved("main.ts", source, &files);
    let in_tools = resolved("tools/main.ts", source, &files);

    assert_eq!(includes(&in_root), ["lib/index.ts"]);
    assert_eq!(includes(&in_tools), Vec::<&str>::new());
}

#[test]
fn the_most_specific_pattern_wins_whatever_order_the_config_lists() {
    let files = MemoryRepository::new([
        (
            "tsconfig.json",
            r#"{ "compilerOptions": { "baseUrl": ".",
                "paths": { "@/*": ["./src/*"], "@/internal/*": ["./core/*"] } } }"#,
        ),
        (
            "packages/kit/package.json",
            r#"{ "name": "kit",
                "exports": { "./features/*": "./src/*.ts", "./features/special/*": "./special/*.ts" } }"#,
        ),
        ("src/internal/log.ts", ""),
        ("core/log.ts", ""),
        ("packages/kit/src/special/flag.ts", ""),
        ("packages/kit/special/flag.ts", ""),
        ("app.ts", ""),
    ]);
    let source =
        "import { l } from \"@/internal/log\";\nimport { f } from \"kit/features/special/flag\";\n";

    let extraction = resolved("app.ts", source, &files);

    assert_eq!(
        includes(&extraction),
        ["core/log.ts", "packages/kit/special/flag.ts"]
    );
}

#[test]
fn a_types_only_export_names_the_declaration_file() {
    let files = MemoryRepository::new([
        (
            "packages/api/package.json",
            r#"{ "name": "@x/api", "exports": { ".": { "types": "./types/index.d.ts" } } }"#,
        ),
        ("packages/api/types/index.d.ts", ""),
        ("app.ts", ""),
    ]);

    let extraction = resolved("app.ts", "import type { A } from \"@x/api\";\n", &files);

    assert_eq!(includes(&extraction), ["packages/api/types/index.d.ts"]);
}

#[test]
fn a_package_with_no_entry_fields_still_takes_its_subpaths() {
    let files = MemoryRepository::new([
        ("packages/util/package.json", r#"{ "name": "@x/util" }"#),
        ("packages/util/strings.ts", ""),
        ("app.ts", ""),
    ]);

    let extraction = resolved("app.ts", "import { s } from \"@x/util/strings\";\n", &files);

    assert_eq!(includes(&extraction), ["packages/util/strings.ts"]);
}

#[test]
fn a_nearer_tsconfig_without_aliases_stops_the_lookup() {
    let files = MemoryRepository::new([
        ("tsconfig.json", APP_ALIASES),
        ("web/tsconfig.json", "{}"),
        ("src/util.ts", ""),
        ("main.ts", ""),
        ("web/app.ts", ""),
    ]);
    let source = "import { u } from \"@/util\";\n";

    let in_root = resolved("main.ts", source, &files);
    let in_web = resolved("web/app.ts", source, &files);

    assert_eq!(includes(&in_root), ["src/util.ts"]);
    assert_eq!(includes(&in_web), Vec::<&str>::new());
}

#[test]
fn root_conditions_of_an_exports_object_are_the_package_entry() {
    let files = MemoryRepository::new([
        (
            "packages/common/package.json",
            r#"{ "name": "@x/common", "exports": { "import": "./src/index.ts", "default": "./dist/index.js" } }"#,
        ),
        ("packages/common/src/index.ts", ""),
        ("app.ts", ""),
    ]);

    let extraction = resolved("app.ts", "import { c } from \"@x/common\";\n", &files);

    assert_eq!(includes(&extraction), ["packages/common/src/index.ts"]);
}

#[test]
fn a_directory_import_reaches_an_mts_or_cts_index() {
    let files = MemoryRepository::of_paths([
        "src/app.ts",
        "src/esm/index.mts",
        "src/cjs/index.cts",
        "src/esm-js/index.mjs",
        "src/cjs-js/index.cjs",
    ]);
    let source = concat!(
        "import { e } from \"./esm\";\n",
        "import { c } from \"./cjs\";\n",
        "import { m } from \"./esm-js\";\n",
        "import { r } from \"./cjs-js\";\n",
    );

    let extraction = resolved("src/app.ts", source, &files);

    assert_eq!(
        includes(&extraction),
        [
            "src/esm/index.mts",
            "src/cjs/index.cts",
            "src/esm-js/index.mjs",
            "src/cjs-js/index.cjs"
        ]
    );
}

#[test]
fn an_extensionless_import_reaches_a_declaration_file() {
    let files = MemoryRepository::of_paths(["src/app.ts", "src/types.d.ts"]);

    let extraction = resolved(
        "src/app.ts",
        "import type { T } from \"./types\";\n",
        &files,
    );

    assert_eq!(includes(&extraction), ["src/types.d.ts"]);
}

#[test]
fn a_root_package_subpath_names_a_file_at_the_root() {
    let files = MemoryRepository::new([
        ("package.json", r#"{ "name": "app", "main": "./index.ts" }"#),
        ("index.ts", ""),
        ("util.ts", ""),
        ("tools/cli.ts", ""),
    ]);

    let extraction = resolved("tools/cli.ts", "import { u } from \"app/util\";\n", &files);

    assert_eq!(includes(&extraction), ["util.ts"]);
}

#[test]
fn an_inherited_base_url_is_rooted_at_the_config_that_declares_it() {
    let files = MemoryRepository::new([
        (
            "tsconfig.base.json",
            r#"{ "compilerOptions": { "baseUrl": ".", "paths": { "@/*": ["./src/*"] } } }"#,
        ),
        (
            "packages/web/tsconfig.json",
            r#"{ "extends": "../../tsconfig.base.json" }"#,
        ),
        ("src/util.ts", ""),
        ("packages/web/src/util.ts", ""),
        ("packages/web/app.ts", ""),
    ]);

    let extraction = resolved(
        "packages/web/app.ts",
        "import { u } from \"@/util\";\n",
        &files,
    );

    assert_eq!(includes(&extraction), ["src/util.ts"]);
}

#[test]
fn imports_resolve_against_the_file_index() {
    let files = MemoryRepository::of_paths(["src/app.ts", "src/util.ts", "src/more/index.ts"]);

    let extraction = resolved("src/app.ts", SAMPLE_TS, &files);

    assert_eq!(includes(&extraction), ["src/util.ts", "src/more/index.ts"]);
    assert_eq!(imports(&extraction), ["./everything"]);
}

#[test]
fn external_import_is_recorded_as_an_importref() {
    let files = MemoryRepository::of_paths(["src/app.ts", "src/util.ts"]);
    let source = "import { useState } from \"react\";\nimport { helper } from \"./util\";\n";

    let extraction = resolved("src/app.ts", source, &files);

    let lines: Vec<u32> = extraction
        .imports
        .iter()
        .map(|import| import.line)
        .collect();
    assert_eq!(includes(&extraction), ["src/util.ts"]);
    assert_eq!(imports(&extraction), ["react"]);
    assert_eq!(lines, [1]);
}

#[test]
fn require_and_dynamic_import_resolve_includes() {
    let files = MemoryRepository::of_paths(["src/app.ts", "src/util.ts", "src/lazy.ts"]);
    let source = "function load() {\n    const u = require(\"./util\");\n    return import(\"./lazy\");\n}\n";

    let extraction = resolved("src/app.ts", source, &files);

    assert_eq!(includes(&extraction), ["src/util.ts", "src/lazy.ts"]);
}

#[test]
fn js_extension_specifiers_rewrite_to_ts_sources() {
    let files = MemoryRepository::of_paths(["src/app.ts", "src/util.ts"]);

    let extraction = resolved(
        "src/app.ts",
        "import { helper } from \"./util.js\";",
        &files,
    );

    assert_eq!(includes(&extraction), ["src/util.ts"]);
}

#[test]
fn a_relative_specifier_above_the_root_names_nothing() {
    let files = MemoryRepository::of_paths(["app.ts", "util.ts"]);

    let extraction = resolved("app.ts", "import { helper } from \"../util\";", &files);

    assert_eq!(includes(&extraction), Vec::<&str>::new());
    assert_eq!(imports(&extraction), ["../util"]);
}

#[test]
fn vite_query_suffixed_specifiers_resolve_to_the_named_file() {
    let files = MemoryRepository::of_paths([
        "src/app.ts",
        "src/icon.svg",
        "src/shader.glsl",
        "src/logo.png",
        "src/worker.ts",
        "src/od?d.ts",
    ]);
    let source = r#"
import Icon from "./icon.svg?react";
import shader from "./shader.glsl?raw";
import logo from "./logo.png?url";
import Worker from "./worker?worker&inline";
import odd from "./od?d";
import gone from "./absent.svg?react";
"#;

    let extraction = resolved("src/app.ts", source, &files);

    assert_eq!(
        includes(&extraction),
        [
            "src/icon.svg",
            "src/shader.glsl",
            "src/logo.png",
            "src/worker.ts",
            "src/od?d.ts"
        ]
    );
    assert_eq!(imports(&extraction), ["./absent.svg?react"]);
}

#[test]
fn sibling_reference_aliases_fall_through_on_a_shared_pattern() {
    let files = MemoryRepository::new([
        ("web/app.ts", ""),
        ("web/src/shared.ts", ""),
        ("web/tools/shared.ts", ""),
        ("web/tools/nodeonly.ts", ""),
        (
            "web/tsconfig.json",
            r#"{ "files": [], "references": [
                { "path": "./tsconfig.app.json" },
                { "path": "./tsconfig.node.json" }
            ] }"#,
        ),
        ("web/tsconfig.app.json", APP_ALIASES),
        (
            "web/tsconfig.node.json",
            r#"{ "compilerOptions": { "baseUrl": ".", "paths": { "@/*": ["./tools/*"] } } }"#,
        ),
    ]);
    let source = "import { shared } from \"@/shared\";\nimport { only } from \"@/nodeonly\";\n";

    let extraction = resolved("web/app.ts", source, &files);

    assert_eq!(
        includes(&extraction),
        ["web/src/shared.ts", "web/tools/nodeonly.ts"]
    );
}

#[test]
fn tsconfig_aliases_resolve_governed_imports() {
    let files = MemoryRepository::new([
        ("web/app.ts", ""),
        ("web/src/util.ts", ""),
        ("other/app.ts", ""),
        ("web/tsconfig.json", APP_ALIASES),
    ]);
    let source = "import { helper } from \"@/util\";";

    let governed = resolved("web/app.ts", source, &files);
    let outside = resolved("other/app.ts", source, &files);

    assert_eq!(includes(&governed), ["web/src/util.ts"]);
    assert_eq!(includes(&outside), Vec::<&str>::new());
}

#[test]
fn the_nearest_tsconfig_governs_alone() {
    let files = MemoryRepository::new([
        (
            "tsconfig.json",
            r#"{ "compilerOptions": { "paths": { "~/*": ["./shared/*"] } } }"#,
        ),
        ("web/tsconfig.json", APP_ALIASES),
        ("web/app.ts", ""),
        ("shared/theme.ts", ""),
    ]);

    let extraction = resolved("web/app.ts", "import { theme } from \"~/theme\";", &files);

    assert_eq!(includes(&extraction), Vec::<&str>::new());
    assert_eq!(imports(&extraction), ["~/theme"]);
}

#[test]
fn hash_imports_resolve_via_the_nearest_package_json() {
    let files = MemoryRepository::new([
        ("packages/common/app.ts", ""),
        ("packages/common/src/internal/db.ts", ""),
        ("elsewhere/app.ts", ""),
        (
            "packages/common/package.json",
            r##"{ "name": "@scope/common", "imports": { "#internal/*": "./src/internal/*" } }"##,
        ),
    ]);
    let source = "import { db } from \"#internal/db\";";

    let governed = resolved("packages/common/app.ts", source, &files);
    let outside = resolved("elsewhere/app.ts", source, &files);

    assert_eq!(includes(&governed), ["packages/common/src/internal/db.ts"]);
    assert_eq!(includes(&outside), Vec::<&str>::new());
}

#[test]
fn a_duplicate_package_name_resolves_within_the_importing_project() {
    let files = MemoryRepository::new([
        ("web/panel/src/app.ts", ""),
        ("web/panel/pkg/src/index.ts", ""),
        ("admin/panel/src/app.ts", ""),
        ("admin/panel/pkg/src/index.ts", ""),
        ("admin/panel/pkg/package.json", PANEL_UI),
        ("web/panel/pkg/package.json", PANEL_UI),
    ]);
    let source = "import { Widget } from \"@panel/ui\";";

    for (importer, expected) in [
        ("web/panel/src/app.ts", "web/panel/pkg/src/index.ts"),
        ("admin/panel/src/app.ts", "admin/panel/pkg/src/index.ts"),
    ] {
        assert_eq!(
            includes(&resolved(importer, source, &files)),
            [expected],
            "{importer}"
        );
    }
}

#[test]
fn equally_distant_duplicate_packages_abstain() {
    let files = MemoryRepository::new([
        ("app.ts", ""),
        ("web/pkg/src/index.ts", ""),
        ("admin/pkg/src/index.ts", ""),
        ("admin/pkg/package.json", PANEL_UI),
        ("web/pkg/package.json", PANEL_UI),
    ]);

    let extraction = resolved("app.ts", "import { Widget } from \"@panel/ui\";", &files);

    assert_eq!(includes(&extraction), Vec::<&str>::new());
    assert_eq!(imports(&extraction), ["@panel/ui"]);
}

#[test]
fn workspace_package_specifiers_resolve_via_exports() {
    let files = MemoryRepository::new([
        ("apps/admin/app.ts", ""),
        ("packages/common/src/index.ts", ""),
        ("packages/common/src/utils.ts", ""),
        ("packages/common/helpers.ts", ""),
        ("packages/common/package.json", COMMON),
    ]);
    let source = r#"
import { all } from "@scope/common";
import { helper } from "@scope/common/utils";
import { extra } from "@scope/common/helpers";
import lodash from "lodash";
"#;

    let extraction = resolved("apps/admin/app.ts", source, &files);

    assert_eq!(
        includes(&extraction),
        [
            "packages/common/src/index.ts",
            "packages/common/src/utils.ts",
            "packages/common/helpers.ts"
        ]
    );
    assert_eq!(imports(&extraction), ["lodash"]);
}

#[test]
fn a_package_without_a_dot_export_falls_back_to_its_main() {
    let files = MemoryRepository::new([
        ("app.ts", ""),
        ("lib/entry.ts", ""),
        (
            "lib/package.json",
            r#"{ "name": "lib", "main": "./entry.js", "exports": { "./x": "./x.ts" } }"#,
        ),
    ]);

    let extraction = resolved("app.ts", "import lib from \"lib\";", &files);

    assert_eq!(includes(&extraction), ["lib/entry.ts"]);
}

#[test]
fn a_vendored_package_is_not_read() {
    let files = MemoryRepository::new([
        ("app.ts", ""),
        ("node_modules/left-pad/index.ts", ""),
        (
            "node_modules/left-pad/package.json",
            r#"{ "name": "left-pad", "main": "./index.ts" }"#,
        ),
    ]);

    let extraction = resolved("app.ts", "import pad from \"left-pad\";", &files);

    assert_eq!(includes(&extraction), Vec::<&str>::new());
    assert_eq!(imports(&extraction), ["left-pad"]);
}

#[test]
fn bare_package_imports_stay_unresolved() {
    let files = MemoryRepository::of_paths(["src/app.ts", "src/lodash.ts"]);

    let extraction = resolved("src/app.ts", "import _ from \"lodash\";", &files);

    assert_eq!(includes(&extraction), Vec::<&str>::new());
}

#[test]
fn a_re_export_names_the_file_its_specifier_resolves_to() {
    let files =
        MemoryRepository::of_paths(["src/app.ts", "src/more/index.ts", "src/everything.tsx"]);

    let extraction = resolved("src/app.ts", SAMPLE_TS, &files);

    assert_eq!(
        re_export_files(&extraction),
        [
            ("help", Some("src/more/index.ts")),
            ("other", Some("src/more/index.ts")),
            ("*", Some("src/everything.tsx"))
        ]
    );
}

#[test]
fn a_re_export_of_a_module_outside_the_repository_names_no_file() {
    let files = MemoryRepository::of_paths(["src/app.ts", "src/util.ts"]);

    let extraction = resolved("src/app.ts", SAMPLE_TS, &files);

    assert_eq!(
        re_export_files(&extraction),
        [("help", None), ("other", None), ("*", None)]
    );
}

#[test]
fn a_re_export_resolves_by_every_module_rule() {
    let files = MemoryRepository::new([
        ("web/index.ts", ""),
        ("web/src/button.tsx", ""),
        ("web/src/icon.svg", ""),
        ("web/src/menu.ts", ""),
        ("web/tsconfig.json", APP_ALIASES),
        ("packages/common/src/index.ts", ""),
        ("packages/common/package.json", COMMON),
    ]);
    let source = r#"
import { Menu } from "./src/menu";
export { Button } from "@/button";
export { default as Icon } from "./src/icon.svg?react";
export { Menu } from "./src/menu.js";
export * from "@scope/common";
export * from "react";
"#;

    let extraction = resolved("web/index.ts", source, &files);

    assert_eq!(
        re_export_files(&extraction),
        [
            ("Button", Some("web/src/button.tsx")),
            ("Icon", Some("web/src/icon.svg")),
            ("Menu", Some("web/src/menu.ts")),
            ("*", Some("packages/common/src/index.ts")),
            ("*", None),
        ]
    );
    assert_eq!(
        includes(&extraction),
        [
            "web/src/menu.ts",
            "web/src/button.tsx",
            "web/src/icon.svg",
            "web/src/menu.ts",
            "packages/common/src/index.ts"
        ]
    );
}
