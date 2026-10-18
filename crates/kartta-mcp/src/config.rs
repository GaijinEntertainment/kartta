use std::num::NonZeroUsize;
use std::path::PathBuf;

use kartta::BuildOptions;
use kartta_fs::{ContentIndexStorage, IgnoreSources};

/// What every build the server runs takes besides its root and its languages, and whether the server follows the tree
/// it built.
///
/// Each build field defaults to the engine's own default: no shared cache, no content index, and the ambient thread
/// pool. [`Config::watch`] defaults to following the tree.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct Config {
    /// The [`kartta_cache`](https://crates.io/crates/kartta-cache) directory builds share per-file data through, as
    /// [`BuildOptions::content_cache`] takes it.
    pub content_cache: Option<PathBuf>,
    /// Where a build keeps the trigram index that narrows `grep`, as [`BuildOptions::content_index`] takes it; `None`
    /// builds none, and `grep` then reads every candidate.
    pub content_index: Option<ContentIndexStorage>,
    /// How many threads a build and every query run on, as [`BuildOptions::threads`] takes it.
    pub threads: Option<NonZeroUsize>,
    /// Whether a built index follows its tree through the operating system's change notifications, as a
    /// [`kartta_fs::Watcher`] reports them, and refreshes itself from each change. Without it, the `refresh` tool
    /// alone takes a change in.
    pub watch: bool,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            content_cache: None,
            content_index: None,
            threads: None,
            watch: true,
        }
    }
}

impl Config {
    /// Reads the configuration from the process environment, and a warning for each variable whose value it could not
    /// read, which leaves that field at its default.
    ///
    /// | Variable | Field |
    /// | --- | --- |
    /// | `KARTTA_CACHE_DIR` | [`Config::content_cache`], when not empty |
    /// | `KARTTA_GREP_INDEX` | [`Config::content_index`]: `memory`, `file` or `mmap`, in any case; empty builds none |
    /// | `KARTTA_GREP_INDEX_DIR` | where a `file` or `mmap` index lives; empty or unset is the temporary directory |
    /// | `KARTTA_THREADS` | [`Config::threads`]: a positive count |
    /// | `KARTTA_WATCH` | [`Config::watch`]: `off`, `0`, `false` or `no`, in any case, turns it off |
    #[must_use]
    pub fn from_env() -> (Self, Vec<String>) {
        Self::from_vars(|name| std::env::var(name).ok())
    }

    fn from_vars(var: impl Fn(&str) -> Option<String>) -> (Self, Vec<String>) {
        let mut warnings = Vec::new();
        let content_cache = var("KARTTA_CACHE_DIR")
            .filter(|dir| !dir.is_empty())
            .map(PathBuf::from);
        let index_dir = || {
            var("KARTTA_GREP_INDEX_DIR")
                .filter(|dir| !dir.is_empty())
                .map_or_else(std::env::temp_dir, PathBuf::from)
        };
        let storage = var("KARTTA_GREP_INDEX")
            .map(|raw| raw.trim().to_ascii_lowercase())
            .unwrap_or_default();
        let content_index = match storage.as_str() {
            "" => None,
            "memory" => Some(ContentIndexStorage::Memory),
            "file" => Some(ContentIndexStorage::File { dir: index_dir() }),
            "mmap" => Some(ContentIndexStorage::Mapped { dir: index_dir() }),
            _ => {
                warnings.push(format!(
                    "KARTTA_GREP_INDEX={storage:?} is none of memory, file and mmap; building no content index"
                ));
                None
            }
        };
        let threads = var("KARTTA_THREADS").and_then(|raw| {
            let count = raw.trim().parse::<NonZeroUsize>().ok();
            if count.is_none() {
                warnings.push(format!(
                    "KARTTA_THREADS={raw:?} is not a positive count; running on every core"
                ));
            }
            count
        });
        let watch = var("KARTTA_WATCH").is_none_or(|raw| {
            match raw.trim().to_ascii_lowercase().as_str() {
                "off" | "0" | "false" | "no" => false,
                "" | "on" | "1" | "true" | "yes" => true,
                _ => {
                    warnings.push(format!(
                        "KARTTA_WATCH={raw:?} is none of off, 0, false, no, on, 1, true and yes; following the tree"
                    ));
                    true
                }
            }
        });
        let config = Self {
            content_cache,
            content_index,
            threads,
            watch,
        };
        (config, warnings)
    }

