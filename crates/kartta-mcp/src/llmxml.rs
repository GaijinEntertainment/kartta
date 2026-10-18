use std::borrow::Cow;
use std::fmt::{Display, Write};

/// `text` as one record of a plain list: as it is, or, when it holds a control character or opens with a quote, quoted
/// and escaped as an attribute value is, so a line break in a path cannot split its record and a quoted record cannot
/// be read as one written as it is.
pub(crate) fn line(text: &str) -> Cow<'_, str> {
    if text.starts_with('"') || text.contains(char::is_control) {
        Cow::Owned(format!("{text:?}"))
    } else {
        Cow::Borrowed(text)
    }
}

/// The plain list of `texts`, a [`line`] each.
pub(crate) fn lines(texts: &[String]) -> String {
    let mut list = List::default();
    for text in texts {
        list.push(text);
    }
    list.into_text()
}

/// A plain list written a record at a time, each as [`line`] writes it, into one buffer.
#[derive(Debug, Default)]
pub(crate) struct List {
    text: String,
    len: usize,
}

impl List {
    /// Appends `record` on a line of its own.
    pub(crate) fn push(&mut self, record: &str) {
        if self.len > 0 {
            self.text.push('\n');
        }
        self.text.push_str(&line(record));
        self.len += 1;
    }

    /// How many records the list holds.
    pub(crate) const fn len(&self) -> usize {
        self.len
    }

    /// The records, one a line.
    pub(crate) fn into_text(self) -> String {
        self.text
    }
}

/// One element of an XML-like answer, rendered straight to a string: the opening tag gathers attributes, and one
/// terminal call renders the element.
///
/// The dialect is written for a model, not a parser. Nothing is indented, and an element's body is not escaped, so code
/// in it reads as written. An attribute value is escaped as a Rust string literal: a quote and a backslash keep the
/// value inside its quotes, and a newline keeps the record on one line.
#[derive(Debug)]
pub(crate) struct Element {
    tag: &'static str,
    head: String,
}

impl Element {
    pub(crate) fn new(tag: &'static str) -> Self {
        Self {
            tag,
            head: format!("<{tag}"),
        }
    }

    /// Appends `key="value"`.
    pub(crate) fn attr(mut self, key: &str, value: impl Display) -> Self {
        let _ = write!(self.head, " {key}={:?}", value.to_string());
        self
    }

    /// Appends `key="value"` when `value` is not zero: a count of what an answer left out is shown only when it left
    /// something out.
    pub(crate) fn count(self, key: &str, value: usize) -> Self {
        if value == 0 {
            self
        } else {
            self.attr(key, value)
        }
    }

    /// Appends `key="true"` when `set`.
    pub(crate) fn flag(self, key: &str, set: bool) -> Self {
        if set { self.attr(key, true) } else { self }
    }

    /// Renders `<tag attrs/>`.
    pub(crate) fn close(self) -> String {
        self.head + "/>"
    }

    /// Renders `<tag attrs>text</tag>` on one line.
    pub(crate) fn inline(self, text: &str) -> String {
        format!("{}>{text}</{}>", self.head, self.tag)
    }

    /// Renders `<tag attrs>`, `body` on the lines after it, and `</tag>` on its own line.
    pub(crate) fn wrap(self, body: &str) -> String {
        format!("{}>\n{body}\n</{}>", self.head, self.tag)
    }

    /// Renders the children joined by newlines inside the element, or the element self-closed when there are none.
    pub(crate) fn children(self, children: impl IntoIterator<Item = String>) -> String {
        let mut body = String::new();
        for child in children {
            if !body.is_empty() {
                body.push('\n');
            }
            body.push_str(&child);
        }
        if body.is_empty() {
            self.close()
        } else {
            self.wrap(&body)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{Element, lines};

    #[test]
    fn a_record_with_a_line_break_or_an_opening_quote_is_quoted_on_its_line() {
        let records = [
            "src/main.rs".to_owned(),
            "src/two\nlines.rs".to_owned(),
            "src/tab\there.rs".to_owned(),
            r#""quoted".rs"#.to_owned(),
            r"src\back.rs".to_owned(),
        ];

        assert_eq!(
            lines(&records),
            [
                "src/main.rs",
                r#""src/two\nlines.rs""#,
                r#""src/tab\there.rs""#,
                r#""\"quoted\".rs""#,
                r"src\back.rs",
            ]
            .join("\n")
        );
    }

    #[test]
    fn an_attribute_value_stays_inside_its_quotes_on_one_line() {
        let rendered = Element::new("x")
            .attr("quoted", r#"say "hi""#)
            .attr("lines", "one\ntwo")
            .attr("root", r"C:\repo\")
            .attr("count", 42)
            .close();

        assert_eq!(
            rendered,
            r#"<x quoted="say \"hi\"" lines="one\ntwo" root="C:\\repo\\" count="42"/>"#
        );
    }

    #[test]
    fn a_zero_count_and_an_unset_flag_render_nothing() {
        let rendered = Element::new("x")
            .count("left", 0)
            .count("cut", 3)
            .flag("truncated", false)
            .flag("fuzzy", true)
            .close();

        assert_eq!(rendered, r#"<x cut="3" fuzzy="true"/>"#);
    }

    #[test]
    fn a_body_is_not_escaped() {
        assert_eq!(
            Element::new("note").inline(r#"a "quoted" <bare> & raw"#),
            r#"<note>a "quoted" <bare> & raw</note>"#
        );
    }

    #[test]
    fn children_join_by_line_and_none_self_closes() {
        let parent = Element::new("p")
            .attr("id", 1)
            .children([Element::new("a").close(), Element::new("b").inline("text")]);

        assert_eq!(parent, "<p id=\"1\">\n<a/>\n<b>text</b>\n</p>");
        assert_eq!(Element::new("p").children([]), "<p/>");
    }
}
