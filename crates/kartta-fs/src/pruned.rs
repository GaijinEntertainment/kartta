//! [`PrunedNames`] — the entry names a build neither indexes nor descends.

use std::borrow::Cow;

/// The entry names a build neither indexes nor descends: what an editor or the operating system leaves in a tree, as
/// opposed to what the tree holds.
///
/// A name prunes every entry it equals, of any kind and at any depth — a directory together with everything beneath it,
/// a file, a symlink. Only the entry's own name is compared, exactly and case included, and the root a build is given
/// is never pruned, whatever its name.
///
/// `.git` is not in the built-in set, and a build prunes it whatever its set holds, so a set that holds `.git` and one
/// that does not prune alike: it marks where a repository's ignore rules start, which is a fact about the tree rather
/// than clutter a caller may want back.
///
/// # Composing a set
///
/// [`PrunedNames::builtin`] is the [`Default`]: `.DS_Store`, `.fleet`, `.idea` and `.vscode`. A set is built from it
/// and the caller's own names with [`union`](Self::union) and [`difference`](Self::difference), and
/// [`PrunedNames::none`] prunes nothing beyond `.git`. A name no entry in a tree carries is not an error, and a name a
/// caller meant and misspelled shows itself: the entries it was meant to prune are in the index.
///
/// # Examples
///
/// ```
/// use kartta_fs::PrunedNames;
///
/// # fn main() -> Result<(), Box<dyn std::error::Error>> {
/// let pruned = PrunedNames::builtin()
///     .difference(&PrunedNames::new([".vscode"])?)
///     .union(&PrunedNames::new(["node_modules"])?);
///
/// assert!(pruned.matches("node_modules"));
/// assert!(pruned.matches(".idea"));
/// assert!(!pruned.matches(".vscode"));
/// assert!(!pruned.matches(".IDEA"), "case is compared");
/// # Ok(())
/// # }
/// ```
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct PrunedNames {
    names: Vec<Cow<'static, str>>,
}

impl PrunedNames {
    /// The names editors and the operating system leave in a tree: `.DS_Store`, `.fleet`, `.idea` and `.vscode`.
    #[must_use]
    pub fn builtin() -> Self {
        Self::normalized(BUILTIN.iter().map(|&name| Cow::Borrowed(name)).collect())
    }

    /// A set that prunes nothing beyond `.git`.
    #[must_use]
    pub const fn none() -> Self {
        Self { names: Vec::new() }
    }

    /// A set of the caller's own names.
    ///
    /// # Errors
    ///
    /// Returns [`PrunedNamesError`] for the first given name that no entry of a tree can carry, naming it as it was
    /// given.
    pub fn new<I, N>(names: I) -> Result<Self, PrunedNamesError>
    where
        I: IntoIterator<Item = N>,
        N: AsRef<str>,
    {
        let parsed = names
            .into_iter()
            .map(|name| parse(name.as_ref()).map(Cow::Owned))
            .collect::<Result<_, _>>()?;

        Ok(Self::normalized(parsed))
    }

    /// This set with every name of `other` added.
    #[must_use]
    pub fn union(mut self, other: &Self) -> Self {
        self.names.extend(other.names.iter().cloned());

        Self::normalized(self.names)
    }

    /// This set with every name of `other` taken out.
    #[must_use]
    pub fn difference(mut self, other: &Self) -> Self {
        self.names
            .retain(|name| other.names.binary_search(name).is_err());

        self
    }

    /// The names in the set, in byte order.
    pub fn names(&self) -> impl ExactSizeIterator<Item = &str> {
        self.names.iter().map(AsRef::as_ref)
    }

    /// Whether an entry named `name` is pruned by this set. `.git` is pruned whatever this answers.
    #[must_use]
    pub fn matches(&self, name: &str) -> bool {
        self.names
            .binary_search_by(|known| (**known).cmp(name))
            .is_ok()
    }

    fn normalized(mut names: Vec<Cow<'static, str>>) -> Self {
        names.sort_unstable();
        names.dedup();

        Self { names }
    }
}

impl Default for PrunedNames {
    fn default() -> Self {
        Self::builtin()
    }
}

/// Why a given name cannot join a [`PrunedNames`] set: no entry of a tree can carry it.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[non_exhaustive]
pub enum PrunedNamesError {
    /// The name is empty.
    #[error("name {name:?} is empty")]
    Empty {
        /// The name as it was given.
        name: String,
    },
    /// The name is `.` or `..`, which name a directory itself or its parent rather than an entry in it.
    #[error("name {name:?} names a directory or its parent rather than an entry in it")]
    Dots {
        /// The name as it was given.
        name: String,
    },
    /// The name holds a path separator, which no single path segment can: `/`, and on Windows `\` as well.
    #[error("name {name:?} holds a path separator, which no entry's name can")]
    Separator {
        /// The name as it was given.
        name: String,
    },
    /// The name holds a NUL, which no entry's name can.
    #[error("name {name:?} holds a NUL, which no entry's name can")]
    Nul {
        /// The name as it was given.
        name: String,
    },
}

