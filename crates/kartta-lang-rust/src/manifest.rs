use std::collections::HashSet;

// constraint: a package is named by its `[package] name`; a manifest without one, a workspace's own, declares no
// package and no library; its dependencies are the ones every build of its code is built with, and its other
// dependencies the ones only some of its builds are, an optional one and those of its tests, examples, benchmarks and
// build script, which a package that depends on it is not built with unless it enables a feature
pub(crate) struct Manifest {
    pub(crate) package: String,
    pub(crate) library: (String, Option<String>),
    pub(crate) dependencies: Vec<String>,
    pub(crate) other_dependencies: Vec<String>,
    // constraint: the dependencies whose entry the workspace's `[workspace.dependencies]` holds, by `workspace = true`
    pub(crate) inherited: Vec<String>,
}

const DEPENDENCY_TABLES: [(&str, bool); 3] = [
    ("dependencies", true),
    ("dev-dependencies", false),
    ("build-dependencies", false),
];

// constraint: one entry of a dependency table: `normal` where the package's code is built with it, rather than its
// tests, examples, benchmarks or build script alone
struct Dependency {
    name: String,
    normal: bool,
    optional: bool,
    inherited: bool,
}

// constraint: a manifest's library is named by its `[lib] name`, else by its `[package] name` with `-` read as `_`, the
// name a path into the crate starts with, and lies at its `[lib] path`, else at `src/lib.rs`; its dependencies are the
// packages its dependency tables name, of every target, by the `package` key that renames one, else by its key, `-`
// read as `_` as well, and those of `[workspace.dependencies]` only where a member names them, as `inherited` marks;
// the keys are read from
// their `key = "value"` lines, the one form Cargo writes them in, or `key = 'value'`, and an inline table on its key's
// line, so a value spread over lines is not read
pub(crate) fn read(manifest: &str) -> Option<Manifest> {
    let mut table = Table::Other;
    let (mut package, mut name, mut path) = (None, None, None);
    let mut dependencies: Vec<Dependency> = Vec::new();
    for line in manifest.lines().map(str::trim) {
        if let Some(header) = header(line) {
            table = Table::of(header);
            if let Table::Dependency(name, normal) = table {
                dependencies.push(Dependency::named(name, normal));
            }
            continue;
        }
        let Some((key, value)) = line.split_once('=') else {
            continue;
        };
        let (key, value) = (key.trim(), value.trim());
        let read = match (table, key) {
            (Table::Package, "name") => &mut package,
            (Table::Lib, "name") => &mut name,
            (Table::Lib, "path") => &mut path,
            (Table::Dependencies(normal), _) => {
                dependencies.push(dependency(key, value, normal));
                continue;
            }
            (Table::Dependency(..), _) => {
                if let Some(last) = dependencies.last_mut() {
                    last.read(key, value);
                }
                continue;
            }
            _ => continue,
        };
        *read = quoted(value);
    }
    let package = package?;
    let inherited = (dependencies.iter())
        .filter(|dependency| dependency.inherited)
        .map(|dependency| dependency.name.replace('-', "_"))
        .collect();
    let (dependencies, other_dependencies) = split(&dependencies);
    Some(Manifest {
        library: (
            name.unwrap_or_else(|| package.clone()).replace('-', "_"),
            path,
        ),
        package: package.replace('-', "_"),
        dependencies,
        other_dependencies,
        inherited,
    })
}

