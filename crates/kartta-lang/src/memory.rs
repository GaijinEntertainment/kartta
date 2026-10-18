use std::io;

use crate::language::Repository;
use crate::path::{base, dir};

/// A [`Repository`] held in memory, as a test of a language's resolver builds one: files named by root-relative,
/// forward-slash paths, each with the content a read of it answers.
///
/// # Examples
///
/// ```
/// use kartta_lang::{MemoryRepository, Repository};
///
/// let repository = MemoryRepository::new([("go.mod", "module example.com/app\n"), ("cmd/main.go", "")]);
///
/// assert!(repository.contains("cmd/main.go"));
/// assert_eq!(repository.files_named("go.mod").collect::<Vec<_>>(), ["go.mod"]);
/// assert_eq!(repository.read("go.mod")?, b"module example.com/app\n");
/// # Ok::<(), std::io::Error>(())
/// ```
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct MemoryRepository {
    files: Vec<(String, Option<Vec<u8>>)>,
}

impl MemoryRepository {
    /// A repository of `files`, each a path and its content.
    #[must_use]
    pub fn new<P: AsRef<str>, C: AsRef<[u8]>>(files: impl IntoIterator<Item = (P, C)>) -> Self {
        Self {
            files: (files.into_iter())
                .map(|(path, content)| (path.as_ref().to_owned(), Some(content.as_ref().to_vec())))
                .collect(),
        }
    }

    /// A repository of `paths` whose reads fail as for files deleted since they were indexed.
    #[must_use]
    pub fn of_paths<P: AsRef<str>>(paths: impl IntoIterator<Item = P>) -> Self {
        Self::default().with_paths(paths)
    }

    /// This repository with `paths` added after its files, each failing to read as [`MemoryRepository::of_paths`]
    /// makes them.
    #[must_use]
    pub fn with_paths<P: AsRef<str>>(mut self, paths: impl IntoIterator<Item = P>) -> Self {
        (self.files).extend(
            paths
                .into_iter()
                .map(|path| (path.as_ref().to_owned(), None)),
        );
        self
    }

    fn paths(&self) -> impl Iterator<Item = &str> {
        self.files.iter().map(|(path, _)| path.as_str())
    }

    fn matching<'a>(
        &'a self,
        keep: impl Fn(&str) -> bool + 'a,
    ) -> Box<dyn Iterator<Item = String> + 'a> {
        Box::new(
            self.paths()
                .filter(move |path| keep(path))
                .map(str::to_owned),
        )
    }
}

impl Repository for MemoryRepository {
    fn contains(&self, path: &str) -> bool {
        self.paths().any(|file| file == path)
    }

    fn files_in_dir<'a>(&'a self, in_dir: &str) -> Box<dyn Iterator<Item = String> + 'a> {
        let in_dir = if in_dir.is_empty() { "." } else { in_dir }.to_owned();
        self.matching(move |file| dir(file) == in_dir)
    }

    fn files_named<'a>(&'a self, name: &str) -> Box<dyn Iterator<Item = String> + 'a> {
        let name = name.to_owned();
        self.matching(move |file| base(file) == name)
    }

    fn files_ignoring_case<'a>(&'a self, path: &str) -> Box<dyn Iterator<Item = String> + 'a> {
        let path = path.to_owned();
        self.matching(move |file| file.eq_ignore_ascii_case(&path))
    }

    fn read(&self, path: &str) -> io::Result<Vec<u8>> {
        (self.files.iter())
            .find(|(file, _)| file == path)
            .and_then(|(_, content)| content.clone())
            .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, path.to_owned()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_directory_lists_only_the_files_directly_in_it_and_both_spellings_name_the_root() {
        let repository = MemoryRepository::of_paths(["a.go", "src/b.go", "src/deep/c.go"]);

        assert_eq!(
            repository.files_in_dir("src").collect::<Vec<_>>(),
            ["src/b.go"]
        );
        assert_eq!(repository.files_in_dir(".").collect::<Vec<_>>(), ["a.go"]);
        assert_eq!(repository.files_in_dir("").collect::<Vec<_>>(), ["a.go"]);
    }

    #[test]
    fn a_path_compared_without_case_finds_each_file_that_differs_only_in_ascii_case() {
        let repository = MemoryRepository::of_paths(["UI/Hud.nut", "ui/hud.nut", "ui/other.nut"]);

        assert_eq!(
            repository
                .files_ignoring_case("ui/HUD.nut")
                .collect::<Vec<_>>(),
            ["UI/Hud.nut", "ui/hud.nut"]
        );
    }

    #[test]
    fn a_file_without_content_and_a_missing_one_both_fail_to_read() {
        let repository = MemoryRepository::of_paths(["Cargo.toml"]);

        assert!(repository.contains("Cargo.toml"));
        assert_eq!(
            repository.read("Cargo.toml").map_err(|error| error.kind()),
            Err(io::ErrorKind::NotFound)
        );
        assert_eq!(
            repository.read("absent.toml").map_err(|error| error.kind()),
            Err(io::ErrorKind::NotFound)
        );
    }
}
