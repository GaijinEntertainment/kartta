//! A Go file's build constraint: how its header states one, and whether any build satisfies two of them.
//!
//! A constraint is kept in the syntax of a `//go:build` line with its spaces taken out, which [`evaluate`] reads.

use super::{ARCHITECTURES, OPERATING_SYSTEMS, is_architecture, is_system};

// constraint: go/build/constraint's `maxSize`, past which `Parse` refuses the line; it bounds this file's recursion too
const MOST_TERMS: usize = 1000;

// constraint: go/build/constraint's `maxOldSize`
const MOST_PLUS_BUILD_OPERATORS: usize = 100;

// shortcut: past this many builds a pair reads as built together, which holds a pair under about a millisecond — raise
// it if a tree shows such a pair
const MOST_BUILDS: usize = 16_384;

const BYTE_ORDER_MARK: &[u8] = b"\xef\xbb\xbf";

const GO_BUILD: &[u8] = b"//go:build";

const PLUS_BUILD: &str = "+build";

// constraint: the go command's `-compiler` takes these two alone (`cmd/go/internal/work/build.go`), and go/build reads
// the one in use as `Context.Compiler`
const COMPILERS: [&str; 2] = ["gc", "gccgo"];

const RELEASE_PREFIX: &str = "go1.";

/// The constraint the header of a Go file states, or `None` where it states none go/build would read.
pub(crate) fn of_header(source: &[u8]) -> Option<String> {
    let header = Header::read(source)?;
    match header.go_build {
        Some(line) => {
            let line = str::from_utf8(line).ok()?;
            let expression = line.strip_prefix("//go:build")?;
            evaluate(expression, |_| true)?;
            Some(expression.split_ascii_whitespace().collect())
        }
        None => plus_build(header.leading),
    }
}

// constraint: as go/build's `parseFileHeader`: a `//go:build` line counts anywhere above the first text that is no
// comment, outside a `/*` comment, and `// +build` lines only above the last blank line that no `/*` comment precedes
/// What `shouldBuild` reads of a file.
struct Header<'s> {
    leading: &'s [u8],
    go_build: Option<&'s [u8]>,
}

impl<'s> Header<'s> {
    /// The header of `source`, or `None` where two `//go:build` lines make it an error.
    fn read(source: &'s [u8]) -> Option<Self> {
        let content = source.strip_prefix(BYTE_ORDER_MARK).unwrap_or(source);
        let mut header = Self {
            leading: &[],
            go_build: None,
        };
        let (mut read, mut ended, mut in_comment) = (0, false, false);
        for raw in content.split_inclusive(|&byte| byte == b'\n') {
            read += raw.len();
            let line = raw.trim_ascii();
            if line.is_empty() && !ended {
                header.leading = &content[..read];
                continue;
            }
            ended |= !line.starts_with(b"//");
            if !in_comment && is_go_build(line) && header.go_build.replace(line).is_some() {
                return None;
            }
            if !skip_comments(line, &mut in_comment) {
                break;
            }
        }
        Some(header)
    }
}

fn is_go_build(line: &[u8]) -> bool {
    line.strip_prefix(GO_BUILD)
        .is_some_and(|rest| rest.first().is_none_or(u8::is_ascii_whitespace))
}

/// Walks `line`'s comments from `in_comment`, the state a `/*` left it in, and answers whether only comments fill it.
fn skip_comments(mut line: &[u8], in_comment: &mut bool) -> bool {
    while !line.is_empty() {
        if *in_comment {
            let Some(end) = line.windows(2).position(|pair| pair == b"*/") else {
                return true;
            };
            *in_comment = false;
            line = line.get(end + 2..).unwrap_or_default().trim_ascii();
        } else if line.starts_with(b"//") {
            return true;
        } else if let Some(rest) = line.strip_prefix(b"/*") {
            *in_comment = true;
            line = rest.trim_ascii();
        } else {
            return false;
        }
    }
    true
}

