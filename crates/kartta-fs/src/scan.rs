use std::io::{self, Read};
use std::num::NonZeroUsize;
use std::path::PathBuf;

use grep_matcher::{LineTerminator, Matcher};
use grep_regex::RegexMatcher;
use grep_searcher::{BinaryDetection, Searcher, SearcherBuilder, Sink, SinkContext, SinkMatch};
use kartta_walk::CancelToken;

use crate::grep::GrepOptions;
use crate::index::{Index, Paths};
use crate::inspection::{PROBE, read_prefix};
use crate::matches::{Cut, Line, LineKind};
use crate::open::{Symlinks, open_regular};

const LINE_CEILING: usize = 256 * 1024 * 1024;

#[derive(Debug)]
pub(crate) enum Scanned {
    Searched(Found),
    Binary,
    Oversized,
    Failed(io::Error),
    Unsearched,
}

#[derive(Debug)]
pub(crate) struct Found {
    pub(crate) lines: Vec<Line>,
    pub(crate) kept: usize,
    pub(crate) past: usize,
}

pub(crate) struct Scanner<'a> {
    index: &'a Index,
    matcher: &'a RegexMatcher,
    options: &'a GrepOptions<'a>,
    searcher: Searcher,
    paths: Paths<'a>,
    native_path: PathBuf,
    head: Vec<u8>,
}

impl<'a> Scanner<'a> {
    pub(crate) fn new(
        index: &'a Index,
        matcher: &'a RegexMatcher,
        options: &'a GrepOptions<'a>,
    ) -> Self {
        let searcher = SearcherBuilder::new()
            .line_terminator(LineTerminator::crlf())
            .line_number(true)
            .before_context(options.context)
            .after_context(options.context)
            .binary_detection(BinaryDetection::none())
            .bom_sniffing(false)
            .heap_limit(Some(LINE_CEILING))
            .build();

        Self {
            index,
            matcher,
            options,
            searcher,
            paths: index.paths(),
            native_path: PathBuf::new(),
            head: Vec::new(),
        }
    }

    pub(crate) fn scan(&mut self, at: usize, budget: usize) -> Scanned {
        if self.options.cancel.is_some_and(CancelToken::is_cancelled) {
            return Scanned::Unsearched;
        }

        self.native_path.as_mut_os_string().clear();
        self.native_path.push(self.index.root());
        self.native_path.push(self.paths.get(at));
        let (mut file, size) = match open_regular(&self.native_path, Symlinks::Refuse) {
            Ok((file, metadata)) => (file, metadata.len()),
            Err(error) => return Scanned::Failed(error),
        };
        let inspection = self.index.inspection();
        inspection.record_size(at, size);
        let size = inspection.known_size(at).unwrap_or(size);

        if self
            .options
            .max_file_bytes
            .is_some_and(|max| size > max.get())
        {
            return Scanned::Oversized;
        }

        let probed = if inspection.known_binary(at) == Some(false) {
            0
        } else {
            self.head.resize(PROBE, 0);
            let filled = match read_prefix(&mut file, &mut self.head) {
                Ok(filled) => filled,
                Err(error) => return Scanned::Failed(error),
            };
            let binary = self.head[..filled].contains(&0);
            inspection.record_binary(at, binary);
            if inspection.known_binary(at).unwrap_or(binary) {
                return Scanned::Binary;
            }
            filled
        };

        let mut collector = Collector::new(self.matcher, self.options, budget);
        let source = (&self.head[..probed]).chain(file);
        match self
            .searcher
            .search_reader(self.matcher, source, &mut collector)
        {
            Ok(()) => Scanned::Searched(collector.into_found()),
            Err(error) => Scanned::Failed(error),
        }
    }
}

struct Collector<'a> {
    matcher: &'a RegexMatcher,
    budget: usize,
    context: u64,
    max_line_bytes: Option<NonZeroUsize>,
    counting: bool,
    lines: Vec<Line>,
    kept: usize,
    past: usize,
    last_kept: u64,
}

impl<'a> Collector<'a> {
    const fn new(matcher: &'a RegexMatcher, options: &GrepOptions<'_>, budget: usize) -> Self {
        Self {
            matcher,
            budget,
            context: options.context as u64,
            max_line_bytes: options.max_line_bytes,
            counting: options.count_all,
            lines: Vec::new(),
            kept: 0,
            past: 0,
            last_kept: 0,
        }
    }

    fn into_found(self) -> Found {
        Found {
            lines: self.lines,
            kept: self.kept,
            past: self.past,
        }
    }
}

