use std::fs::File;
use std::io::{self, BufRead, BufReader};
use std::num::NonZeroUsize;
use std::path::Path;

use kartta_graph::Definition;
use kartta_lang::DocComments;

/// How much of a definition's text [`Engine::sources`](crate::Engine::sources) answers.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub struct SourceOptions {
    /// How many lines to answer at most, those of the doc comment included. A longer definition stops at the cap, and
    /// a doc comment too long to leave room loses its head, so the text always holds the definition's first line.
    /// `None` answers every line.
    pub max_lines: Option<NonZeroUsize>,
    /// How many bytes of a line to answer. A longer line keeps its start, and [`Source::cut_lines`] says how much it
    /// lost. A cut never splits a character, and never leaves a line empty: a limit narrower than the line's first
    /// character keeps that character whole. `None` answers lines whole.
    pub max_line_bytes: Option<NonZeroUsize>,
}

impl SourceOptions {
    /// Answers at most `max` lines of each definition. `0` lifts the bound, as `None` does.
    #[must_use]
    pub const fn max_lines(mut self, max: usize) -> Self {
        self.max_lines = NonZeroUsize::new(max);
        self
    }

    /// Answers at most `max` bytes of each line. `0` lifts the bound, as `None` does.
    #[must_use]
    pub const fn max_line_bytes(mut self, max: usize) -> Self {
        self.max_line_bytes = NonZeroUsize::new(max);
        self
    }

    pub(crate) fn last_line_answered(self, definition: &Definition<'_>) -> u32 {
        let end = definition.end_line.max(definition.line);
        self.max_lines.map_or(end, |max| {
            let reach = u32::try_from(max.get() - 1).unwrap_or(u32::MAX);
            end.min(definition.line.saturating_add(reach))
        })
    }
}

/// A definition's text as its file holds it when asked, from the doc comment directly above it.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub struct Source {
    /// The first line of the doc comment directly above the definition, as its language writes one, with the
    /// attributes, decorators or template headers the declaration carries there, or the definition's own line when
    /// there is none.
    pub doc_line: u32,
    /// The first line `text` holds: `doc_line`, or a later one when the line cap left out the head of the doc comment.
    pub first_line: u32,
    /// How many lines `text` holds from `first_line`. The definition's lines past them were left out by the line cap
    /// or by a file that now ends sooner; a file that ends before the definition starts leaves none.
    pub lines: u32,
    /// The lines, each without its line ending, joined by `\n`. A byte sequence that is not UTF-8 reads as U+FFFD.
    pub text: String,
    /// The lines of `text` the byte cap cut, in line order.
    pub cut_lines: Vec<CutLine>,
    /// Whether the file's size now differs from the size the build read, so the lines the graph holds for the
    /// definition may no longer be its own. An edit that keeps the size goes unseen.
    pub resized: bool,
}

/// A line of a [`Source`] cut at [`SourceOptions::max_line_bytes`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub struct CutLine {
    /// The one-based line.
    pub line: u32,
    /// How many bytes of it the cut left out.
    pub bytes: usize,
}

pub(crate) struct FileLines {
    bytes: Vec<u8>,
    starts: Vec<usize>,
    resized: bool,
}

impl FileLines {
    pub(crate) fn read(path: &Path, size_read: Option<u64>, through: u32) -> io::Result<Self> {
        let file = File::open(path)?;
        let resized = size_read != Some(file.metadata()?.len());
        let through = usize::try_from(through).unwrap_or(usize::MAX);
        let mut reader = BufReader::new(file);
        let mut lines = Self {
            bytes: Vec::new(),
            starts: Vec::new(),
            resized,
        };
        while lines.starts.len() < through {
            let start = lines.bytes.len();
            if reader.read_until(b'\n', &mut lines.bytes)? == 0 {
                break;
            }
            lines.starts.push(start);
        }
        Ok(lines)
    }

    pub(crate) fn reading(&self, doc: DocComments, options: SourceOptions) -> Reading<'_> {
        let ends = self
            .starts
            .iter()
            .skip(1)
            .copied()
            .chain([self.bytes.len()]);
        Reading {
            lines: self
                .starts
                .iter()
                .zip(ends)
                .map(|(&start, end)| without_line_ending(&self.bytes[start..end]))
                .collect(),
            resized: self.resized,
            doc,
            options,
        }
    }
}

pub(crate) struct Reading<'f> {
    lines: Vec<&'f [u8]>,
    resized: bool,
    doc: DocComments,
    options: SourceOptions,
}

impl Reading<'_> {
    pub(crate) fn source(&self, definition: &Definition<'_>) -> Source {
        let lines = &self.lines;
        let mut source = Source {
            doc_line: definition.line,
            first_line: definition.line,
            lines: 0,
            text: String::new(),
            cut_lines: Vec::new(),
            resized: self.resized,
        };
        let line = to_usize(definition.line);
        if line == 0 || line > lines.len() {
            return source;
        }
        source.doc_line = self.doc.first_line(lines, definition.line);
        let mut first = to_usize(source.doc_line);
        let mut last = to_usize(definition.end_line).clamp(line, lines.len());
        if let Some(max) = self.options.max_lines {
            first = first.max((line + 1).saturating_sub(max.get()));
            last = last.min(first.saturating_add(max.get() - 1));
        }
        source.first_line = to_u32(first);
        source.lines = to_u32(last + 1 - first);
        for (at, text) in lines[first - 1..last].iter().enumerate() {
            if at > 0 {
                source.text.push('\n');
            }
            let cut = push_line(&mut source.text, text, self.options.max_line_bytes);
            if cut > 0 {
                source.cut_lines.push(CutLine {
                    line: to_u32(first + at),
                    bytes: cut,
                });
            }
        }
        source
    }
}

fn push_line(text: &mut String, line: &[u8], max_bytes: Option<NonZeroUsize>) -> usize {
    let line = String::from_utf8_lossy(line);
    let Some(max) = max_bytes
        .map(NonZeroUsize::get)
        .filter(|&max| max < line.len())
    else {
        text.push_str(&line);
        return 0;
    };
    let mut kept = line.floor_char_boundary(max);
    if kept == 0 {
        kept = line.ceil_char_boundary(1);
    }
    text.push_str(&line[..kept]);
    line.len() - kept
}

fn without_line_ending(line: &[u8]) -> &[u8] {
    let line = line.strip_suffix(b"\n").unwrap_or(line);
    line.strip_suffix(b"\r").unwrap_or(line)
}

fn to_usize(line: u32) -> usize {
    usize::try_from(line).unwrap_or(usize::MAX)
}

fn to_u32(line: usize) -> u32 {
    u32::try_from(line).unwrap_or(u32::MAX)
}