/// The conjunction of the `// +build` lines of `leading`.
fn plus_build(leading: &[u8]) -> Option<String> {
    let mut lines: Vec<String> = leading
        .split(|&byte| byte == b'\n')
        .filter_map(|line| plus_build_line(str::from_utf8(line).ok()?.trim_ascii()))
        .collect();
    if lines.len() > 1 {
        Some(format!("({})", lines.join(")&&(")))
    } else {
        lines.pop()
    }
}

/// A `// +build` line as one expression; `None` for any other line and for one too large for go/build.
fn plus_build_line(line: &str) -> Option<String> {
    let text = line
        .strip_prefix("//")?
        .trim_ascii()
        .strip_prefix(PLUS_BUILD)?;
    if text.starts_with(|c: char| !c.is_ascii_whitespace()) {
        return None;
    }
    let clauses: Vec<String> = text
        .split_ascii_whitespace()
        .map(|clause| {
            let terms: Vec<String> = clause.split(',').map(plus_build_term).collect();
            terms.join("&&")
        })
        .collect();
    let operators = clauses.len().saturating_sub(1)
        + clauses
            .iter()
            .map(|clause| clause.matches("&&").count())
            .sum::<usize>();
    if operators > MOST_PLUS_BUILD_OPERATORS {
        return None;
    }
    Some(if clauses.is_empty() {
        "ignore".to_owned()
    } else {
        clauses.join("||")
    })
}

// constraint: go/build/constraint's `parsePlusBuildExpr` reads a word it cannot take as a tag as the tag `ignore`
fn plus_build_term(term: &str) -> String {
    let (negation, tag) = term.strip_prefix('!').map_or(("", term), |tag| ("!", tag));
    if tag.starts_with('!') || !is_tag(tag) {
        let negation = if tag.starts_with('!') || tag.is_empty() {
            ""
        } else {
            negation
        };
        return format!("{negation}ignore");
    }
    format!("{negation}{tag}")
}

fn is_tag(word: &str) -> bool {
    !word.is_empty() && word.chars().all(is_tag_character)
}

fn is_tag_character(c: char) -> bool {
    c.is_alphanumeric() || c == '_' || c == '.'
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Token<'e> {
    End,
    Open,
    Close,
    Not,
    And,
    Or,
    Tag(&'e str),
}

/// A reader of one `//go:build` expression, by go/build/constraint's `exprParser`.
struct Expression<'e, T> {
    rest: &'e str,
    token: Token<'e>,
    terms: usize,
    holds: T,
}

/// The value of `expression` where `holds` says which tags are set, or `None` where go/build refuses its syntax.
fn evaluate(expression: &str, holds: impl Fn(&str) -> bool) -> Option<bool> {
    let mut reader = Expression {
        rest: expression,
        token: Token::End,
        terms: 0,
        holds,
    };
    let value = reader.either()?;
    (reader.token == Token::End).then_some(value)
}

impl<T: Fn(&str) -> bool> Expression<'_, T> {
    fn either(&mut self) -> Option<bool> {
        let mut value = self.both()?;
        while self.token == Token::Or {
            value |= self.both()?;
        }
        Some(value)
    }

    fn both(&mut self) -> Option<bool> {
        let mut value = self.term()?;
        while self.token == Token::And {
            value &= self.term()?;
        }
        Some(value)
    }

    fn term(&mut self) -> Option<bool> {
        self.terms += 1;
        if self.terms > MOST_TERMS {
            return None;
        }
        self.lex()?;
        if self.token != Token::Not {
            return self.atom();
        }
        self.lex()?;
        self.atom().map(|value| !value)
    }

    fn atom(&mut self) -> Option<bool> {
        match self.token {
            Token::Open => {
                let value = self.either()?;
                if self.token != Token::Close {
                    return None;
                }
                self.lex()?;
                Some(value)
            }
            Token::Tag(tag) => {
                self.lex()?;
                Some((self.holds)(tag))
            }
            _ => None,
        }
    }

    fn lex(&mut self) -> Option<()> {
        let rest = self.rest.trim_start_matches([' ', '\t']);
        let (token, length) = match rest.as_bytes() {
            [] => (Token::End, 0),
            [b'(', ..] => (Token::Open, 1),
            [b')', ..] => (Token::Close, 1),
            [b'!', ..] => (Token::Not, 1),
            [b'&', b'&', ..] => (Token::And, 2),
            [b'|', b'|', ..] => (Token::Or, 2),
            _ => {
                let end = rest.find(|c| !is_tag_character(c)).unwrap_or(rest.len());
                if end == 0 {
                    return None;
                }
                (Token::Tag(rest.get(..end)?), end)
            }
        };
        self.token = token;
        self.rest = rest.get(length..)?;
        Some(())
    }
}

