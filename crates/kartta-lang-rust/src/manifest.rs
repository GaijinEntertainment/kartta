// constraint: a manifest's library is named by its `[lib] name`, else by its `[package] name` with `-` read as `_`, the
// name a path into the crate starts with, and lies at its `[lib] path`, else at `src/lib.rs`; a manifest without a
// `[package]` name, a workspace's own, declares none; the keys are read from their `key = "value"` lines, the one form
// Cargo writes them in, or `key = 'value'`, so a value spread over lines or kept in an inline table is not read
pub(crate) fn library(manifest: &str) -> Option<(String, Option<String>)> {
    let mut table = "";
    let (mut package, mut name, mut path) = (None, None, None);
    for line in manifest.lines().map(str::trim) {
        if let Some(header) = line.strip_prefix('[') {
            table = header
                .trim_start_matches('[')
                .split(']')
                .next()
                .unwrap_or_default()
                .trim();
            continue;
        }
        let Some((key, value)) = line.split_once('=') else {
            continue;
        };
        let read = match (table, key.trim()) {
            ("package", "name") => &mut package,
            ("lib", "name") => &mut name,
            ("lib", "path") => &mut path,
            _ => continue,
        };
        *read = quoted(value.trim());
    }
    let package = package?;
    Some((name.unwrap_or(package).replace('-', "_"), path))
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
}
