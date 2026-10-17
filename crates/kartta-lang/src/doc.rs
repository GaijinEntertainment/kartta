/// Where a language writes the comments that document a definition from the lines directly above it, and what else a
/// declaration carries on those lines: an attribute, a decorator, a template header.
///
/// A language states it through [`Language::doc_comments`](crate::Language::doc_comments), and a query that answers a
/// definition's text with its doc comment reads it from the file when asked, so no extraction records it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct DocComments {
    line_comments: &'static [&'static str],
    block_comments: &'static [BlockComment],
    carried: &'static [&'static str],
}

/// A comment that opens with one token and closes with another, `/*` and `*/`, across as many lines as it needs.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct BlockComment {
    open: &'static str,
    close: &'static str,
    closed_alone: bool,
}

impl DocComments {
    /// No line documents a definition.
    pub const NONE: Self = Self {
        line_comments: &[],
        block_comments: &[],
        carried: &[],
    };

    /// The comments of C: `//` to the end of the line, and `/*` to `*/`.
    pub const C_LIKE: Self = Self::lines(&["//"]).with_blocks(&[BlockComment::new("/*", "*/")]);

    /// Comments that start with one of `prefixes`, `//` or `#`, and run to the end of their line.
    #[must_use]
    pub const fn lines(prefixes: &'static [&'static str]) -> Self {
        Self {
            line_comments: prefixes,
            ..Self::NONE
        }
    }

    /// These comments, and `blocks`: one for each pair of tokens that opens and closes a block comment, as Lua's long
    /// comments open with `--[[`, `--[=[` or `--[==[` and close with a bracket of the same level.
    #[must_use]
    pub const fn with_blocks(self, blocks: &'static [BlockComment]) -> Self {
        Self {
            block_comments: blocks,
            ..self
        }
    }

    /// These comments, and the lines that start with one of `prefixes` and belong to the declaration below them, as
    /// `#[derive(Debug)]` does in Rust and `template <typename T>` in C++.
    #[must_use]
    pub const fn with_carried(self, prefixes: &'static [&'static str]) -> Self {
        Self {
            carried: prefixes,
            ..self
        }
    }

    /// The first line of what documents the definition that starts on the one-based `line` of `lines`, where
    /// `lines[0]` is line 1: the run of lines directly above it that, after their leading whitespace, start a line
    /// comment, a block comment or a carried line. A blank line ends the run. A line that ends with a closing token, a
    /// line comment after it aside, closes a block comment that counts whole from there up to the nearest line above
    /// that starts with the opening token and leaves a comment open at its end, whatever the lines between hold; a line
    /// between them that ends another comment means the token closes none, as `int x; // a */` below `/* old */` closes
    /// none. `line` itself when no
    /// such line is above it, and when it is past the line after the last of `lines`, where nothing above it is known.
    #[must_use]
    pub fn first_line(&self, lines: &[&[u8]], line: u32) -> u32 {
        let Ok(mut first) = usize::try_from(line) else {
            return line;
        };
        if first > lines.len() + 1 {
            return line;
        }
        while let Some(above) = first.checked_sub(2).and_then(|at| lines.get(at)) {
            let above = above.trim_ascii();
            if above.is_empty() {
                break;
            }
            let closing_at = first - 2;
            if let Some(opening) = self
                .block_comments
                .iter()
                .find_map(|block| block.opened_above(lines, closing_at, self.line_comments))
            {
                first = opening + 1;
                continue;
            }
            if !self.starts_a_line_of_doc(above) {
                break;
            }
            first -= 1;
        }
        u32::try_from(first).unwrap_or(line)
    }

    fn starts_a_line_of_doc(&self, line: &[u8]) -> bool {
        let opens = self.block_comments.iter().map(|block| block.open);
        self.line_comments
            .iter()
            .chain(self.carried)
            .copied()
            .chain(opens)
            .any(|prefix| line.starts_with(prefix.as_bytes()))
    }
}

impl BlockComment {
    /// A comment opened by `open` and closed by `close`.
    #[must_use]
    pub const fn new(open: &'static str, close: &'static str) -> Self {
        Self {
            open,
            close,
            closed_alone: false,
        }
    }

