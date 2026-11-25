use std::num::NonZeroUsize;
use std::path::PathBuf;
use std::time::Duration;

use kartta::BuildOptions;
use kartta_fs::{ContentIndexStorage, IgnoreSources};

/// What every build the server runs takes besides its root and its languages, whether the server follows the tree it
/// built, and how many roots it holds at once.
///
/// Each build field defaults to the engine's own default: no shared cache, no content index, and the ambient thread
/// pool. [`Config::watch`] defaults to following the tree, and [`Config::max_roots`] to [`Config::MAX_ROOTS`].
/// [`Config::from_env`] takes the server's defaults instead, which add a shared cache under the user's cache directory
/// and a mapped content index.
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
    /// [`kartta::Watcher`] started before the build walks the tree reports them, and refreshes itself from each change.
    /// Without it, the `refresh` tool alone takes a change in.
    pub watch: bool,
    /// How many roots the server holds an index of at once. An `index_root` of a root past it drops a root whose build
    /// failed, else the least recently asked root that is not building, so 1 holds one repository, which an
    /// `index_root` of another replaces.
    pub max_roots: NonZeroUsize,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            content_cache: None,
            content_cache_ttl: Duration::ZERO,
            content_index: None,
            threads: None,
            watch: true,
            max_roots: Self::MAX_ROOTS,
        }
    }
}

impl Config {
    /// How many roots the server holds at once by default.
    pub const MAX_ROOTS: NonZeroUsize = NonZeroUsize::new(4).expect("four is not zero");

    /// Reads the configuration from the process environment, and a warning for each variable whose value it could not
    /// read, which leaves that field at its default.
    ///
    /// | Variable | Field |
    /// | --- | --- |
    /// | `KARTTA_CACHE_DIR` | [`Config::content_cache`]; unset is `kartta` in the user's cache directory, empty none |
    /// | `KARTTA_CACHE_ENTRY_TTL_SECS` | [`Config::content_cache_ttl`]: a count of seconds; empty or `0` is a week |
    /// | `KARTTA_GREP_INDEX` | [`Config::content_index`]: `memory`, `file` or `mmap`; unset `mmap`, empty none |
    /// | `KARTTA_GREP_INDEX_DIR` | where a `file` or `mmap` index lives; empty or unset is the temporary directory |
    /// | `KARTTA_THREADS` | [`Config::threads`]: a positive count |
    /// | `KARTTA_WATCH` | [`Config::watch`]: `off`, `0`, `false` or `no` turns it off |
    /// | `KARTTA_MAX_ROOTS` | [`Config::max_roots`]: a positive count |
    ///
    /// The words of `KARTTA_GREP_INDEX` and `KARTTA_WATCH` read in any case.
    #[must_use]
    pub fn from_env() -> (Self, Vec<String>) {
        Self::from_vars(|name| std::env::var(name).ok())
    }