fn release_of(tag: &str) -> Option<u16> {
    tag.strip_prefix(RELEASE_PREFIX)?.parse().ok()
}

// constraint: go/build's `matchTag` sets `linux` under GOOS=android, `solaris` under illumos and `darwin` under ios
fn general_system_of(system: &str) -> Option<&'static str> {
    match system {
        "android" => Some("linux"),
        "illumos" => Some("solaris"),
        "ios" => Some("darwin"),
        _ => None,
    }
}

// constraint: internal/syslist's `UnixOS`
fn is_unix(system: &str) -> bool {
    !matches!(
        system,
        "js" | "nacl" | "plan9" | "wasip1" | "windows" | "zos"
    )
}

/// One build a pair of constraints can meet: what go/build's `matchTag` sets, narrowed to the tags the pair names.
struct Build<'t> {
    system: &'t str,
    architecture: &'t str,
    compiler: &'t str,
    release: u16,
    other_tags: &'t [&'t str],
    set: usize,
}

impl Build<'_> {
    fn sets(&self, tag: &str) -> bool {
        tag == self.system
            || tag == self.architecture
            || tag == self.compiler
            || general_system_of(self.system) == Some(tag)
            || (tag == "unix" && is_unix(self.system))
            || release_of(tag).is_some_and(|release| release <= self.release)
            || (self.other_tags.iter())
                .position(|&other| other == tag)
                .is_some_and(|at| self.set >> at & 1 == 1)
    }
}

/// The tags two constraints name, by what go/build makes of each.
#[derive(Default)]
struct Named<'e> {
    systems: Vec<&'e str>,
    architectures: Vec<&'e str>,
    names_a_compiler: bool,
    releases: Vec<u16>,
    others: Vec<&'e str>,
}

fn keep_once<T: PartialEq>(kept: &mut Vec<T>, item: T) {
    if !kept.contains(&item) {
        kept.push(item);
    }
}

impl<'e> Named<'e> {
    fn of(expressions: [&'e str; 2]) -> Self {
        let mut named = Self::default();
        let tags = expressions
            .into_iter()
            .flat_map(|expression| expression.split(['&', '|', '!', '(', ')']));
        for tag in tags.filter(|tag| !tag.is_empty()) {
            if is_system(tag) {
                keep_once(&mut named.systems, tag);
            } else if is_architecture(tag) {
                keep_once(&mut named.architectures, tag);
            } else if let Some(release) = release_of(tag) {
                keep_once(&mut named.releases, release);
            } else if tag == "gc" || tag == "gccgo" {
                named.names_a_compiler = true;
            } else if tag != "unix" {
                keep_once(&mut named.others, tag);
            }
        }
        named
    }

    // constraint: of the systems the pair does not name, every one sets the same tags but `unix`, and one a named
    // system covers sets what that one does, so one of each kind stands for the rest
    fn unnamed_system(&self, unix: bool) -> Option<&'static str> {
        let named = |system: &str| self.systems.contains(&system);
        OPERATING_SYSTEMS.into_iter().find(|&system| {
            is_unix(system) == unix
                && !named(system)
                && !general_system_of(system).is_some_and(named)
        })
    }