// constraint: a workspace's `[workspace.dependencies]` renames an entry by its `package` key, and a member that
// inherits the entry depends on the package it names; each rename maps the entry's key to that package, `-` read as
// `_`, and a manifest that declares no workspace has none
pub(crate) fn workspace_renames(manifest: &str) -> Option<Vec<(String, String)>> {
    let (mut workspace, mut table) = (false, Table::Other);
    let mut entries: Vec<(&str, Dependency)> = Vec::new();
    for line in manifest.lines().map(str::trim) {
        if let Some(header) = header(line) {
            workspace |= header == "workspace" || header.starts_with("workspace.");
            table = Table::of(header);
            if let Table::WorkspaceDependency(key) = table {
                entries.push((key, Dependency::named(key, true)));
            }
            continue;
        }
        let Some((key, value)) = line.split_once('=') else {
            continue;
        };
        let (key, value) = (key.trim(), value.trim());
        match table {
            Table::WorkspaceDependencies => {
                let entry = dependency(key, value, true);
                let key = unquoted(key.split_once('.').map_or(key, |(key, _)| key).trim());
                entries.push((key, entry));
            }
            Table::WorkspaceDependency(_) => {
                if let Some((_, last)) = entries.last_mut() {
                    last.read(key, value);
                }
            }
            _ => {}
        }
    }
    workspace.then(|| {
        (entries.into_iter())
            .filter(|(key, entry)| *key != entry.name)
            .map(|(key, entry)| (key.replace('-', "_"), entry.name.replace('-', "_")))
            .collect()
    })
}

fn header(line: &str) -> Option<&str> {
    let header = line.strip_prefix('[')?;
    Some(
        header
            .trim_start_matches('[')
            .split(']')
            .next()
            .unwrap_or_default()
            .trim(),
    )
}

impl Dependency {
    fn named(name: &str, normal: bool) -> Self {
        Self {
            name: name.to_owned(),
            normal,
            optional: false,
            inherited: false,
        }
    }

    // constraint: a dependency's own table renames it by its `package` key, makes it optional by `optional = true` and
    // takes its entry from the workspace by `workspace = true`
    fn read(&mut self, key: &str, value: &str) {
        match key {
            "package" => {
                if let Some(renamed) = quoted(value) {
                    self.name = renamed;
                }
            }
            "optional" if value == "true" => self.optional = true,
            "workspace" if value == "true" => self.inherited = true,
            _ => {}
        }
    }

    const fn is_required(&self) -> bool {
        self.normal && !self.optional
    }
}

// constraint: a name is a dependency every build takes only where none of its entries for the code is optional, as a
// dotted key spreads one dependency over several lines
fn split(dependencies: &[Dependency]) -> (Vec<String>, Vec<String>) {
    let optional: HashSet<&str> = (dependencies.iter())
        .filter(|dependency| dependency.normal && dependency.optional)
        .map(|dependency| dependency.name.as_str())
        .collect();
    let (required, other): (Vec<&Dependency>, Vec<&Dependency>) =
        (dependencies.iter()).partition(|dependency| {
            dependency.is_required() && !optional.contains(dependency.name.as_str())
        });
    let names = |of: Vec<&Dependency>| -> Vec<String> {
        (of.iter())
            .map(|dependency| dependency.name.replace('-', "_"))
            .collect()
    };
    (names(required), names(other))
}

