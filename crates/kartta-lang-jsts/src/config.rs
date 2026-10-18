use std::cmp::Reverse;

use kartta_lang::Repository;
use kartta_lang::path::{base, dir};

pub(crate) type Aliases = Vec<(String, Vec<String>)>;

pub(crate) struct ScopedAliases {
    pub(crate) dir: String,
    pub(crate) aliases: Aliases,
}

// constraint: Node's `exports` and `imports` and TypeScript's `paths` all take an exact key before a pattern, and of
// two patterns the one with the longer prefix before its `*`, so patterns are tried in that order
pub(crate) fn by_specificity(patterns: &mut [(String, Vec<String>)]) {
    patterns.sort_by_key(|(pattern, _)| {
        pattern
            .find('*')
            .map_or((false, Reverse(0), Reverse(0)), |star| {
                (true, Reverse(star), Reverse(pattern.len()))
            })
    });
}

pub(crate) fn named<'r>(
    repository: &'r dyn Repository,
    name: &str,
) -> impl Iterator<Item = (String, String)> + 'r {
    (repository.files_named(name)).filter_map(|path| {
        let contents = read(repository, &path)?;
        Some((path, contents))
    })
}

pub(crate) fn read(repository: &dyn Repository, path: &str) -> Option<String> {
    if !is_project_config(path) {
        return None;
    }
    String::from_utf8(repository.read(path).ok()?).ok()
}

pub(crate) fn is_json(path: &str) -> bool {
    path.rsplit_once('.')
        .is_some_and(|(_, extension)| extension == "json")
}

pub(crate) fn config_dir(path: &str) -> &str {
    match dir(path) {
        "." => "",
        dir => dir,
    }
}

pub(crate) fn is_vendored(path: &str) -> bool {
    path.starts_with("node_modules/")
        || path.contains("/node_modules/")
        || path.starts_with("vendor/")
        || path.contains("/vendor/")
}

fn is_project_config(path: &str) -> bool {
    let name = base(path);

    !is_vendored(path)
        && (name == "package.json" || (name.starts_with("tsconfig") && is_json(name)))
}
