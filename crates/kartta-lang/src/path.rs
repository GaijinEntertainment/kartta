//! Root-relative, forward-slash paths, as a [`Repository`](crate::Repository) names files.

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
    fn a_relative_path_that_climbs_above_the_root_names_nothing() {
        assert_eq!(join(".", "../foo"), None);
        assert_eq!(join("a", "../../foo"), None);
    }
}