#[derive(Clone, Copy)]
enum Table<'m> {
    Package,
    Lib,
    Dependencies(bool),
    Dependency(&'m str, bool),
    WorkspaceDependencies,
    WorkspaceDependency(&'m str),
    Other,
}

impl<'m> Table<'m> {
    // constraint: a dependency table is `[dependencies]` or a target's `[target.<cfg>.dependencies]`, of each kind, and
    // one dependency's own table adds `.<name>` to it; the workspace's tables declare what members may name
    fn of(header: &'m str) -> Self {
        match header {
            "package" => return Self::Package,
            "lib" => return Self::Lib,
            "workspace.dependencies" => return Self::WorkspaceDependencies,
            _ if header.starts_with("workspace") => {
                return (header.strip_prefix("workspace.dependencies."))
                    .filter(|key| !key.is_empty())
                    .map_or(Self::Other, |key| Self::WorkspaceDependency(unquoted(key)));
            }
            _ => {}
        }
        let segment = DEPENDENCY_TABLES.iter().find_map(|&(kind, normal)| {
            header.match_indices(kind).find_map(|(at, _)| {
                let rest = &header[at + kind.len()..];
                (header[..at].is_empty() || header[..at].ends_with('.'))
                    .then_some((rest, normal))
                    .filter(|(rest, _)| rest.is_empty() || rest.starts_with('.'))
            })
        });
        match segment.map(|(rest, normal)| (rest.strip_prefix('.'), normal)) {
            Some((None, normal)) => Self::Dependencies(normal),
            Some((Some(name), normal)) if !name.is_empty() => {
                Self::Dependency(unquoted(name), normal)
            }
            _ => Self::Other,
        }
    }
}

// constraint: a dependency's key may be dotted, `name.workspace = true`, and its inline table, or a dotted
// `name.package` or `name.optional`, reads as its own table does
fn dependency(key: &str, value: &str, normal: bool) -> Dependency {
    let (name, field) = key.split_once('.').unwrap_or((key, ""));
    let mut dependency = Dependency::named(unquoted(name.trim()), normal);
    if field.is_empty() {
        let entries = value
            .strip_prefix('{')
            .unwrap_or_default()
            .split([',', '}']);
        for (key, value) in entries.filter_map(|entry| entry.split_once('=')) {
            dependency.read(key.trim(), value.trim());
        }
    } else {
        dependency.read(field.trim(), value);
    }
    dependency
}

fn unquoted(key: &str) -> &str {
    key.trim_matches(|quote| matches!(quote, '"' | '\''))
}

fn quoted(value: &str) -> Option<String> {
    let quote = value
        .chars()
        .next()
        .filter(|quote| matches!(quote, '"' | '\''))?;
    let rest = &value[1..];
    rest.find(quote).map(|end| rest[..end].to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn library(manifest: &str) -> Option<(String, Option<String>)> {
        read(manifest).map(|manifest| manifest.library)
    }

    #[test]
    fn a_library_is_named_by_its_lib_table_else_by_its_package() {
        let package =
            library("[package]\nname = \"kartta-lang\" # the engine's\nversion = \"0.0.0\"\n");
        let renamed = library(
            "[package]\nname = \"walker\"\n\n[lib]\nname = 'kartta_walk'\npath = \"lib.rs\"\n\n\
             [[bin]]\nname = \"walk\"\n",
        );
        let workspace =
            library("[workspace]\nmembers = [\"crates/*\"]\n\n[workspace.package]\nname = \"x\"\n");
        let no_package = library("[workspace]\nmembers = []\n\n[lib]\nname = \"orphan\"\n");

        assert_eq!(package, Some(("kartta_lang".to_owned(), None)));
        assert_eq!(
            renamed,
            Some(("kartta_walk".to_owned(), Some("lib.rs".to_owned())))
        );
        assert_eq!(workspace, None);
        assert_eq!(no_package, None);
    }

    #[test]
    fn a_package_depends_on_what_each_of_its_dependency_tables_names() {
        let manifest = read(
            "[package]\nname = \"kartta-fs\"\n\n[dependencies]\nkartta-walk = { path = \"../kartta-walk\" }\n\
             gix-ignore.workspace = true\nwalk = { version = \"1\", package = \"walkdir\" }\n\
             \"quoted-key\" = \"1\"\nnotify.package = 'notify-rs'\nserde = { version = \"1\", optional = true }\n\
             rayon.version = \"1\"\nrayon.optional = true\n\n\
             [target.'cfg(unix)'.dependencies]\nlibc = \"0.2\"\n\n[dev-dependencies]\ntempfile = \"3\"\n\
             kartta-walk = { path = \"../kartta-walk\" }\n\n\
             [build-dependencies.cc]\nversion = \"1\"\n\n[target.\"cfg(windows)\".dev-dependencies.win]\n\
             package = \"windows-sys\"\n\n[dependencies.mio]\nversion = \"1\"\noptional = true\n\n\
             [workspace.dependencies]\nunused = \"1\"\n\n[features]\ndefault = [\"std\"]\n",
        )
        .expect("the manifest names a package");

        assert_eq!(manifest.package, "kartta_fs");
        assert_eq!(
            manifest.dependencies,
            [
                "kartta_walk",
                "gix_ignore",
                "walkdir",
                "quoted_key",
                "notify_rs",
                "libc",
            ]
        );
        assert_eq!(
            manifest.other_dependencies,
            [
                "serde",
                "rayon",
                "rayon",
                "tempfile",
                "kartta_walk",
                "cc",
                "windows_sys",
                "mio"
            ]
        );
    }
}
