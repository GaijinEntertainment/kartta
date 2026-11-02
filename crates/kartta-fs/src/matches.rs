use std::io;
use std::marker::PhantomData;

use crate::index::Index;

/// What [`Index::grep`](crate::Index::grep) found: the matched files in index order, and where every other candidate
/// went.
///
/// An agent cannot see what it was not given, so the answer accounts for each file the search could have read. A
/// **candidate** is a regular file the filter selected and the exclusion kept, and every candidate lands in exactly one
/// of six places:
///
/// | Count | The file was |
/// | --- | --- |
/// | [`searched`](Matches::searched) | read; its matches are in this answer up to the limit, and counted past it when asked |
/// | [`binary`](Matches::binary) | skipped as binary, by its extension or by a NUL in its first 64 KiB |
/// | [`oversized`](Matches::oversized) | skipped for being larger than [`GrepOptions::max_file_bytes`](crate::GrepOptions::max_file_bytes) |
/// | [`ruled_out`](Matches::ruled_out) | skipped unread, because the content index shows it holds no match |
/// | [`failures`](Matches::failures) | not readable, and listed with the reason |
/// | [`unsearched`](Matches::unsearched) | not covered, because a match past the limit or a cancellation ended the search first |
///
/// A file that is both binary and too large counts as oversized, unless its extension alone makes it binary: the name
/// is judged first, then the size, then the content, whatever an earlier read of the file already learned.
///
/// The answer is complete exactly when nothing is [`truncated`](Matches::truncated), the search was not
/// [`cancelled`](Matches::cancelled) and nothing [`failed`](Matches::failures) — a file that would not read holds
/// matches the answer cannot know of.
#[derive(Debug)]
pub struct Matches<'a> {
    pub(crate) files: Vec<MatchedFile<'a>>,
    pub(crate) lines: usize,
    pub(crate) truncated: bool,
    pub(crate) total: Option<Total>,
    pub(crate) cancelled: bool,
    pub(crate) tally: Tally,
    pub(crate) failures: Vec<Failure>,
}

impl<'a> Matches<'a> {
    /// The files holding a match, in index order, each with its lines in file order.
    #[must_use]
    pub fn files(&self) -> &[MatchedFile<'a>] {
        &self.files
    }

    /// How many matching lines the answer holds, context lines not counted.
    #[must_use]
    pub const fn lines(&self) -> usize {
        self.lines
    }

    /// Whether the limit turned a matching line away, so more matches exist than the answer shows.
    ///
    /// Set by a match actually seen past the limit and never inferred from a count, so an answer holding exactly the
    /// limit can still be whole. Exact unless the search was [`cancelled`](Matches::cancelled) or a file
    /// [`failed`](Matches::failures): once the limit fills, the search reads on until one more match proves the cut, so
    /// `false` means no candidate it read holds another match.
    #[must_use]
    pub const fn truncated(&self) -> bool {
        self.truncated
    }

    /// How many matching lines the searched candidates hold, the ones past the limit included — `Some` only when
    /// [`GrepOptions::count_all`](crate::GrepOptions::count_all) asked for the count.
    ///
    /// The count covers what was read. It is the whole tree's count exactly when nothing went
    /// [`unsearched`](Matches::unsearched) and nothing [`failed`](Matches::failures); after a cancellation or an
    /// unreadable file it is a lower bound. Binary and oversized files are never read, so they hold no counted match.
    #[must_use]
    pub fn total_lines(&self) -> Option<usize> {
        self.total.map(|total| total.lines)
    }

    /// How many searched candidates hold a matching line, the ones past the limit included — `Some` only when
    /// [`GrepOptions::count_all`](crate::GrepOptions::count_all) asked for the count.
    ///
    /// A lower bound on the same terms as [`Matches::total_lines`].
    #[must_use]
    pub fn total_files(&self) -> Option<usize> {
        self.total.map(|total| total.files)
    }

    /// Whether a cancellation stopped the search before it covered every candidate.
    ///
    /// Set only when the cancellation actually turned a file away, so a search that finished before noticing it is
    /// complete and says so.
    #[must_use]
    pub const fn cancelled(&self) -> bool {
        self.cancelled
    }

    /// How many regular files the filter selected and the exclusion kept — every file this search could have read.
    #[must_use]
    pub const fn candidates(&self) -> usize {
        self.tally.candidates
    }

    /// How many regular files the filter selected that [`GrepOptions::exclude`](crate::GrepOptions::exclude) removed,
    /// whether matched themselves or lying beneath a matched directory.
    #[must_use]
    pub const fn excluded(&self) -> usize {
        self.tally.excluded
    }

    /// How many symlinks the filter selected, excluded or not, none of which is searched: a symlink is reported, never
    /// read through.
    #[must_use]
    pub const fn symlinks(&self) -> usize {
        self.tally.symlinks
    }

    /// How many candidates were read and are covered by this answer: their matches are in it up to the limit, and
    /// counted past it when [`GrepOptions::count_all`](crate::GrepOptions::count_all) asked.
    #[must_use]
    pub const fn searched(&self) -> usize {
        self.tally.searched
    }

    /// How many candidates were skipped as binary.
    #[must_use]
    pub const fn binary(&self) -> usize {
        self.tally.binary
    }

