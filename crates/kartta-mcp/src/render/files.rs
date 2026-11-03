use std::fmt::Write;

use kartta_fs::{Capped, EntryKind, Failure, Index, Line, LineKind, Listing, Matches, Paths};

use crate::llmxml::{Element, List};
use crate::paths::GLOB_CHARACTERS;

/// How many unreadable files a `grep` answer lists before it counts the rest.
const LISTED_FAILURES: usize = 5;

/// The index a file tool searched and the globs that selected its files, which an answer reads for a glob that names a
/// directory.
pub(crate) struct Selection<'a> {
    pub(crate) index: &'a Index,
    pub(crate) globs: &'a [String],
}

impl Selection<'_> {
    /// The fix for each glob that holds no glob character nor `\`, and so names one path from the root, a trailing `/`
    /// dropped, where that path is a directory the index holds, or one name the root does not hold that directories
    /// below it carry: `<dir>/**` for one directory, `**/<name>/**` and their count for more. Such a glob selects
    /// nothing, since a glob selects files alone. `None` when no glob names a directory.
    fn directories(&self) -> Option<String> {
        let paths: Vec<Option<&str>> = self
            .globs
            .iter()
            .map(|glob| {
                Some(glob.strip_suffix('/').unwrap_or(glob)).filter(|path| {
                    !path.is_empty() && !path.contains(GLOB_CHARACTERS) && !path.contains('\\')
                })
            })
            .collect();
        let fixes: Vec<String> = self
            .globs
            .iter()
            .zip(&paths)
            .zip(self.below(&paths))
            .filter_map(|((glob, &path), below)| {
                let path = path?;
                let fix = match below {
                    (0, _) => {
                        self.index
                            .entry(path)
                            .filter(|entry| entry.kind() == EntryKind::Dir)?;
                        format!("{path}/**")
                    }
                    (1, Some(dir)) => format!("{dir}/**"),
                    (count, _) => {
                        return Some(format!(
                            "{:?} ({count} directories are named {path:?}) in place of {glob:?}",
                            format!("**/{path}/**")
                        ));
                    }
                };
                Some(format!("{fix:?} in place of {glob:?}"))
            })
            .collect();
        (!fixes.is_empty()).then(|| {
            format!(
                "a glob selects files by their path from the root, so one that names a directory selects nothing: \
                 pass {}",
                fixes.join(", ")
            )
        })
    }

    /// For each path of one name the root does not hold, how many directories below the root carry that name and the
    /// first of them; `(0, None)` for every other path. One pass over the index's directories, run only when such a
    /// path is asked.
    fn below(&self, paths: &[Option<&str>]) -> Vec<(usize, Option<String>)> {
        let mut below = vec![(0, None); paths.len()];
        let unheld: Vec<usize> = (0..paths.len())
            .filter(|&at| {
                paths[at]
                    .is_some_and(|path| !path.contains('/') && self.index.entry(path).is_none())
            })
            .collect();
        if unheld.is_empty() {
            return below;
        }
        let mut every = self.index.paths();
        for position in (0..self.index.len()).filter(|&at| self.index.kind_at(at) == EntryKind::Dir)
        {
            let path = every.get(position);
            let name = path.rsplit_once('/').map_or(path, |(_, name)| name);
            for &at in unheld.iter().filter(|&&at| paths[at] == Some(name)) {
                let (count, first) = &mut below[at];
                *count += 1;
                first.get_or_insert_with(|| path.to_owned());
            }
        }
        below
    }
}

/// `" - "` and the fix of [`Selection::directories`], which an answer that lists nothing ends with; empty when no
/// glob names a directory.
fn dashed(fix: Option<String>) -> String {
    fix.map_or_else(String::new, |fix| format!(" - {fix}"))
}

/// The `find_files` answer: the paths one per line, best first, and a last line counting the `matched` paths when the
/// cap cut them. The order says the paths left out rank lower; the count says how many, so an agent knows whether to
/// narrow the pattern.
pub(crate) fn paths(
    pattern: &str,
    paths: List,
    matched: Option<usize>,
    includes: &Selection<'_>,
) -> String {
    let shown = paths.len();
    if shown == 0 {
        return format!(
            "no files match {pattern:?}{}",
            dashed(includes.directories())
        );
    }
    let mut answer = paths.into_text();
    if let Some(matched) = matched.filter(|&matched| matched > shown) {
        let _ = write!(
            answer,
            "\n({shown} of {matched} matching paths, best first)"
        );
    }
    if let Some(directories) = includes.directories() {
        let _ = write!(answer, "\n({directories})");
    }
    answer
}

