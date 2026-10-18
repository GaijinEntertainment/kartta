use std::cmp::Reverse;

use kartta_lang::Repository;
use kartta_lang::path::join;
use serde_json::Value;

use crate::config::{self, config_dir, is_json};

pub(crate) type Aliases = Vec<(String, Vec<String>)>;

pub(crate) struct TsAliasConfig {
    pub(crate) dir: String,
    pub(crate) aliases: Aliases,
}

pub(crate) fn alias_configs(repository: &dyn Repository) -> Vec<TsAliasConfig> {
    let mut compiled: Vec<TsAliasConfig> = config::named(repository, "tsconfig.json")
        .filter_map(|(path, contents)| compile(&path, &contents, repository))
        .collect();

    compiled.sort_by_key(|compiled| Reverse(compiled.dir.len()));
    compiled
}

fn compile(path: &str, contents: &str, repository: &dyn Repository) -> Option<TsAliasConfig> {
    let dir = config_dir(path);
    let value = parse(contents)?;

    let mut aliases = aliases_in(&value, dir, repository)
        .unwrap_or_else(|| aliases_via_references(&value, dir, repository));
    config::by_specificity(&mut aliases);

    // constraint: the nearest config governs alone, so one that yields no aliases keeps an empty entry that stops the
    // lookup at it
    Some(TsAliasConfig {
        dir: dir.to_owned(),
        aliases,
    })
}

fn parse(contents: &str) -> Option<Value> {
    serde_json::from_str(&strip_jsonc(contents)).ok()
}

fn aliases_via_references(value: &Value, dir: &str, repository: &dyn Repository) -> Aliases {
    let Some(references) = value.get("references").and_then(Value::as_array) else {
        return Vec::new();
    };

    let mut merged = Vec::new();
    for reference in references
        .iter()
        .filter_map(|reference| reference.get("path")?.as_str())
    {
        if !is_json(reference) {
            continue;
        }
        let Some(target) = join(dir, reference).filter(|target| config_dir(target) == dir) else {
            continue;
        };
        let Some(sibling) = config::read(repository, &target).as_deref().and_then(parse) else {
            continue;
        };
        merged.extend(aliases_in(&sibling, dir, repository).into_iter().flatten());
    }
    merged
}

fn aliases_in(value: &Value, dir: &str, repository: &dyn Repository) -> Option<Aliases> {
    let base = value
        .get("extends")
        .and_then(Value::as_str)
        .and_then(|extends| {
            let target = if is_json(extends) {
                join(dir, extends)
            } else {
                join(dir, &format!("{extends}.json"))
            }?;
            let parsed = parse(&config::read(repository, &target)?)?;
            Some((parsed, config_dir(&target).to_owned()))
        });
    let base = base
        .as_ref()
        .map(|(parsed, declared_in)| (parsed, declared_in.as_str()));

    let (paths, paths_dir) = compiler_option((value, dir), base, "paths")?;
    let paths = paths.as_object()?;
    let base_dir = compiler_option((value, dir), base, "baseUrl")
        .and_then(|(base_url, declared_in)| Some((base_url.as_str()?, declared_in)))
        .map_or_else(
            || Some(paths_dir.to_owned()),
            |(base_url, declared_in)| join(declared_in, base_url),
        );

    let mut aliases = Vec::new();
    for (pattern, targets) in paths {
        let Some(targets) = targets.as_array() else {
            continue;
        };
        let targets: Vec<&str> = targets.iter().filter_map(Value::as_str).collect();
        if targets.is_empty() {
            continue;
        }
        let targets = (targets.iter())
            .filter_map(|target| join(base_dir.as_deref()?, target))
            .collect();
        aliases.push((pattern.clone(), targets));
    }

    Some(aliases)
}