    /// Whether some build sets what `holds` asks of it; `true` past [`MOST_BUILDS`], which the pair leaves unread.
    fn some_build(mut self, holds: impl Fn(&Build<'_>) -> bool) -> bool {
        self.systems.extend(self.unnamed_system(true));
        self.systems.extend(self.unnamed_system(false));
        let unnamed = ARCHITECTURES
            .into_iter()
            .find(|architecture| !self.architectures.contains(architecture));
        self.architectures.extend(unnamed);
        self.releases.sort_unstable_by(|one, other| other.cmp(one));
        self.releases.push(0);
        let compilers = if self.names_a_compiler {
            COMPILERS.as_slice()
        } else {
            &[""]
        };
        let Self {
            systems,
            architectures,
            releases,
            others,
            ..
        } = &self;
        let sets = 1_usize
            .checked_shl(narrow(others.len()))
            .unwrap_or(usize::MAX);
        let builds = [
            systems.len(),
            architectures.len(),
            compilers.len(),
            releases.len(),
        ]
        .into_iter()
        .fold(sets, usize::saturating_mul);
        builds > MOST_BUILDS
            || (0..builds).any(|mut build| {
                let mut pick = |choices: usize| {
                    let picked = build % choices;
                    build /= choices;
                    picked
                };
                let found = (
                    systems.get(pick(systems.len())),
                    architectures.get(pick(architectures.len())),
                    compilers.get(pick(compilers.len())),
                    releases.get(pick(releases.len())),
                );
                let (Some(system), Some(architecture), Some(compiler), Some(&release)) = found
                else {
                    return false;
                };
                holds(&Build {
                    system,
                    architecture,
                    compiler,
                    release,
                    other_tags: others,
                    set: build,
                })
            })
    }
}

fn narrow(count: usize) -> u32 {
    u32::try_from(count).unwrap_or(u32::MAX)
}

/// Whether no build satisfies both `one` and `other`, each a kept constraint and the empty one none.
///
/// A build here is one go/build reads with one `GOOS`, one `GOARCH`, one compiler and one release, and any other tags:
/// `-tags` can set a tag of those too, and a file the command line names skips constraints, and neither counts.
pub(crate) fn built_apart(one: &str, other: &str) -> bool {
    let holds = |expression: &str, build: &Build<'_>| {
        evaluate(expression, |tag| build.sets(tag)) != Some(false)
    };
    !Named::of([one, other]).some_build(|build| holds(one, build) && holds(other, build))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_header_states_a_constraint_as_go_build_reads_it() {
        for (source, constraint) in [
            ("package p\n", None),
            ("//go:build windows\n\npackage p\n", Some("windows")),
            ("//go:build windows\npackage p\n", Some("windows")),
            ("  //go:build windows \r\n\npackage p\n", Some("windows")),
            ("\u{feff}//go:build windows\n\npackage p\n", Some("windows")),
            (
                "/* c */\n//go:build windows\n\npackage p\n",
                Some("windows"),
            ),
            (
                "// c\n\n//go:build (a || b) && !c\n\npackage p\n",
                Some("(a||b)&&!c"),
            ),
            ("package p\n\n//go:build windows\n", None),
            ("//go:buildx windows\n\npackage p\n", None),
            (
                "//go:buildx a\n// +build windows\n\npackage p\n",
                Some("windows"),
            ),
            ("/*\n//go:build windows\n*/\n\npackage p\n", None),
            ("//go:build\n\npackage p\n", None),
            ("//go:build a b\n\npackage p\n", None),
            ("//go:build !!a\n\npackage p\n", None),
            ("//go:build (a\n\npackage p\n", None),
            ("//go:build a\n//go:build b\n\npackage p\n", None),
            (
                "//go:build windows\n// +build linux\n\npackage p\n",
                Some("windows"),
            ),
        ] {
            assert_eq!(
                of_header(source.as_bytes()).as_deref(),
                constraint,
                "{source:?}"
            );
        }
    }