/// The `glob_files` answer: the paths one per line, in path order, and a last line saying so when the cap cut them,
/// since paths in path order look the same whether or not more follow, and a last line counting the matches the
/// excludes removed. An answer that lists nothing echoes each glob quoted and escaped, `, ` between them: a glob can
/// hold a comma, and a line break too, since a name can.
pub(crate) fn globbed(globs: &Selection<'_>, found: &Capped<'_>) -> String {
    let excluded = found.excluded();
    let shown = found.items().len();
    if shown == 0 {
        let mut joined = String::new();
        for glob in globs.globs {
            if !joined.is_empty() {
                joined.push_str(", ");
            }
            let _ = write!(joined, "{glob:?}");
        }
        let directories = dashed(globs.directories());
        return if excluded == 0 {
            format!("no files match {joined}{directories}")
        } else {
            format!(
                "no files match {joined} outside the excludes (excludes removed: {excluded}){directories}"
            )
        };
    }
    let mut paths = globs.index.paths();
    let mut listed = List::default();
    for &position in found.items() {
        listed.push(paths.get(position));
    }
    let mut answer = listed.into_text();
    if found.truncated() {
        let _ = write!(
            answer,
            "\n(truncated: the first {shown} matching paths in path order; more match)"
        );
    }
    if excluded > 0 {
        let _ = write!(answer, "\n(excludes removed: {excluded})");
    }
    if let Some(directories) = globs.directories() {
        let _ = write!(answer, "\n({directories})");
    }
    answer
}

/// The `grep` answer: a `<file>` block per matched file, in path order, with `N:text` for a match and `N-text` for
/// context, `--` between regions that do not touch, the unreadable files, and what the search did not cover. The
/// patterns echo one a line, which the attribute writes as `\n`: a pattern can hold a comma, and the search refuses
/// one that holds a line break.
pub(crate) fn grep(patterns: &[String], matches: &Matches<'_>, includes: &Selection<'_>) -> String {
    let element = Element::new("grep")
        .attr("patterns", patterns.join("\n"))
        .attr("candidates", matches.candidates())
        .attr("files", matches.files().len())
        .attr("matches", matches.lines())
        .flag("truncated", matches.truncated())
        .count("unsearched", matches.unsearched())
        .count("excluded", matches.excluded())
        .count("binary", matches.binary())
        .count("oversized", matches.oversized())
        .count("ruled_out", matches.ruled_out())
        .count("symlinks", matches.symlinks());
    let directories = includes.directories();
    if matches.candidates() == 0 {
        let directories = dashed(directories);
        if matches.symlinks() > 0 {
            return element.inline(&format!(
                "the include/exclude globs selected symlinks alone, which are never read through - include their \
                 targets{directories}"
            ));
        }
        return element.inline(&if directories.is_empty() {
            "the include/exclude globs selected no files - glob_files shows what they match"
                .to_owned()
        } else {
            format!("the include/exclude globs selected no files{directories}")
        });
    }
    if matches.files().is_empty() && matches.failures().is_empty() {
        return element.inline(&(no_match_hint(patterns, matches) + &dashed(directories)));
    }

    let mut paths = includes.index.paths();
    let failures = failures_block(matches.failures(), &mut paths);
    let blocks = matches
        .files()
        .iter()
        .map(|file| file_block(paths.get(file.position()), file.lines()));
    let note = directories.map(|fix| Element::new("note").inline(&fix));
    element.children(blocks.chain(failures).chain(note))
}

/// The `<failures>` block naming the files that would not read, or `None` when every file read.
fn failures_block(failures: &[Failure], paths: &mut Paths<'_>) -> Option<String> {
    if failures.is_empty() {
        return None;
    }
    let mut listed = List::default();
    for failure in failures.iter().take(LISTED_FAILURES) {
        listed.push(&format!(
            "{}: {}",
            paths.get(failure.position()),
            failure.error()
        ));
    }
    let mut text = listed.into_text();
    if failures.len() > LISTED_FAILURES {
        let _ = write!(text, "\n(+{} more)", failures.len() - LISTED_FAILURES);
    }
    Some(
        Element::new("failures")
            .attr("count", failures.len())
            .wrap(&text),
    )
}

