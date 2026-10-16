const MARKERS: [&[u8]; 3] = [
    b"@generated",
    b"DO NOT EDIT",
    b"This file is auto-generated",
];

const BYTE_ORDER_MARK: &[u8] = b"\xEF\xBB\xBF";

// constraint: only the leading run of `#` comments is the header, after a byte order mark that Ruby skips; the first
// code line ends the scan, and a `=begin` block comment is code to it
pub(crate) fn is_generated_source(source: &[u8]) -> bool {
    let source = source.strip_prefix(BYTE_ORDER_MARK).unwrap_or(source);
    for line in source.split(|&byte| byte == b'\n') {
        let line = line.trim_ascii();
        if line.is_empty() {
            continue;
        }
        if !line.starts_with(b"#") {
            return false;
        }
        if announces_generated(line) {
            return true;
        }
    }

    false
}

// constraint: a marker inside a backtick span documents the convention rather than claiming this file follows it
fn announces_generated(line: &[u8]) -> bool {
    let mut outside_span = true;
    for (at, &byte) in line.iter().enumerate() {
        if byte == b'`' {
            outside_span = !outside_span;
        } else if outside_span && MARKERS.iter().any(|marker| line[at..].starts_with(marker)) {
            return true;
        }
    }

    false
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_quoted_marker_only_mentions_the_convention() {
        assert!(is_generated_source(
            b"# This file is auto-generated\nclass A; end\n"
        ));
        assert!(!is_generated_source(
            b"# a `@generated` marker\nclass A; end\n"
        ));
        assert!(is_generated_source(
            b"# the `@generated` marker: @generated\nclass A; end\n"
        ));
    }

    #[test]
    fn a_byte_order_mark_leaves_the_header_in_place() {
        assert!(is_generated_source(
            b"\xEF\xBB\xBF# @generated\nclass A; end\n"
        ));
    }

    #[test]
    fn a_block_comment_ends_the_header() {
        assert!(!is_generated_source(
            b"=begin\n@generated\n=end\nclass A; end\n"
        ));
    }
}