    pub(crate) fn build_options(&self, index_ignored: bool) -> BuildOptions {
        let mut options = BuildOptions::default();
        options.stamps = true;
        options.ignore_sources = if index_ignored {
            IgnoreSources::None
        } else {
            IgnoreSources::InTree
        };
        options.content_cache.clone_from(&self.content_cache);
        options.content_index.clone_from(&self.content_index);
        options.threads = self.threads;
        options
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn read(vars: &[(&str, &str)]) -> (Config, Vec<String>) {
        Config::from_vars(|name| {
            vars.iter()
                .find(|(key, _)| *key == name)
                .map(|(_, value)| (*value).to_owned())
        })
    }

    #[test]
    fn an_unset_environment_takes_every_default() {
        assert_eq!(read(&[]), (Config::default(), Vec::new()));
    }

    #[test]
    fn each_variable_sets_its_field() {
        let (config, warnings) = read(&[
            ("KARTTA_CACHE_DIR", "/cache"),
            ("KARTTA_GREP_INDEX", " MMAP "),
            ("KARTTA_GREP_INDEX_DIR", "/scratch"),
            ("KARTTA_THREADS", "4"),
        ]);

        assert_eq!(warnings, Vec::<String>::new());
        assert_eq!(config.content_cache, Some(PathBuf::from("/cache")));
        assert_eq!(
            config.content_index,
            Some(ContentIndexStorage::Mapped {
                dir: PathBuf::from("/scratch")
            })
        );
        assert_eq!(config.threads, NonZeroUsize::new(4));
    }

    #[test]
    fn a_value_that_does_not_read_warns_and_keeps_the_default() {
        let (config, warnings) = read(&[
            ("KARTTA_GREP_INDEX", "disk"),
            ("KARTTA_THREADS", "0"),
            ("KARTTA_CACHE_DIR", ""),
        ]);

        assert_eq!(config, Config::default());
        assert_eq!(warnings.len(), 2, "{warnings:?}");
        assert!(warnings[0].contains("KARTTA_GREP_INDEX=\"disk\""));
        assert!(warnings[1].contains("KARTTA_THREADS=\"0\""));
    }

    #[test]
    fn kartta_watch_turns_the_watch_off_by_any_of_its_four_words() {
        for off in ["off", "0", "FALSE", " No "] {
            assert_eq!(
                read(&[("KARTTA_WATCH", off)]),
                (
                    Config {
                        watch: false,
                        ..Config::default()
                    },
                    Vec::new()
                ),
                "{off:?}"
            );
        }
        for on in ["on", "1", "True", "yes", ""] {
            assert_eq!(
                read(&[("KARTTA_WATCH", on)]),
                (Config::default(), Vec::new()),
                "{on:?}"
            );
        }
    }

    #[test]
    fn an_unknown_kartta_watch_warns_and_follows_the_tree() {
        let (config, warnings) = read(&[("KARTTA_WATCH", "sometimes")]);

        assert!(config.watch);
        assert_eq!(warnings.len(), 1, "{warnings:?}");
        assert!(warnings[0].contains("KARTTA_WATCH=\"sometimes\""));
    }

    #[test]
    fn every_build_keeps_what_a_refresh_reads_only_the_changes_by() {
        assert!(Config::default().build_options(false).stamps);
    }

    #[test]
    fn index_ignored_applies_no_ignore_file() {
        let config = Config::default();

        assert_eq!(
            config.build_options(false).ignore_sources,
            IgnoreSources::InTree
        );
        assert_eq!(
            config.build_options(true).ignore_sources,
            IgnoreSources::None
        );
    }
}
