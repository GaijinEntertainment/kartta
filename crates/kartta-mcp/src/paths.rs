//! The path arguments of the file and symbol tools, read as the index spells a path.

/// `path` without its leading `./`: the index spells a path from the root, where `./a` names what `a` names.
pub(crate) fn relative(path: &str) -> &str {
    path.trim_start_matches("./")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_leading_dot_slash_names_what_the_path_after_it_names() {
        assert_eq!(relative("./pkg/a.go"), "pkg/a.go");
        assert_eq!(relative("././pkg/"), "pkg/");
        assert_eq!(relative("pkg/./a.go"), "pkg/./a.go");
        assert_eq!(relative(".hidden/a"), ".hidden/a");
        assert_eq!(relative("."), ".");
    }
}