const BUILTIN: &[&str] = &[".DS_Store", ".fleet", ".idea", ".vscode"];

fn parse(given: &str) -> Result<String, PrunedNamesError> {
    let name = || given.to_owned();

    match given {
        "" => Err(PrunedNamesError::Empty { name: name() }),
        "." | ".." => Err(PrunedNamesError::Dots { name: name() }),
        _ if given.contains('/') || (cfg!(windows) && given.contains('\\')) => {
            Err(PrunedNamesError::Separator { name: name() })
        }
        _ if given.contains('\0') => Err(PrunedNamesError::Nul { name: name() }),
        _ => Ok(name()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_builtin_set_is_editor_and_os_clutter_and_the_default() {
        let builtin = PrunedNames::builtin();

        assert_eq!(
            builtin.names().collect::<Vec<_>>(),
            [".DS_Store", ".fleet", ".idea", ".vscode"]
        );
        assert_eq!(PrunedNames::default(), builtin);
    }

    #[test]
    fn a_name_matches_exactly_and_by_the_whole_name() {
        let builtin = PrunedNames::builtin();

        assert!(builtin.matches(".idea"));
        assert!(builtin.matches(".DS_Store"));
        assert!(!builtin.matches(".Idea"), "case is compared");
        assert!(!builtin.matches(".ds_store"), "case is compared");
        assert!(!builtin.matches(".idea2"), "a longer name is another name");
        assert!(!builtin.matches("idea"), "and so is a shorter one");
        assert!(
            !builtin.matches(".git"),
            "`.git` is pruned apart from any set"
        );
    }

    #[test]
    fn none_prunes_nothing() {
        let none = PrunedNames::none();

        assert_eq!(none.names().len(), 0);
        assert!(!none.matches(".idea"));
        assert_eq!(
            PrunedNames::new([""; 0]).expect("an empty set"),
            none,
            "an empty set compares by its members"
        );
    }

    #[test]
    fn union_adds_and_difference_takes_out() {
        let pruned = PrunedNames::builtin()
            .difference(&PrunedNames::new([".vscode", "absent"]).expect("names"))
            .union(&PrunedNames::new(["node_modules", ".idea"]).expect("names"));

        assert_eq!(
            pruned.names().collect::<Vec<_>>(),
            [".DS_Store", ".fleet", ".idea", "node_modules"],
            "each name once, in byte order, and taking out a name the set lacks changes nothing"
        );
    }

    #[test]
    fn a_name_no_entry_can_carry_is_refused_by_name() {
        let refused = |given: &str| PrunedNames::new([given]).expect_err("the name is refused");

        assert_eq!(
            refused(""),
            PrunedNamesError::Empty {
                name: String::new()
            }
        );
        assert_eq!(
            refused("."),
            PrunedNamesError::Dots {
                name: ".".to_owned()
            }
        );
        assert_eq!(
            refused(".."),
            PrunedNamesError::Dots {
                name: "..".to_owned()
            }
        );
        assert_eq!(
            refused("a/b"),
            PrunedNamesError::Separator {
                name: "a/b".to_owned()
            }
        );
        assert_eq!(
            refused("a\0b"),
            PrunedNamesError::Nul {
                name: "a\0b".to_owned()
            }
        );
        assert!(
            PrunedNames::new(["...", ".hidden", "with space"]).is_ok(),
            "only `.` and `..` are refused for their dots"
        );
    }

    #[test]
    fn a_backslash_separates_only_where_the_platform_says_so() {
        let backslashed = PrunedNames::new(["a\\b"]);

        if cfg!(windows) {
            assert_eq!(
                backslashed,
                Err(PrunedNamesError::Separator {
                    name: "a\\b".to_owned()
                })
            );
        } else {
            assert!(backslashed.is_ok(), "a unix name may hold a backslash");
        }
    }

    #[test]
    fn the_first_refused_name_is_the_one_named() {
        assert_eq!(
            PrunedNames::new([".idea", "a/b", ""]),
            Err(PrunedNamesError::Separator {
                name: "a/b".to_owned()
            })
        );
    }

    #[test]
    fn every_builtin_name_is_one_new_accepts() {
        for name in BUILTIN {
            assert_eq!(
                parse(name).as_deref(),
                Ok(*name),
                "{name} could not be given by a caller"
            );
        }
    }
}