    /// How many candidates were skipped for exceeding
    /// [`GrepOptions::max_file_bytes`](crate::GrepOptions::max_file_bytes).
    #[must_use]
    pub const fn oversized(&self) -> usize {
        self.tally.oversized
    }

    /// How many candidates the content index ruled out, so the search never read them: the trigrams a match needs are
    /// missing from the content the build read. Zero for an index built without
    /// [`BuildOptions::content_index`](crate::BuildOptions::content_index), and for patterns that need no trigram, such
    /// as `\w+`, which read every candidate.
    ///
    /// The index holds each file's content as the build read it, so a file changed since then is ruled in or out by
    /// that content rather than by what it holds now.
    #[must_use]
    pub const fn ruled_out(&self) -> usize {
        self.tally.ruled_out
    }

    /// How many candidates this answer does not cover, because a match past the limit proved the cut or a cancellation
    /// landed first.
    ///
    /// A file read after the cut was proven counts here too: its lines are not in the answer.
    #[must_use]
    pub const fn unsearched(&self) -> usize {
        self.tally.unsearched
    }

    /// The candidates that could not be read, in index order.
    #[must_use]
    pub fn failures(&self) -> &[Failure] {
        &self.failures
    }
}

/// One file of [`Matches`] and its lines.
#[derive(Debug, Clone)]
pub struct MatchedFile<'a> {
    pub(crate) position: usize,
    pub(crate) lines: Vec<Line>,
    pub(crate) index: PhantomData<&'a Index>,
}

impl MatchedFile<'_> {
    /// Where the file sits in [`Index::entries`], for reading its path through [`Index::paths`]. The files of one
    /// answer rise in position, so reading their paths in order costs a few bytes each.
    #[must_use]
    pub const fn position(&self) -> usize {
        self.position
    }

    /// The matching lines and the context around them, in file order.
    #[must_use]
    pub fn lines(&self) -> &[Line] {
        &self.lines
    }
}

/// One reported line: a match, or a context line around one.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Line {
    pub(crate) number: u64,
    pub(crate) kind: LineKind,
    pub(crate) text: Box<str>,
    pub(crate) cut: Option<Cut>,
}

impl Line {
    /// The line's number in its file, counting from 1.
    #[must_use]
    pub const fn number(&self) -> u64 {
        self.number
    }

    /// Whether the line matched or rides along as context.
    #[must_use]
    pub const fn kind(&self) -> LineKind {
        self.kind
    }

    /// The line's text without its line ending.
    ///
    /// Bytes that are not valid UTF-8 read as U+FFFD, so the text is faithful for reading and not for byte-exact reuse.
    /// A line longer than [`GrepOptions::max_line_bytes`](crate::GrepOptions::max_line_bytes) is cut, and
    /// [`Line::cut`] says by how much.
    #[must_use]
    pub fn text(&self) -> &str {
        &self.text
    }

    /// How much of the line [`Line::text`] leaves out, or `None` when it holds the whole line.
    #[must_use]
    pub const fn cut(&self) -> Option<Cut> {
        self.cut
    }
}

/// How many bytes of a long line its reported text leaves out on each side.
///
/// A matching line is cut around its first match, so the part that matched is what survives, and a context line keeps
/// its start. Either side may be zero. Exhaustive on purpose: a line has two sides.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Cut {
    /// The bytes left out before the reported text.
    pub before: usize,
    /// The bytes left out after it.
    pub after: usize,
}

/// Whether a [`Line`] matched or rides along as context.
///
/// Exhaustive on purpose: a reported line is one or the other, and the wire and the MCP answer map each to a word of
/// their own.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum LineKind {
    /// The line matched one of the patterns.
    Match,
    /// The line sits within [`GrepOptions::context`](crate::GrepOptions::context) lines of a match.
    Context,
}

/// A candidate the search could not read.
#[derive(Debug)]
pub struct Failure {
    pub(crate) position: usize,
    pub(crate) error: io::Error,
}

impl Failure {
    /// Where the file sits in [`Index::entries`](crate::Index::entries).
    #[must_use]
    pub const fn position(&self) -> usize {
        self.position
    }

    /// Why it could not be read.
    #[must_use]
    pub const fn error(&self) -> &io::Error {
        &self.error
    }
}

#[derive(Debug, Clone, Copy, Default)]
pub(crate) struct Total {
    pub(crate) lines: usize,
    pub(crate) files: usize,
}

#[derive(Debug, Clone, Copy, Default)]
pub(crate) struct Tally {
    pub(crate) candidates: usize,
    pub(crate) excluded: usize,
    pub(crate) symlinks: usize,
    pub(crate) searched: usize,
    pub(crate) binary: usize,
    pub(crate) oversized: usize,
    pub(crate) ruled_out: usize,
    pub(crate) unsearched: usize,
}

impl Tally {
    pub(crate) const fn add(&mut self, other: &Self) {
        self.candidates += other.candidates;
        self.excluded += other.excluded;
        self.symlinks += other.symlinks;
        self.searched += other.searched;
        self.binary += other.binary;
        self.oversized += other.oversized;
        self.ruled_out += other.ruled_out;
        self.unsearched += other.unsearched;
    }
}