    fn from_vars(var: impl Fn(&str) -> Option<String>) -> (Self, Vec<String>) {
        let mut warnings = Vec::new();
        let content_cache = match var("KARTTA_CACHE_DIR") {
            Some(dir) if dir.is_empty() => None,
            Some(dir) => Some(PathBuf::from(dir)),
            None => user_cache_dir(&var, &mut warnings),
        };
        let content_cache_ttl = cache_ttl(var("KARTTA_CACHE_ENTRY_TTL_SECS"), &mut warnings);
        let index_dir = || {
            var("KARTTA_GREP_INDEX_DIR")
                .filter(|dir| !dir.is_empty())
                .map_or_else(std::env::temp_dir, PathBuf::from)
        };
        let storage = var("KARTTA_GREP_INDEX").map(|raw| raw.trim().to_ascii_lowercase());
        let content_index = match storage.as_deref() {
            Some("") => None,
            Some("memory") => Some(ContentIndexStorage::Memory),
            Some("file") => Some(ContentIndexStorage::File { dir: index_dir() }),
            None | Some("mmap") => Some(ContentIndexStorage::Mapped { dir: index_dir() }),
            Some(storage) => {
                warnings.push(format!(
                    "KARTTA_GREP_INDEX={storage:?} is none of memory, file and mmap; building a mapped content index"
                ));
                Some(ContentIndexStorage::Mapped { dir: index_dir() })
            }
        };
        let threads = count(var("KARTTA_THREADS"), &mut warnings, |raw| {
            format!("KARTTA_THREADS={raw:?} is not a positive count; running on every core")
        });
        let max_roots = count(var("KARTTA_MAX_ROOTS"), &mut warnings, |raw| {
            format!(
                "KARTTA_MAX_ROOTS={raw:?} is not a positive count; holding up to {} roots",
                Self::MAX_ROOTS
            )
        })
        .unwrap_or(Self::MAX_ROOTS);
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
            max_roots,
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

/// `kartta` under the user's cache directory: `$HOME/Library/Caches` on macOS, `%LOCALAPPDATA%` on Windows, and
/// elsewhere `$XDG_CACHE_HOME`, or `$HOME/.cache` where that is unset, empty or relative, as the XDG Base Directory
/// Specification 0.8 reads it. A relative base is no directory at all, since the server's working directory is its
/// client's choice, and a warning says so.
fn user_cache_dir(
    var: &impl Fn(&str) -> Option<String>,
    warnings: &mut Vec<String>,
) -> Option<PathBuf> {
    let absolute = |name: &str| var(name).map(PathBuf::from).filter(|dir| dir.is_absolute());
    let base = if cfg!(target_os = "macos") {
        absolute("HOME").map(|home| home.join("Library").join("Caches"))
    } else if cfg!(windows) {
        absolute("LOCALAPPDATA")
    } else {
        absolute("XDG_CACHE_HOME").or_else(|| absolute("HOME").map(|home| home.join(".cache")))
    };
    if base.is_none() {
        let named = if cfg!(windows) {
            "LOCALAPPDATA"
        } else {
            "HOME"
        };
        warnings.push(format!(
            "{named} is unset or not absolute, so there is no user cache directory; building without a content cache"
        ));
    }
    base.map(|base| base.join("kartta"))
}

/// The positive count `raw` holds, or `None` with the warning `unread` writes when it holds none.
fn count(
    raw: Option<String>,
    warnings: &mut Vec<String>,
    unread: impl FnOnce(&str) -> String,
) -> Option<NonZeroUsize> {
    let raw = raw?;
    let count = raw.trim().parse().ok();
    if count.is_none() {
        warnings.push(unread(&raw));
    }
    count
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

    /// Reads `vars` with the cache and the content index turned off, which leaves every field at the engine's default.
    fn read_off(vars: &[(&str, &str)]) -> (Config, Vec<String>) {
        read(&[vars, &OFF].concat())
    }

    const OFF: [(&str, &str); 2] = [("KARTTA_CACHE_DIR", ""), ("KARTTA_GREP_INDEX", "")];

    const USER_DIRS: [(&str, &str); 3] = [
        ("HOME", "/home/u"),
        ("XDG_CACHE_HOME", "/xdg"),
        ("LOCALAPPDATA", r"C:\Users\u\AppData\Local"),
    ];

    #[test]
    fn an_unset_cache_dir_is_kartta_under_the_user_cache_directory() {
        let user_cache = if cfg!(target_os = "macos") {
            "/home/u/Library/Caches/kartta"
        } else if cfg!(windows) {
            r"C:\Users\u\AppData\Local\kartta"
        } else {
            "/xdg/kartta"
        };

        let (config, warnings) = read(&USER_DIRS);

        assert_eq!(warnings, Vec::<String>::new());
        assert_eq!(config.content_cache, Some(PathBuf::from(user_cache)));
    }

    #[test]
    fn an_unset_grep_index_is_mapped_in_the_temporary_directory() {
        let (config, warnings) = read(&USER_DIRS);

        assert_eq!(warnings, Vec::<String>::new());
        assert_eq!(
            config.content_index,
            Some(ContentIndexStorage::Mapped {
                dir: std::env::temp_dir()
            })
        );
    }

    #[test]
    fn an_empty_cache_dir_and_grep_index_take_the_engine_defaults() {
        assert_eq!(
            read(&[&USER_DIRS[..], &OFF].concat()),
            (Config::default(), Vec::new())
        );
    }

    #[cfg(not(any(target_os = "macos", windows)))]
    #[test]
    fn a_relative_xdg_cache_home_falls_back_to_the_home_cache() {
        let (config, warnings) = read(&[("HOME", "/home/u"), ("XDG_CACHE_HOME", "xdg")]);

        assert_eq!(warnings, Vec::<String>::new());
        assert_eq!(
            config.content_cache,
            Some(PathBuf::from("/home/u/.cache/kartta"))
        );
    }

    #[test]
    fn no_user_cache_directory_warns_and_builds_without_a_cache() {
        let (config, warnings) = read(&[("KARTTA_GREP_INDEX", "")]);

        assert_eq!(config, Config::default());
        assert_eq!(warnings.len(), 1, "{warnings:?}");
        assert!(warnings[0].contains("no user cache directory"));
    }

    #[test]
    fn each_variable_sets_its_field() {
        let (config, warnings) = read(&[
            ("KARTTA_CACHE_DIR", "/cache"),
            ("KARTTA_CACHE_ENTRY_TTL_SECS", " 3600 "),
            ("KARTTA_GREP_INDEX", " MMAP "),
            ("KARTTA_GREP_INDEX_DIR", "/scratch"),
            ("KARTTA_THREADS", "4"),
            ("KARTTA_MAX_ROOTS", " 1 "),
        ]);

        assert_eq!(warnings, Vec::<String>::new());
        assert_eq!(config.max_roots, NonZeroUsize::MIN);
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
            ("KARTTA_MAX_ROOTS", "0"),
        ]);

        assert_eq!(
            config,
            Config {
                content_index: Some(ContentIndexStorage::Mapped {
                    dir: std::env::temp_dir()
                }),
                ..Config::default()
            }
        );
        assert_eq!(warnings.len(), 4, "{warnings:?}");
        assert!(warnings[0].contains("KARTTA_CACHE_ENTRY_TTL_SECS=\"a week\""));
        assert!(warnings[1].contains("KARTTA_GREP_INDEX=\"disk\""));
        assert!(warnings[2].contains("KARTTA_THREADS=\"0\""));
        assert_eq!(
            warnings[3],
            "KARTTA_MAX_ROOTS=\"0\" is not a positive count; holding up to 4 roots"
        );
    }

    #[test]
    fn a_cache_time_to_live_of_zero_is_the_default_week() {
        assert_eq!(
            read_off(&[("KARTTA_CACHE_ENTRY_TTL_SECS", "0")]),
            (Config::default(), Vec::new())
        );
    }

    #[test]
    fn kartta_watch_turns_the_watch_off_by_any_of_its_four_words() {
        for off in ["off", "0", "FALSE", " No "] {
            assert_eq!(
                read_off(&[("KARTTA_WATCH", off)]),
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
                read_off(&[("KARTTA_WATCH", on)]),
                (Config::default(), Vec::new()),
                "{on:?}"
            );
        }
    }

    #[test]
    fn an_unknown_kartta_watch_warns_and_follows_the_tree() {
        let (config, warnings) = read_off(&[("KARTTA_WATCH", "sometimes")]);

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
