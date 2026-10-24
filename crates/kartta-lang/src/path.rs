//! Root-relative, forward-slash paths, as a [`Repository`](crate::Repository) names files.

use std::borrow::Cow;

/// `path` with each `\` turned into `/`, as a loader that reads a backslash as a separator spells it; `path` itself,
/// borrowed, when it holds none.
#[must_use]
pub fn forward_slashes(path: &str) -> Cow<'_, str> {
    if path.contains('\\') {
        Cow::Owned(path.replace('\\', "/"))
    } else {
        Cow::Borrowed(path)
    }
}

/// The directory of `path`: `src/a` for `src/a/b.go`, and `.` for a file at the root.
#[must_use]
pub fn dir(path: &str) -> &str {
    path.rsplit_once('/').map_or(".", |(dir, _)| dir)
}

/// The last component of `path`: `b.go` for `src/a/b.go`.
#[must_use]
pub fn base(path: &str) -> &str {
    path.rsplit_once('/').map_or(path, |(_, base)| base)
}

/// Whether the last component of `path` ends in a dot and one of `extensions`, in any case.
#[must_use]
pub fn has_extension(path: &str, extensions: &[&str]) -> bool {
    base(path).rsplit_once('.').is_some_and(|(_, extension)| {
        extensions
            .iter()
            .any(|known| extension.eq_ignore_ascii_case(known))
    })
}

/// Whether a whole `/`-separated component of `path`, the last one included, is one of `names` in any case.
#[must_use]
pub fn has_component(path: &str, names: &[&str]) -> bool {
    path.split('/').any(|component| {
        names
            .iter()
            .any(|name| component.eq_ignore_ascii_case(name))
    })
}

/// `name` under the directory `dir`, where an empty `dir` is the root and an empty `name` is `dir` itself.
#[must_use]
pub fn child(dir: &str, name: &str) -> String {
    match (dir.is_empty(), name.is_empty()) {
        (true, _) => name.to_owned(),
        (false, true) => dir.to_owned(),
        (false, false) => format!("{dir}/{name}"),
    }
}

/// The one path `found` yields; `None` when it yields none, or several and a choice between them would be a guess.
#[must_use]
pub fn sole(mut found: impl Iterator<Item = String>) -> Option<String> {
    let only = found.next()?;
    found.next().is_none().then_some(only)
}

/// How many leading `/`-separated components `left` and `right` share: 1 for `a/b` and `a/c`, 0 for `a` and `b`.
#[must_use]
pub fn shared_segments(left: &str, right: &str) -> usize {
    left.split('/')
        .zip(right.split('/'))
        .take_while(|(left, right)| left == right)
        .count()
}

/// The root-relative path that `relative`, such as `./util/b` or `../b`, names from the directory `from`, with its
/// `.` and `..` components applied; `None` when it climbs above the root.
///
/// The root itself comes back as the empty path, which a [`Repository`](crate::Repository) takes for the root as it
/// takes `.`.
#[must_use]
pub fn join(from: &str, relative: &str) -> Option<String> {
    let mut components: Vec<&str> = from
        .split('/')
        .filter(|component| !matches!(*component, "" | "."))
        .collect();

    for component in relative.split('/') {
        match component {
            "" | "." => {}
            ".." => {
                components.pop()?;
            }
            name => components.push(name),
        }
    }

    Some(components.join("/"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_file_at_the_root_is_in_the_dot_directory() {
        assert_eq!(dir("src/a/b.go"), "src/a");
        assert_eq!(dir("b.go"), ".");
        assert_eq!(base("src/a/b.go"), "b.go");
        assert_eq!(base("b.go"), "b.go");
    }

    #[test]
    fn a_relative_path_applies_its_dot_components() {
        assert_eq!(join("src", "./b").as_deref(), Some("src/b"));
        assert_eq!(join("src/a", "../b").as_deref(), Some("src/b"));
        assert_eq!(join("src/a", "../../b").as_deref(), Some("b"));
        assert_eq!(join(".", "./util/b").as_deref(), Some("util/b"));
        assert_eq!(join("", "./b").as_deref(), Some("b"));
        assert_eq!(join("a", "..").as_deref(), Some(""));
    }

    #[test]
    fn an_extension_matches_in_any_case_after_the_last_dot_of_the_name() {
        assert!(has_extension("src/Main.PY", &["py", "pyi"]));
        assert!(has_extension("a.tar.gz", &["gz"]));
        assert!(!has_extension("a.tar.gz", &["tar"]));
        assert!(!has_extension("py/README", &["py"]));
    }

    #[test]
    fn a_component_matches_whole_and_in_any_case() {
        assert!(has_component("phys/Tests/a.cpp", &["test", "tests"]));
        assert!(has_component("UnitTests/a.h", &["unittests"]));
        assert!(
            has_component("a/tests", &["tests"]),
            "the last component counts"
        );
        assert!(!has_component("prog/latestData/a.nut", &["testdata"]));
        assert!(!has_component("prog/latest/a.nut", &["test"]));
        assert!(!has_component("a/b.cpp", &[]));
    }

    #[test]
    fn an_empty_side_of_a_join_drops_its_slash() {
        assert_eq!(child("src", "a.py"), "src/a.py");
        assert_eq!(child("", "a.py"), "a.py");
        assert_eq!(child("src", ""), "src");
        assert_eq!(child("", ""), "");
    }

    #[test]
    fn only_a_lookup_that_yields_one_path_picks_it() {
        let found = |paths: &[&str]| sole(paths.iter().map(|&path| path.to_owned()));
        assert_eq!(found(&[]), None);
        assert_eq!(found(&["a/b.nut"]).as_deref(), Some("a/b.nut"));
        assert_eq!(
            found(&["a/b.nut", "c/b.nut"]),
            None,
            "two paths are a guess"
        );
    }

    #[test]
    fn a_backslash_reads_as_a_separator() {
        assert_eq!(forward_slashes(r"ui\hud\main.nut"), "ui/hud/main.nut");
        assert!(matches!(
            forward_slashes("ui/hud.nut"),
            Cow::Borrowed("ui/hud.nut")
        ));
    }

    #[test]
    fn a_relative_path_that_climbs_above_the_root_names_nothing() {
        assert_eq!(join(".", "../foo"), None);
        assert_eq!(join("a", "../../foo"), None);
    }

    #[test]
    fn two_paths_share_their_equal_leading_components() {
        assert_eq!(shared_segments("a/b/c", "a/b/d.blk"), 2);
        assert_eq!(shared_segments("a/b", "a/c"), 1);
        assert_eq!(shared_segments("a", "b/a"), 0);
        assert_eq!(shared_segments("", "a/b"), 0);
        assert_eq!(
            shared_segments(".", "."),
            1,
            "two files at the root share the root"
        );
    }
}
