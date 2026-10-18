use std::collections::HashMap;

use kartta_lang::path::{forward_slashes, join};

pub(crate) const SQCONFIG: &str = ".sqconfig";

const MAX_MOUNT_NAME_LEN: usize = 64;

#[derive(Debug, Default, PartialEq, Eq)]
pub(crate) struct SqConfig {
    pub(crate) mounts: HashMap<String, String>,
    pub(crate) base_paths: Vec<String>,
}

impl SqConfig {
    // constraint: the script tooling registers each `mount` and `base_path` line of the file relative to the file's
    // directory; it reads the file as a key-value text whose `include` lines and `\` continuations this does not follow
    pub(crate) fn parse(config_dir: &str, contents: &str) -> Self {
        let mut config = Self::default();

        for (key, value) in key_values(contents) {
            match key {
                "mount" => config.mount(config_dir, value),
                "base_path" if !is_absolute(value) => {
                    config.base_paths.extend(in_repository(config_dir, value));
                }
                _ => {}
            }
        }

        config
    }

    fn mount(&mut self, config_dir: &str, value: &str) {
        let Some((name, path)) = value.split_once('=') else {
            return;
        };
        let Some(name) = mount_name(name) else {
            return;
        };

        match in_repository(config_dir, path) {
            Some(dir) => {
                self.mounts.insert(name.to_owned(), dir);
            }
            None => {
                self.mounts.remove(name);
            }
        }
    }
}

fn key_values(contents: &str) -> impl Iterator<Item = (&str, &str)> {
    let contents = contents.strip_prefix('\u{feff}').unwrap_or(contents);

    contents.lines().filter_map(|line| {
        let line = without_comment(line).trim_start_matches([' ', '\t']);
        let (key, rest) = line.split_at(line.find([' ', '\t', '=']).unwrap_or(line.len()));
        let (_, value) = rest.split_once('=')?;
        let value = value.trim_matches([' ', '\t']);

        (!key.is_empty() && !value.is_empty()).then_some((key, value))
    })
}

// constraint: a `;` starts a comment only at the start of a line or after a space or a tab
fn without_comment(line: &str) -> &str {
    let bytes = line.as_bytes();
    let start = (0..bytes.len())
        .find(|&at| bytes[at] == b';' && (at == 0 || matches!(bytes[at - 1], b' ' | b'\t')));

    start.map_or(line, |start| &line[..start])
}

// constraint: the engine's named mounts take a name of ASCII letters, digits and `_` shorter than 64 bytes, after
// one leading `%`
fn mount_name(name: &str) -> Option<&str> {
    let name = name.strip_prefix('%').unwrap_or(name);
    let valid = name.len() < MAX_MOUNT_NAME_LEN
        && name
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'_');

    valid.then_some(name)
}

fn is_absolute(path: &str) -> bool {
    path.starts_with('/') || path.contains(':')
}

fn in_repository(config_dir: &str, path: &str) -> Option<String> {
    join(config_dir, &forward_slashes(path))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn mounts(entries: &[(&str, &str)]) -> HashMap<String, String> {
        entries
            .iter()
            .map(|&(name, dir)| (name.to_owned(), dir.to_owned()))
            .collect()
    }

    #[test]
    fn mount_and_base_path_lines_resolve_against_the_directory_of_the_file() {
        let config = SqConfig::parse(
            "enlisted/prog",
            "\u{feff}mount = %x=kept\n; mount = %y=ignored\nmount = %darg=../../prog/scripts/sq/daRg/\n\
             mount=scripts=scripts\nmount = %ui=\n  base_path\t= ../../prog/scripts/sq/stubs/ ;adds basepaths\n\
             base_path = .\n",
        );

        assert_eq!(
            config,
            SqConfig {
                mounts: mounts(&[
                    ("x", "enlisted/prog/kept"),
                    ("darg", "prog/scripts/sq/daRg"),
                    ("scripts", "enlisted/prog/scripts"),
                    ("ui", "enlisted/prog"),
                ]),
                base_paths: vec![
                    "prog/scripts/sq/stubs".to_owned(),
                    "enlisted/prog".to_owned()
                ],
            }
        );
    }

    #[test]
    fn a_later_mount_of_a_name_replaces_an_earlier_one_and_invalid_names_map_nothing() {
        let config = SqConfig::parse(
            "game",
            "mount = %a=one\nmount = %a=two\nmount = %b=kept\nmount = %b=../../outside\n\
             mount = %bad-name=x\nmount = %spaced =x\nmount = %%twice=x\nmount = noequals\n\
             mount = %Case=c\n",
        );

        assert_eq!(
            config.mounts,
            mounts(&[("a", "game/two"), ("Case", "game/c")])
        );
    }

    #[test]
    fn a_mount_name_is_short_and_spelled_with_word_bytes() {
        let longest = "n".repeat(MAX_MOUNT_NAME_LEN - 1);
        let too_long = "n".repeat(MAX_MOUNT_NAME_LEN);

        assert_eq!(mount_name(&format!("%{longest}")), Some(longest.as_str()));
        assert_eq!(mount_name(&too_long), None);
        assert_eq!(mount_name("plain_1"), Some("plain_1"));
    }

    #[test]
    fn base_paths_outside_the_repository_are_dropped_and_the_order_is_kept() {
        let config = SqConfig::parse(
            "game/prog",
            "base_path = /abs/stubs\nbase_path = C:/stubs\nbase_path = ../../../up\nbase_path = b/\n\
             base_path = a\nbase_path =\nbase_path stubs\n",
        );

        assert_eq!(config.base_paths, ["game/prog/b", "game/prog/a"]);
    }

    #[test]
    fn a_semicolon_inside_a_word_is_part_of_the_value() {
        let config = SqConfig::parse(".", "base_path = a;b ; trailing\n");

        assert_eq!(config.base_paths, ["a;b"]);
    }
}
