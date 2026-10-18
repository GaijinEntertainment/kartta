use std::fmt::Write;

use kartta_fs::{EntryKind, Failure, Line, LineKind, Listing, Matches, Paths};

use crate::llmxml::{Element, List, lines};

/// How many unreadable files a `grep` answer lists before it counts the rest.
const LISTED_FAILURES: usize = 5;

/// The `find_files` answer: the paths one per line, best first. A ranked answer needs no mark of a cap, since its order
/// says the paths it left out rank lower.
pub(crate) fn paths(pattern: &str, paths: &[String]) -> String {
    if paths.is_empty() {
        return format!("no files match {pattern:?}");
    }
    lines(paths)
}

/// How much of what a glob matched a `glob_files` answer lists.
#[derive(Debug, Clone, Copy)]
pub(crate) enum Globbed {
    /// Every path that matched.
    Whole,
    /// The first paths in path order, and how many matched in all when the search counted them.
    Cut { matched: Option<usize> },
}

/// The `glob_files` answer: the paths one per line, in path order, and a last line saying so when the cap cut them,
/// since paths in path order look the same whether or not more follow, and a last line counting the `excluded`
/// matches the excludes removed.
pub(crate) fn globbed(globs: &str, listed: List, globbed: Globbed, excluded: usize) -> String {
    let shown = listed.len();
    if shown == 0 {
        return if excluded == 0 {
            format!("no files match {globs}")
        } else {
            format!("no files match {globs} outside the excludes (excludes removed: {excluded})")
        };
    }
    let mut answer = listed.into_text();
    let _ = match globbed {
        Globbed::Whole => Ok(()),
        Globbed::Cut {
            matched: Some(matched),
        } => write!(
            answer,
            "\n(truncated: {shown} of {matched} matching paths, in path order)"
        ),
        Globbed::Cut { matched: None } => write!(
            answer,
            "\n(truncated: the first {shown} matching paths in path order; more match)"
        ),
    };
    if excluded > 0 {
        let _ = write!(answer, "\n(excludes removed: {excluded})");
    }
    answer
}

/// The `grep` answer: a `<file>` block per matched file, in path order, with `N:text` for a match and `N-text` for
/// context, `--` between regions that do not touch, the unreadable files, and what the search did not cover.
pub(crate) fn grep(patterns: &[String], matches: &Matches<'_>, paths: &mut Paths<'_>) -> String {
    let element = Element::new("grep")
        .attr("patterns", patterns.join(", "))
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
    if matches.candidates() == 0 {
        return element.inline(if matches.symlinks() == 0 {
            "the include/exclude globs selected no files - glob_files shows what they match"
        } else {
            "the include/exclude globs selected symlinks alone, which are never read through - include their targets"
        });
    }
    if matches.files().is_empty() && matches.failures().is_empty() {
        return element.inline(&no_match_hint(patterns, matches));
    }

    let failures = failures_block(matches.failures(), paths);
    let blocks = matches
        .files()
        .iter()
        .map(|file| file_block(paths.get(file.position()), file.lines()));
    element.children(blocks.chain(failures))
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
    if failures.len() > LISTED_FAILURES {
        listed.push(&format!("(+{} more)", failures.len() - LISTED_FAILURES));
    }
    Some(
        Element::new("failures")
            .attr("count", failures.len())
            .wrap(&listed.into_text()),
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

#[cfg(test)]
mod tests {
    use super::{Globbed, globbed};
    use crate::llmxml::List;

    #[test]
    fn a_glob_answer_keeps_a_path_with_a_line_break_on_its_line() {
        let mut paths = List::default();
        paths.push("a.rs");
        paths.push("b\nc.rs");

        assert_eq!(
            globbed("**", paths, Globbed::Cut { matched: Some(3) }, 0),
            "a.rs\n\"b\\nc.rs\"\n(truncated: 2 of 3 matching paths, in path order)"
        );
    }
}