    /// This comment, closed only by a line that holds `close` alone, or right after the start of a line comment that
    /// `open` itself starts with, `]]` or `--]]`: what a language needs whose code can end with the closing token too,
    /// as a Lua index ends with `]]`.
    #[must_use]
    pub const fn closed_alone(self) -> Self {
        Self {
            closed_alone: true,
            ..self
        }
    }

    fn closes(self, line: &[u8], line_comments: &[&str]) -> bool {
        let close = self.close.as_bytes();
        if self.closed_alone {
            return line == close
                || line_comments
                    .iter()
                    .filter(|prefix| self.open.starts_with(*prefix))
                    .any(|prefix| {
                        line.strip_prefix(prefix.as_bytes()).map(<[u8]>::trim_ascii) == Some(close)
                    });
        }
        let Some(at) = last(line, self.close) else {
            return false;
        };
        let after = line[at + close.len()..].trim_ascii();
        let ends_the_line = after.is_empty()
            || line_comments
                .iter()
                .any(|prefix| after.starts_with(prefix.as_bytes()));
        ends_the_line && !line.starts_with(self.open.as_bytes())
    }

    fn ends_a_comment(self, line: &[u8], line_comments: &[&str]) -> bool {
        if self.closed_alone {
            self.closes(line, line_comments)
        } else {
            contains(line, self.close)
        }
    }

    fn opened_above(
        self,
        lines: &[&[u8]],
        closing_at: usize,
        line_comments: &[&str],
    ) -> Option<usize> {
        let closing = lines.get(closing_at)?.trim_ascii();
        if !self.closes(closing, line_comments) {
            return None;
        }
        let mut opening = None;
        for (at, line) in lines.get(..closing_at)?.iter().enumerate().rev() {
            let line = line.trim_ascii();
            if self.opens_a_comment_left_open(line) {
                opening = Some(at);
            } else if self.ends_a_comment(line, line_comments) {
                break;
            }
        }
        opening
    }

    // constraint: an opening token inside an open comment is its text, so the comment opens at the topmost such line
    // below the nearest line that ends another comment
    fn opens_a_comment_left_open(self, line: &[u8]) -> bool {
        let Some(rest) = line.strip_prefix(self.open.as_bytes()) else {
            return false;
        };
        match (last(rest, self.close), last(rest, self.open)) {
            (None, _) => true,
            (Some(closed), Some(opened)) => opened > closed,
            (Some(_), None) => false,
        }
    }
}

fn contains(line: &[u8], token: &str) -> bool {
    line.windows(token.len())
        .any(|window| window == token.as_bytes())
}

fn last(line: &[u8], token: &str) -> Option<usize> {
    line.windows(token.len())
        .rposition(|window| window == token.as_bytes())
}

#[cfg(test)]
mod tests {
    use super::*;

    const C: DocComments = DocComments::C_LIKE;

    fn first_line(doc: DocComments, text: &str, line: u32) -> u32 {
        let lines: Vec<&[u8]> = text.lines().map(str::as_bytes).collect();
        doc.first_line(&lines, line)
    }

    #[test]
    fn a_run_of_line_comments_above_a_definition_documents_it() {
        let text = "package a\n\n// Parse reads.\n// It fails on nothing.\nfunc Parse() {}\n";
        assert_eq!(first_line(C, text, 5), 3);
    }

    #[test]
    fn a_blank_line_ends_the_run() {
        let text = "// About the file.\n\n// Parse reads.\nfunc Parse() {}\n";
        assert_eq!(first_line(C, text, 4), 3);
    }

    #[test]
    fn code_above_documents_nothing() {
        let text = "int x = 1;\nint parse() {}\n";
        assert_eq!(first_line(C, text, 2), 2);
        assert_eq!(first_line(C, text, 1), 1);
    }

    #[test]
    fn a_block_comment_counts_whole_whatever_its_lines_hold() {
        let text = "x;\n/**\n * Parses.\n\n * @return nothing\n */\nint parse();\n";
        assert_eq!(first_line(C, text, 7), 2);
    }

    #[test]
    fn a_block_comment_on_one_line_counts_as_a_line_of_doc() {
        let text = "x;\n/* Parses. */\n// More.\nint parse();\n";
        assert_eq!(first_line(C, text, 4), 2);
    }

