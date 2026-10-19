use kartta_lang::Banner;

const BANNER: Banner = Banner::new(&["@generated", "DO NOT EDIT"]);

// constraint: a minifier or a code generator writes a whole module on a few lines, and a hand-written file opens with
// a short line of code, so a first line of code longer than this is a build output
const MINIFIED_LINE: usize = 500;

const SCRIPT_EXTENSIONS: [&str; 11] = [
    ".d.ts", ".d.mts", ".d.cts", ".tsx", ".ts", ".mts", ".cts", ".jsx", ".js", ".mjs", ".cjs",
];

enum HeaderLine {
    Generated,
    Comment { block_open: bool },
    Code,
}

pub(crate) fn is_generated_source(source: &[u8]) -> bool {
    let source = source.strip_prefix(b"\xEF\xBB\xBF").unwrap_or(source);
    let mut block_open = false;

    for line in source.split(|&byte| byte == b'\n') {
        match header_line(line.trim_ascii(), block_open) {
            HeaderLine::Generated => return true,
            HeaderLine::Comment { block_open: open } => block_open = open,
            HeaderLine::Code => return line.len() > MINIFIED_LINE,
        }
    }

    false
}

fn header_line(mut line: &[u8], block_open: bool) -> HeaderLine {
    if block_open {
        let Some(close) = find(line, b"*/") else {
            return comment(line, true);
        };
        if BANNER.marks(&line[..close]) {
            return HeaderLine::Generated;
        }
        line = line[close + 2..].trim_ascii();
    }

    if line.is_empty() {
        return HeaderLine::Comment { block_open: false };
    }
    if line.starts_with(b"//") {
        return comment(line, false);
    }
    let Some(body) = line.strip_prefix(b"/*") else {
        return HeaderLine::Code;
    };
    let Some(close) = find(body, b"*/") else {
        return comment(line, true);
    };

    let block_end = close + 4;
    if BANNER.marks(&line[..block_end]) {
        HeaderLine::Generated
    } else if line[block_end..].trim_ascii().is_empty() {
        HeaderLine::Comment { block_open: false }
    } else {
        HeaderLine::Code
    }
}

fn comment(text: &[u8], block_open: bool) -> HeaderLine {
    if BANNER.marks(text) {
        HeaderLine::Generated
    } else {
        HeaderLine::Comment { block_open }
    }
}

fn find(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    haystack
        .windows(needle.len())
        .position(|window| window == needle)
}

pub(crate) fn name_marks_generated(name: &str) -> bool {
    if name.contains(".gen.") || name.contains(".generated.") {
        return true;
    }
    let stem = SCRIPT_EXTENSIONS
        .iter()
        .find_map(|extension| {
            let at = name.len().checked_sub(extension.len())?;
            let suffix = name.get(at..)?;
            suffix.eq_ignore_ascii_case(extension).then(|| &name[..at])
        })
        .unwrap_or(name);
    stem.strip_suffix("pb")
        .is_some_and(|rest| rest.ends_with(['_', '.']))
        || stem
            .strip_suffix("min")
            .is_some_and(|rest| rest.ends_with('.'))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_byte_order_mark_before_the_banner_hides_nothing() {
        assert!(is_generated_source(
            b"\xEF\xBB\xBF// @generated\nexport const a = 1;\n"
        ));
        assert!(is_generated_source(
            b"\xEF\xBB\xBF/* DO NOT EDIT */\nexport const a = 1;\n"
        ));
        assert!(!is_generated_source(
            b"\xEF\xBB\xBF// a header\nexport const a = 1;\n"
        ));
    }

    #[test]
    fn a_marker_name_counts_whatever_case_the_extension_is_in() {
        assert!(name_marks_generated("schema_pb.TS"));
        assert!(name_marks_generated("vendor.min.JS"));
        assert!(!name_marks_generated("schema.TS"));
    }
}
