use kartta_lang::Repository;
use kartta_lang::path::{dir, join};

use crate::config::ScopedAliases;
use crate::package_json::{self, PkgExports};
use crate::tsconfig;

const PROBE_SUFFIXES: [&str; 19] = [
    "",
    ".tsx",
    ".ts",
    ".mts",
    ".cts",
    ".d.ts",
    ".jsx",
    ".js",
    ".mjs",
    ".cjs",
    "/index.tsx",
    "/index.ts",
    "/index.mts",
    "/index.cts",
    "/index.d.ts",
    "/index.jsx",
    "/index.js",
    "/index.mjs",
    "/index.cjs",
];

const ESM_EXTENSIONS: [&str; 4] = [".js", ".mjs", ".cjs", ".jsx"];

pub(crate) struct Modules<'r> {
    repository: &'r dyn Repository,
    ts_aliases: Vec<ScopedAliases>,
    pkg_imports: Vec<ScopedAliases>,
    packages: Vec<PkgExports>,
}

impl<'r> Modules<'r> {
    pub(crate) fn read(repository: &'r dyn Repository) -> Self {
        let (pkg_imports, packages) = package_json::compile(repository);

        Self {
            repository,
            ts_aliases: tsconfig::alias_configs(repository),
            pkg_imports,
            packages,
        }
    }

    pub(crate) fn resolve(&self, specifier: &str, importer: &str) -> Option<String> {
        if let Some(target) = self.resolve_literal(specifier, importer) {
            return Some(target);
        }
        let (module, _) = specifier.split_once('?')?;
        if module.is_empty() {
            return None;
        }
        self.resolve_literal(module, importer)
    }

    fn resolve_literal(&self, specifier: &str, importer: &str) -> Option<String> {
        if specifier.starts_with("./") || specifier.starts_with("../") {
            return self.probe(&join(dir(importer), specifier)?);
        }
        if specifier.starts_with('#') {
            return self.resolve_alias(&self.pkg_imports, specifier, importer);
        }
        (self.resolve_alias(&self.ts_aliases, specifier, importer))
            .or_else(|| self.resolve_package(specifier, importer))
    }

    fn resolve_alias(
        &self,
        configs: &[ScopedAliases],
        specifier: &str,
        importer: &str,
    ) -> Option<String> {
        let governing = configs
            .iter()
            .find(|config| governs(&config.dir, importer))?;
        self.probe_patterns(&governing.aliases, specifier)
    }

    fn resolve_package(&self, specifier: &str, importer: &str) -> Option<String> {
        let package = self.nearest_package(specifier, importer)?;
        let subpath = if specifier == package.name {
            ".".to_owned()
        } else {
            format!(".{}", &specifier[package.name.len()..])
        };

        if let Some(target) = self.probe_patterns(&package.exports, &subpath) {
            return Some(target);
        }
        if subpath == "." {
            return (package.main.iter())
                .find_map(|main| self.probe(main))
                .or_else(|| self.probe(&package.dir));
        }
        self.probe(&join(&package.dir, &subpath)?)
    }

    fn nearest_package(&self, specifier: &str, importer: &str) -> Option<&PkgExports> {
        let named = || {
            self.packages.iter().filter(|package| {
                specifier
                    .strip_prefix(package.name.as_str())
                    .is_some_and(|rest| rest.is_empty() || rest.starts_with('/'))
            })
        };
        let name = &named().next()?.name;
        let same_name = || named().filter(|package| package.name == *name);

        let closest = same_name()
            .map(|package| shared_segments(&package.dir, importer))
            .max()?;
        let mut nearest =
            same_name().filter(|package| shared_segments(&package.dir, importer) == closest);
        let winner = nearest.next()?;
        nearest.next().is_none().then_some(winner)
    }

    fn probe_patterns(
        &self,
        patterns: &[(String, Vec<String>)],
        specifier: &str,
    ) -> Option<String> {
        patterns.iter().find_map(|(pattern, targets)| {
            let captured = match_pattern(pattern, specifier)?;
            (targets.iter()).find_map(|target| self.probe(&target.replacen('*', captured, 1)))
        })
    }

    fn probe(&self, base_path: &str) -> Option<String> {
        self.probe_suffixes(base_path).or_else(|| {
            let stripped = ESM_EXTENSIONS
                .iter()
                .find_map(|extension| base_path.strip_suffix(extension))?;
            self.probe_suffixes(stripped)
        })
    }

    // constraint: the root directory is the empty path, so its index files are `index.*` with no leading separator, and
    // a file suffix on the empty path names nothing
    fn probe_suffixes(&self, base_path: &str) -> Option<String> {
        (PROBE_SUFFIXES.iter())
            .filter_map(|suffix| match base_path {
                "" => suffix.strip_prefix('/').map(str::to_owned),
                base_path => Some(format!("{base_path}{suffix}")),
            })
            .find(|candidate| self.repository.contains(candidate))
    }
}

fn governs(dir: &str, importer: &str) -> bool {
    dir.is_empty()
        || importer
            .strip_prefix(dir)
            .is_some_and(|rest| rest.starts_with('/'))
}

fn shared_segments(dir: &str, path: &str) -> usize {
    if dir.is_empty() {
        return 0;
    }
    (dir.split('/').zip(path.split('/')))
        .take_while(|(dir, path)| dir == path)
        .count()
}

fn match_pattern<'s>(pattern: &str, specifier: &'s str) -> Option<&'s str> {
    match pattern.split_once('*') {
        Some((prefix, suffix)) => specifier.strip_prefix(prefix)?.strip_suffix(suffix),
        None => (pattern == specifier).then_some(""),
    }
}