fn file_block(path: &str, lines: &[Line]) -> String {
    let mut body = String::new();
    let mut previous: Option<u64> = None;
    let mut matched = 0;
    for line in lines {
        if let Some(previous) = previous {
            body.push_str(if line.number() > previous + 1 {
                "\n--\n"
            } else {
                "\n"
            });
        }
        matched += usize::from(line.kind() == LineKind::Match);
        push_grep_line(&mut body, line);
        previous = Some(line.number());
    }
    Element::new("file")
        .attr("path", path)
        .attr("matches", matched)
        .wrap(&body)
}

/// Appends one line: `12:text` for a match, `11-text` for context, and the bytes a long line lost on each side.
fn push_grep_line(body: &mut String, line: &Line) {
    let separator = match line.kind() {
        LineKind::Match => ':',
        LineKind::Context => '-',
    };
    let _ = write!(body, "{}{separator}", line.number());
    let (before, after) = line.cut().map_or((0, 0), |cut| (cut.before, cut.after));
    if before > 0 {
        let _ = write!(body, "[... omitted {before} bytes] ");
    }
    body.push_str(line.text());
    if after > 0 {
        let _ = write!(body, " [... omitted {after} bytes]");
    }
}

/// The answer to a search that matched no line: whether files went unread, and the likely mistake when a pattern
/// assumes `.*` crosses lines.
fn no_match_hint(patterns: &[String], matches: &Matches<'_>) -> String {
    let mut hint = if matches.binary() + matches.oversized() == 0 {
        "no line matches".to_owned()
    } else {
        "no line matches in the files read - binary and oversized files are skipped unread"
            .to_owned()
    };
    if patterns
        .iter()
        .any(|pattern| pattern.contains(".*") || pattern.contains(".+"))
    {
        hint.push_str(
            " - matching is per-line: \".*\"/\".+\" cannot cross line breaks; search the most distinctive part alone",
        );
    }
    hint
}

/// The `list_dir` answer: an `<entry/>` per listed entry, depth-first in name order, a directory before its children,
/// and the counts of what the listing left out.
pub(crate) fn dir(path: &str, listing: &Listing<'_>) -> String {
    let shown = if path.is_empty() { "." } else { path };
    let element = Element::new("dir")
        .attr("path", shown)
        .attr("entries", listing.len())
        .optional(
            "total",
            (listing.total() != listing.len()).then(|| listing.total()),
        )
        .count("excluded", listing.excluded())
        .flag("truncated", listing.truncated());
    if listing.is_empty() {
        return element.inline(if listing.excluded() == 0 {
            "directory is empty"
        } else {
            "no entry shown - skip_dotfiles excluded every entry"
        });
    }
    element.children(listing.items().iter().map(|listed| {
        let entry = listed.entry();
        let mut record = Element::new("entry")
            .attr("path", entry.path())
            .attr("kind", kind(entry.kind()));
        if entry.kind() == EntryKind::File {
            record = match listed.size() {
                Some(size) => record.attr("size", size),
                None => record.attr("size", "unknown"),
            };
        }
        record
            .optional("target", entry.target())
            .optional("unlisted", listed.unlisted())
            .close()
    }))
}

const fn kind(kind: EntryKind) -> &'static str {
    match kind {
        EntryKind::Dir => "dir",
        EntryKind::File => "file",
        EntryKind::Symlink => "symlink",
        EntryKind::Other => "other",
    }
}

// constraint: Windows refuses a line break in a file name
#[cfg(all(test, unix))]
mod tests {
    use kartta_fs::{BuildOptions, Filter, GlobOptions, Index};

    use super::{Selection, globbed};

    #[test]
    fn a_glob_answer_keeps_a_path_with_a_line_break_on_its_line() {
        let dir = tempfile::tempdir().expect("create temp dir");
        for name in ["a.rs", "b\nc.rs", "d.rs"] {
            std::fs::write(dir.path().join(name), "").expect("write file");
        }
        let index = Index::build(dir.path(), &BuildOptions::default())
            .expect("build")
            .index;
        let globs = ["**".to_owned()];
        let every = Filter::new(&globs).expect("compile");
        let found = index.glob(&every, &GlobOptions::default().limit(2));
        let selection = Selection {
            index: &index,
            globs: &globs,
        };

        assert_eq!(
            globbed(&selection, &found),
            "a.rs\n\"b\\nc.rs\"\n(truncated: the first 2 matching paths in path order; more match)"
        );
    }
}