    #[test]
    fn a_comment_that_closes_after_code_documents_nothing() {
        let text = "/* old */\nint x; /* trailing */\nint parse();\n";
        assert_eq!(first_line(C, text, 3), 3);
    }

    #[test]
    fn a_closing_line_whose_opening_line_is_missing_documents_nothing() {
        let text = "int y; /* opened\n * on code */\nint parse();\n";
        assert_eq!(first_line(C, text, 3), 3);
        let text = "a */\nb */\nint parse();\n";
        assert_eq!(first_line(C, text, 3), 3);
    }

    #[test]
    fn a_closing_token_never_reaches_past_code_to_an_older_comment() {
        let text = "/* old */\nint x; // a */\nint parse();\n";
        assert_eq!(first_line(C, text, 3), 3);
        let text = "/* old */\nint y; /* opened\n on code */\nint parse();\n";
        assert_eq!(first_line(C, text, 4), 4);
        let text = "/** Parses,\n * see https://example.com */\nint parse();\n";
        assert_eq!(first_line(C, text, 3), 1);
    }

    #[test]
    fn an_opening_token_inside_a_comment_s_text_opens_nothing() {
        let text = "/**\n * Matches src/*.ts and http://*.\n */\nint parse();\n";
        assert_eq!(first_line(C, text, 4), 1);
        let text = "/*// commented out\nint old();\n*/\nint parse();\n";
        assert_eq!(first_line(C, text, 4), 1);
        let text = "/**\n * Parses,\n * an opener written /* in text */\nint parse();\n";
        assert_eq!(first_line(C, text, 4), 1);
        let text = "x;\n/* Parses,\n/* a line of text\n */\nint parse();\n";
        assert_eq!(first_line(C, text, 5), 2);
    }

    #[test]
    fn a_comment_opened_after_another_closes_on_its_line_opens_the_block() {
        let text = "/* Buffers. */\n/* old; *//*\n * now elsewhere\n */\nint size;\n";
        assert_eq!(first_line(C, text, 5), 1);
    }

    #[test]
    fn a_line_comment_after_the_closing_token_still_closes_the_block() {
        let text = "/* Parses,\n * in detail.\n */ // note\nint parse();\n";
        assert_eq!(first_line(C, text, 4), 1);
        let text = "/* old */\nint x; /* a */ // b\nint parse();\n";
        assert_eq!(first_line(C, text, 3), 3);
    }

    #[test]
    fn a_carried_line_belongs_to_the_declaration_below_it() {
        let rust = DocComments::lines(&["//"]).with_carried(&["#["]);
        let text = "/// Parses.\n#[must_use]\n#[inline]\npub fn parse() {}\n";
        assert_eq!(first_line(rust, text, 4), 1);
        assert_eq!(first_line(C, text, 4), 4);
    }

    const LUA: DocComments = DocComments::lines(&["--"]).with_blocks(&[
        BlockComment::new("--[[", "]]").closed_alone(),
        BlockComment::new("--[=[", "]=]").closed_alone(),
    ]);

    #[test]
    fn a_block_closed_alone_ignores_code_that_ends_with_its_closer() {
        let lua = LUA;
        let text = "--[[\nParses.\n]]\nfunction parse() end\n";
        assert_eq!(first_line(lua, text, 4), 1);
        let text = "--[[ Parses.\n--]]\nfunction parse() end\n";
        assert_eq!(first_line(lua, text, 3), 1);
        let text = "x = t[a[1]]\nfunction parse() end\n";
        assert_eq!(first_line(lua, text, 2), 2);
    }

    #[test]
    fn each_block_closes_at_its_own_closer_whatever_another_s_closer_inside_it() {
        let text = "--[=[\nParses t[a[1]]\n]]\n]=]\nfunction parse() end\n";
        assert_eq!(first_line(LUA, text, 5), 1);
    }

    #[test]
    fn a_line_past_the_text_has_nothing_known_above_it() {
        let text = "// Parses.\nfn parse() {}\n";
        assert_eq!(first_line(C, text, 3), 3);
        assert_eq!(first_line(C, text, 9), 9);
        assert_eq!(first_line(DocComments::NONE, text, 2), 2);
    }
}
