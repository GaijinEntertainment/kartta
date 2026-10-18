use std::cmp::Reverse;

use kartta_lang::Repository;
use kartta_lang::path::join;
use serde_json::{Map, Value};

use crate::config::{self, config_dir};
use crate::tsconfig::Aliases;

const CONDITIONS: [&str; 7] = [
    "source", "import", "module", "default", "require", "node", "types",
];

pub(crate) struct PkgImports {
    pub(crate) dir: String,
    pub(crate) patterns: Aliases,
}

pub(crate) struct PkgExports {
    pub(crate) name: String,
    pub(crate) dir: String,
    pub(crate) exports: Aliases,
    pub(crate) main: Vec<String>,
}

pub(crate) fn compile(repository: &dyn Repository) -> (Vec<PkgImports>, Vec<PkgExports>) {
    let mut imports = Vec::new();
    let mut packages = Vec::new();

    for (path, contents) in config::named(repository, "package.json") {
        let Ok(value) = serde_json::from_str::<Value>(&contents) else {
            continue;
        };
        let dir = config_dir(&path);

        // constraint: a `#` specifier resolves against the nearest package alone, so a package with no `imports` keeps
        // an empty entry that stops the lookup at it
        let patterns = value
            .get("imports")
            .and_then(Value::as_object)
            .map(|map| compile_map(map, dir))
            .unwrap_or_default();
        imports.push(PkgImports {
            dir: dir.to_owned(),
            patterns,
        });
        packages.extend(package(&value, dir));
    }

    imports.sort_by_key(|imports| Reverse(imports.dir.len()));
    packages.sort_by_key(|package| Reverse(package.name.len()));
    (imports, packages)
}

fn package(value: &Value, dir: &str) -> Option<PkgExports> {
    let name = value.get("name")?.as_str()?;
    let exports = match value.get("exports") {
        Some(Value::String(target)) => {
            vec![(".".to_owned(), join(dir, target).into_iter().collect())]
        }
        Some(Value::Object(map)) if map.keys().any(|key| key.starts_with('.')) => {
            compile_map(map, dir)
        }
        // constraint: an `exports` object whose keys are conditions rather than subpaths is the target of `.`
        Some(conditions @ Value::Object(_)) => {
            let mut targets = Vec::new();
            collect_targets(conditions, &mut targets);
            let targets: Vec<String> = targets
                .iter()
                .filter_map(|target| join(dir, target))
                .collect();
            vec![(".".to_owned(), targets)]
        }
        _ => Vec::new(),
    };
    let entries: Vec<&str> = (["module", "main"].iter())
        .filter_map(|key| value.get(*key)?.as_str())
        .collect();

    Some(PkgExports {
        name: name.to_owned(),
        dir: dir.to_owned(),
        exports,
        main: entries
            .iter()
            .filter_map(|entry| join(dir, entry))
            .collect(),
    })
}

fn compile_map(map: &Map<String, Value>, dir: &str) -> Aliases {
    let mut compiled = Vec::new();
    for (pattern, value) in map {
        let mut targets = Vec::new();
        collect_targets(value, &mut targets);
        if !targets.is_empty() {
            compiled.push((
                pattern.clone(),
                targets
                    .iter()
                    .filter_map(|target| join(dir, target))
                    .collect(),
            ));
        }
    }
    config::by_specificity(&mut compiled);
    compiled
}

fn collect_targets<'v>(value: &'v Value, targets: &mut Vec<&'v str>) {
    match value {
        Value::String(target) => targets.push(target),
        Value::Object(conditions) => {
            for condition in CONDITIONS
                .iter()
                .filter_map(|condition| conditions.get(*condition))
            {
                collect_targets(condition, targets);
            }
        }
        Value::Array(items) => {
            for item in items {
                collect_targets(item, targets);
            }
        }
        _ => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use kartta_lang::MemoryRepository;

    #[test]
    fn imports_exports_and_main_compile_onto_the_package_directory() {
        let files = MemoryRepository::new([
            (
                "packages/common/package.json",
                r##"{
                    "name": "@scope/common",
                    "main": "./index.js",
                    "imports": { "#internal/*": "./src/internal/*" },
                    "exports": {
                        ".": { "import": "./src/index.ts", "default": "./dist/index.js" },
                        "./utils": "./src/utils.ts",
                        "./lib/*": ["./src/lib/*", "./fallback/lib/*"]
                    }
                }"##,
            ),
            ("packages/common/tsconfig.json", "not a package.json"),
        ]);

        let (imports, packages) = compile(&files);

        let imports: Vec<_> = (imports.iter())
            .map(|imports| (imports.dir.as_str(), &imports.patterns))
            .collect();
        assert_eq!(
            imports,
            [(
                "packages/common",
                &vec![(
                    "#internal/*".to_owned(),
                    vec!["packages/common/src/internal/*".to_owned()]
                )]
            )]
        );
        let packages: Vec<_> = (packages.iter())
            .map(|package| (package.name.as_str(), &package.exports[0], &package.main))
            .collect();
        assert_eq!(
            packages,
            [(
                "@scope/common",
                &(
                    ".".to_owned(),
                    vec![
                        "packages/common/src/index.ts".to_owned(),
                        "packages/common/dist/index.js".to_owned()
                    ]
                ),
                &vec!["packages/common/index.js".to_owned()]
            )]
        );
    }
}