// constraint: a path a config declares is relative to the directory of the config that declares it, so an option a
// base config supplies comes back with the base config's directory
fn compiler_option<'v>(
    (value, dir): (&'v Value, &'v str),
    base: Option<(&'v Value, &'v str)>,
    key: &str,
) -> Option<(&'v Value, &'v str)> {
    (value
        .get("compilerOptions")
        .and_then(|options| options.get(key)))
    .map(|found| (found, dir))
    .or_else(|| {
        let (base, base_dir) = base?;
        Some((base.get("compilerOptions")?.get(key)?, base_dir))
    })
}

fn strip_jsonc(source: &str) -> String {
    let bytes = source.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());

    let mut at = 0;
    while let Some(&byte) = bytes.get(at) {
        at = match byte {
            b'"' => copy_string(bytes, at, &mut out),
            b'/' if bytes.get(at + 1) == Some(&b'/') => (bytes[at..].iter())
                .position(|&byte| byte == b'\n')
                .map_or(bytes.len(), |offset| at + offset),
            b'/' if bytes.get(at + 1) == Some(&b'*') => (bytes[at + 2..].windows(2))
                .position(|pair| pair == b"*/")
                .map_or(bytes.len(), |offset| at + offset + 4),
            b',' if closes_after(bytes, at + 1) => at + 1,
            byte => {
                out.push(byte);
                at + 1
            }
        };
    }

    String::from_utf8(out).unwrap_or_default()
}

fn copy_string(bytes: &[u8], start: usize, out: &mut Vec<u8>) -> usize {
    out.push(b'"');

    let mut at = start + 1;
    while let Some(&byte) = bytes.get(at) {
        out.push(byte);
        at += 1;
        match byte {
            b'"' => return at,
            b'\\' => {
                if let Some(&escaped) = bytes.get(at) {
                    out.push(escaped);
                    at += 1;
                }
            }
            _ => {}
        }
    }
    at
}

