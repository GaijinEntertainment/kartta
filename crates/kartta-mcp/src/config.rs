use std::num::NonZeroUsize;
use std::path::PathBuf;
use std::time::Duration;

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
    /// The age past which an entry of [`Config::content_cache`] expires, counted from when the build that computed it
    /// began, as [`BuildOptions::content_cache_ttl`] takes it; zero is a week.
    pub content_cache_ttl: Duration,
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
            content_cache_ttl: Duration::ZERO,
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
    /// | `KARTTA_CACHE_ENTRY_TTL_SECS` | [`Config::content_cache_ttl`]: a count of seconds; empty or `0` is a week |
    /// | `KARTTA_CACHE_SWEEP` | nothing: every build expires entries, so a value only warns that it does not apply |
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
        let content_cache_ttl = cache_ttl(var("KARTTA_CACHE_ENTRY_TTL_SECS"), &mut warnings);
        if var("KARTTA_CACHE_SWEEP").is_some_and(|raw| !raw.is_empty()) {
            warnings.push(
                "KARTTA_CACHE_SWEEP does not apply: every build's merge expires the cache entries older than \
                 KARTTA_CACHE_ENTRY_TTL_SECS, a week unless it says otherwise"
                    .to_owned(),
            );
        }
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
            content_cache_ttl,
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
        options.content_cache_ttl = self.content_cache_ttl;
        options.content_index.clone_from(&self.content_index);
        options.threads = self.threads;
        options
    }
}

fn cache_ttl(raw: Option<String>, warnings: &mut Vec<String>) -> Duration {
    let Some(raw) = raw.filter(|raw| !raw.trim().is_empty()) else {
        return Duration::ZERO;
    };
    raw.trim().parse().map_or_else(
        |_| {
            warnings.push(format!(
                "KARTTA_CACHE_ENTRY_TTL_SECS={raw:?} is not a count of seconds; keeping cache entries a week"
            ));
            Duration::ZERO
        },
        Duration::from_secs,
    )
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
            ("KARTTA_CACHE_ENTRY_TTL_SECS", " 3600 "),
            ("KARTTA_GREP_INDEX", " MMAP "),
            ("KARTTA_GREP_INDEX_DIR", "/scratch"),
            ("KARTTA_THREADS", "4"),
        ]);

        assert_eq!(warnings, Vec::<String>::new());
        assert_eq!(config.content_cache, Some(PathBuf::from("/cache")));
        assert_eq!(config.content_cache_ttl, Duration::from_hours(1));
        assert_eq!(
            config.build_options(false).content_cache_ttl,
            Duration::from_hours(1)
        );
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
            ("KARTTA_CACHE_ENTRY_TTL_SECS", "a week"),
        ]);

        assert_eq!(config, Config::default());
        assert_eq!(warnings.len(), 3, "{warnings:?}");
        assert!(warnings[0].contains("KARTTA_CACHE_ENTRY_TTL_SECS=\"a week\""));
        assert!(warnings[1].contains("KARTTA_GREP_INDEX=\"disk\""));
        assert!(warnings[2].contains("KARTTA_THREADS=\"0\""));
    }

    #[test]
    fn a_cache_time_to_live_of_zero_is_the_default_week() {
        assert_eq!(
            read(&[("KARTTA_CACHE_ENTRY_TTL_SECS", "0")]),
            (Config::default(), Vec::new())
        );
    }

    #[test]
    fn a_cache_sweep_setting_warns_that_it_does_not_apply() {
        let (config, warnings) = read(&[("KARTTA_CACHE_SWEEP", "1")]);

        assert_eq!(config, Config::default());
        assert_eq!(warnings.len(), 1, "{warnings:?}");
        assert!(warnings[0].starts_with("KARTTA_CACHE_SWEEP does not apply"));
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