    #[test]
    fn a_plus_build_line_counts_only_above_a_blank_line_and_ands_with_the_others() {
        for (source, constraint) in [
            ("// +build windows\n\npackage p\n", Some("windows")),
            ("//+build windows\n\npackage p\n", Some("windows")),
            ("// +build windows\npackage p\n", None),
            ("/* c */\n// +build windows\n\npackage p\n", None),
            (
                "// doc\n\n// +build windows\n\n// doc\npackage p\n",
                Some("windows"),
            ),
            ("// +build a,!b c\n\npackage p\n", Some("a&&!b||c")),
            (
                "// +build a b\n// +build !c\n\npackage p\n",
                Some("(a||b)&&(!c)"),
            ),
            ("// +build\n\npackage p\n", Some("ignore")),
            (
                "// +build !!a ! b-c !d-e\n\npackage p\n",
                Some("ignore||ignore||ignore||!ignore"),
            ),
            ("// +buildx a\n\npackage p\n", None),
        ] {
            assert_eq!(
                of_header(source.as_bytes()).as_deref(),
                constraint,
                "{source:?}"
            );
        }
    }

    #[test]
    fn an_expression_is_read_as_go_build_constraint_reads_it() {
        let set = |tag: &str| tag == "a";
        for (expression, value) in [
            ("a", Some(true)),
            ("!a", Some(false)),
            ("b||a", Some(true)),
            ("b||a&&b", Some(false)),
            ("(b||a)&&!b", Some(true)),
            ("a b", None),
            ("a&b", None),
            ("!!a", None),
            ("a)", None),
            ("", None),
        ] {
            assert_eq!(evaluate(expression, set), value, "{expression}");
        }
        let too_large = vec!["a"; MOST_TERMS + 1].join("&&");
        assert_eq!(evaluate(&too_large, set), None);
    }

    #[test]
    fn two_constraints_are_built_apart_only_where_no_build_satisfies_both() {
        for (one, other, apart) in [
            ("", "", false),
            ("", "linux", false),
            ("linux", "windows", true),
            ("linux", "!linux", true),
            ("linux", "!windows", false),
            ("linux", "android", false),
            ("solaris", "illumos", false),
            ("darwin", "ios", false),
            ("android", "ios", true),
            ("linux&&!android", "android", true),
            ("unix", "windows", true),
            ("unix", "zos", true),
            ("unix", "linux", false),
            ("!unix", "linux", true),
            ("!unix", "!windows&&!plan9", false),
            ("!linux&&unix", "aix||freebsd", false),
            ("!linux&&!aix&&unix", "", false),
            ("amd64", "arm64", true),
            ("amd64", "linux", false),
            ("linux&&amd64", "(linux&&arm64)||darwin", true),
            ("linux&&amd64", "(linux&&arm64)||amd64", false),
            ("purego", "!purego", true),
            ("purego", "!cgo", false),
            ("a||b", "!a&&!b", true),
            ("a||b", "!a", false),
            ("go1.21", "!go1.20", true),
            ("go1.20", "!go1.21", false),
            ("!go1.20", "!go1.21", false),
            ("go1.18&&!go1.20", "go1.21", true),
            ("gc", "gccgo", true),
            ("!gc", "!gccgo", true),
            ("!gc", "gccgo", false),
            ("linux&&!linux", "", true),
        ] {
            assert_eq!(built_apart(one, other), apart, "{one} and {other}");
            assert_eq!(built_apart(other, one), apart, "{other} and {one}");
        }
    }

    #[test]
    fn a_pair_past_the_bound_reads_as_built_together() {
        let apart_with_tags = |count: usize| {
            let tags: Vec<String> = (0..count).map(|tag| format!("t{tag}")).collect();
            let every = tags.join("&&");
            built_apart(&every, &format!("!({every})"))
        };

        assert!(apart_with_tags(13), "two systems times 2^13 sets the bound");
        assert!(!apart_with_tags(14));
    }
}