impl Sink for Collector<'_> {
    type Error = io::Error;

    fn matched(&mut self, _searcher: &Searcher, found: &SinkMatch<'_>) -> Result<bool, io::Error> {
        if self.kept == self.budget {
            self.past += 1;
            return Ok(self.counting);
        }

        let number = line_number(found.line_number());
        let (text, cut) = clip_match(found.bytes(), self.matcher, self.max_line_bytes);
        self.lines.push(Line {
            number,
            kind: LineKind::Match,
            text,
            cut,
        });
        self.kept += 1;
        self.last_kept = number;

        Ok(true)
    }

    fn context(
        &mut self,
        _searcher: &Searcher,
        context: &SinkContext<'_>,
    ) -> Result<bool, io::Error> {
        let number = line_number(context.line_number());
        let trails_kept_match = self.kept > 0
            && self.past == 0
            && number <= self.last_kept.saturating_add(self.context);
        let belongs = self.kept < self.budget || trails_kept_match;

        if belongs {
            let (text, cut) = clip_start(context.bytes(), self.max_line_bytes);
            self.lines.push(Line {
                number,
                kind: LineKind::Context,
                text,
                cut,
            });
        }

        Ok(true)
    }
}

#[expect(
    clippy::expect_used,
    reason = "every searcher here is built with line numbers on, so a line without one is a broken build of it"
)]
const fn line_number(number: Option<u64>) -> u64 {
    number.expect("the searcher counts lines")
}

fn clip_match(
    bytes: &[u8],
    matcher: &RegexMatcher,
    max: Option<NonZeroUsize>,
) -> (Box<str>, Option<Cut>) {
    let line = without_line_end(bytes);
    let Some(max) = max.map(NonZeroUsize::get).filter(|&max| max < line.len()) else {
        return (lossy(line), None);
    };

    let (start, end) = match matcher.find(line) {
        Ok(Some(found)) => window(found.start(), found.end(), line.len(), max),
        _ => (0, max),
    };

    clip(line, start, end)
}

fn clip_start(bytes: &[u8], max: Option<NonZeroUsize>) -> (Box<str>, Option<Cut>) {
    let line = without_line_end(bytes);
    max.map(NonZeroUsize::get)
        .filter(|&max| max < line.len())
        .map_or_else(|| (lossy(line), None), |max| clip(line, 0, max))
}

fn window(start: usize, end: usize, len: usize, max: usize) -> (usize, usize) {
    let matched = end - start;
    if matched >= max {
        return (start, start + max);
    }

    let lead = (max - matched) / 2;
    let from = start.saturating_sub(lead).min(len - max);

    (from, from + max)
}

fn clip(line: &[u8], mut start: usize, mut end: usize) -> (Box<str>, Option<Cut>) {
    while start < end && is_continuation(line[start]) {
        start += 1;
    }
    while end > start && end < line.len() && is_continuation(line[end]) {
        end -= 1;
    }
    if start == end && end < line.len() {
        end += 1;
        while end < line.len() && end - start < 4 && is_continuation(line[end]) {
            end += 1;
        }
    }

    let cut = Cut {
        before: start,
        after: line.len() - end,
    };

    (lossy(&line[start..end]), Some(cut))
}

const fn is_continuation(byte: u8) -> bool {
    byte & 0xC0 == 0x80
}

fn without_line_end(bytes: &[u8]) -> &[u8] {
    bytes
        .strip_suffix(b"\r\n")
        .or_else(|| bytes.strip_suffix(b"\n"))
        .unwrap_or(bytes)
}

fn lossy(bytes: &[u8]) -> Box<str> {
    String::from_utf8_lossy(bytes).into()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_window_centres_the_match_where_the_line_allows() {
        assert_eq!(window(50, 54, 100, 10), (47, 57));
        assert_eq!(window(1, 3, 100, 10), (0, 10), "the start bounds it");
        assert_eq!(window(97, 99, 100, 10), (90, 100), "the end bounds it");
        assert_eq!(
            window(20, 40, 100, 10),
            (20, 30),
            "a match wider than the window keeps its start"
        );
    }

    #[test]
    fn a_cut_never_splits_a_character() {
        let line = "aé€b".as_bytes();

        assert_eq!(&*clip(line, 0, 2).0, "a", "é starts at 1 and ends at 3");
        assert_eq!(
            &*clip(line, 2, 6).0,
            "€",
            "a window starting inside é moves past it"
        );
    }

    #[test]
    fn a_window_narrower_than_its_character_keeps_that_character_whole() {
        let line = "aé".as_bytes();

        assert_eq!(
            clip(line, 1, 2),
            (
                "é".into(),
                Some(Cut {
                    before: 1,
                    after: 0
                })
            )
        );
    }

    #[test]
    fn a_window_over_malformed_bytes_keeps_at_most_one_character_width() {
        let line = [0x80; 1000];

        let (_, cut) = clip(&line, 0, 10);

        let cut = cut.expect("the line is cut");
        assert!(
            line.len() - cut.before - cut.after <= 4,
            "a run of continuation bytes kept past one character: {cut:?}"
        );
    }

    #[test]
    fn a_line_loses_its_line_end_whichever_it_is() {
        assert_eq!(without_line_end(b"x\r\n"), b"x");
        assert_eq!(without_line_end(b"x\n"), b"x");
        assert_eq!(without_line_end(b"x"), b"x");
        assert_eq!(
            without_line_end(b"x\r"),
            b"x\r",
            "a lone carriage return ends no line"
        );
    }
}