fn closes_after(bytes: &[u8], mut at: usize) -> bool {
    loop {
        at = match bytes.get(at..) {
            Some([byte, ..]) if byte.is_ascii_whitespace() => at + 1,
            Some([b'/', b'/', ..]) => (bytes[at..].iter())
                .position(|&byte| byte == b'\n')
                .map_or(bytes.len(), |offset| at + offset),
            Some([b'/', b'*', rest @ ..]) => (rest.windows(2))
                .position(|pair| pair == b"*/")
                .map_or(bytes.len(), |offset| at + offset + 4),
            Some([b'}' | b']', ..]) => return true,
            _ => return false,
        };
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use kartta_lang::MemoryRepository;

    fn compiled(files: &[(&str, &str)]) -> Vec<(String, Aliases)> {
        (alias_configs(&MemoryRepository::new(files.iter().copied())).into_iter())
            .map(|compiled| (compiled.dir, compiled.aliases))
            .collect()
    }

    fn alias(pattern: &str, targets: &[&str]) -> (String, Vec<String>) {
        (
            pattern.to_owned(),
            targets.iter().map(|target| (*target).to_owned()).collect(),
        )
    }

    const APP: &str =
        r#"{ "compilerOptions": { "baseUrl": ".", "paths": { "@/*": ["./src/*"] } } }"#;

    #[test]
    fn comments_and_trailing_commas_are_stripped_outside_strings() {
        let commented = r#"{
  // line comment with "a quote"
  "compilerOptions": {
    /* block
       comment */
    "baseUrl": ".",
    "paths": { "@/*": ["./src/*"], },
  },
}"#;
        let with_comma_in_string = r#"{"a": "x,}", "b": 1}"#;

        let stripped = parse(commented).expect("valid after strip");
        let kept = parse(with_comma_in_string).expect("string commas intact");

        assert_eq!(stripped["compilerOptions"]["baseUrl"], ".");
        assert_eq!(kept["a"], "x,}");
    }

    #[test]
    fn paths_take_the_base_url_and_one_extends_hop() {
        let got = compiled(&[
            (
                "web/tsconfig.json",
                r#"{ "extends": "./tsconfig.base", "compilerOptions": {} }"#,
            ),
            ("web/tsconfig.base.json", APP),
        ]);

        assert_eq!(
            got,
            [("web".to_owned(), vec![alias("@/*", &["web/src/*"])])]
        );
    }

    #[test]
    fn a_solution_style_config_takes_aliases_from_its_sibling_references() {
        let got = compiled(&[
            (
                "web/tsconfig.json",
                r#"{
                    "files": [],
                    "references": [
                        { "path": "./tsconfig.app.json" },
                        { "path": "./tsconfig.node.json" }
                    ]
                }"#,
            ),
            ("web/tsconfig.app.json", APP),
            (
                "web/tsconfig.node.json",
                r#"{ "compilerOptions": { "baseUrl": ".", "paths": { "~/*": ["./tools/*"] } } }"#,
            ),
        ]);

        assert_eq!(
            got,
            [(
                "web".to_owned(),
                vec![alias("@/*", &["web/src/*"]), alias("~/*", &["web/tools/*"])]
            )]
        );
    }

    #[test]
    fn a_root_solution_style_config_follows_its_siblings() {
        let got = compiled(&[
            (
                "tsconfig.json",
                r#"{ "files": [], "references": [{ "path": "./tsconfig.app.json" }] }"#,
            ),
            ("tsconfig.app.json", APP),
        ]);

        assert_eq!(got, [(String::new(), vec![alias("@/*", &["src/*"])])]);
    }

    #[test]
    fn a_sibling_that_fails_to_parse_contributes_nothing() {
        let got = compiled(&[
            (
                "web/tsconfig.json",
                r#"{ "files": [], "references": [
                    { "path": "./tsconfig.broken.json" },
                    { "path": "./tsconfig.app.json" }
                ] }"#,
            ),
            ("web/tsconfig.broken.json", "{ this is not json"),
            ("web/tsconfig.app.json", APP),
        ]);

        assert_eq!(
            got,
            [("web".to_owned(), vec![alias("@/*", &["web/src/*"])])]
        );
    }

    #[test]
    fn an_explicitly_empty_paths_declaration_suppresses_the_fallback() {
        let got = compiled(&[
            (
                "web/tsconfig.json",
                r#"{
                    "compilerOptions": { "paths": {} },
                    "references": [{ "path": "./tsconfig.app.json" }]
                }"#,
            ),
            ("web/tsconfig.app.json", APP),
        ]);

        assert_eq!(got, [("web".to_owned(), Vec::new())]);
    }

    #[test]
    fn a_subproject_reference_does_not_hoist_its_aliases() {
        let got = compiled(&[
            (
                "tsconfig.json",
                r#"{ "files": [], "references": [{ "path": "./packages/ui" }] }"#,
            ),
            ("packages/ui/tsconfig.json", APP),
        ]);

        assert_eq!(
            got,
            [
                (
                    "packages/ui".to_owned(),
                    vec![alias("@/*", &["packages/ui/src/*"])]
                ),
                (String::new(), Vec::new()),
            ]
        );
    }

    #[test]
    fn a_trailing_comma_before_a_comment_still_parses() {
        let got = compiled(&[(
            "tsconfig.json",
            "{ \"compilerOptions\": { \"paths\": { \"@/*\": [\"./src/*\"], // the app\n } } }",
        )]);

        assert_eq!(got, [(String::new(), vec![alias("@/*", &["src/*"])])]);
    }

    #[test]
    fn a_vendored_config_governs_nothing() {
        let got = compiled(&[
            ("node_modules/lib/tsconfig.json", APP),
            ("web/vendor/tsconfig.json", APP),
        ]);

        assert_eq!(got, []);
    }

    #[test]
    fn an_extends_target_outside_the_config_names_reads_as_no_base() {
        let got = compiled(&[
            ("web/tsconfig.json", r#"{ "extends": "./base.json" }"#),
            ("web/base.json", APP),
        ]);

        assert_eq!(got, [("web".to_owned(), Vec::new())]);
    }
}
