// constraint: a first byte from 1 to 6 starts a binary stream; a file of 12 bytes or more is also binary when its
// header is a BBF3 one (`\0BBF`, `\0BBz`), and the loader rejects the obsolete `blk 1.1` and `SB`..`blk` headers
// without reading the file as text; any other file is text, the empty file included
pub(crate) const fn loads_as_text(source: &[u8]) -> bool {
    !matches!(
        source,
        [1..=6, ..]
            | [0, b'B', b'B', b'F' | b'z', _, _, _, _, _, _, _, _, ..]
            | [b'b', b'l', b'k', b' ', 0, b'1', b'.', b'1', _, _, _, _, ..]
            | [0, 0, b'S', b'B', _, _, _, _, 0, b'b', b'l', b'k', ..]
    )
}

pub(crate) fn source_text(source: &[u8]) -> &[u8] {
    if loads_as_text(source) && (may_hold_include(source) || may_state_mounts(source)) {
        source
    } else {
        &[]
    }
}

// constraint: a quoted name that spells `mountPoints` with `~` escapes or splits it with CR bytes keeps the parse
// through `may_hold_include`, so only the bare word with no word byte beside it is looked for here
fn may_state_mounts(text: &[u8]) -> bool {
    const NAME: &[u8] = b"mountPoints";
    let is_name_byte = |byte: &u8| byte.is_ascii_alphanumeric() || *byte == b'_';

    text.windows(NAME.len()).enumerate().any(|(at, window)| {
        window == NAME
            && !at
                .checked_sub(1)
                .is_some_and(|before| is_name_byte(&text[before]))
            && !text.get(at + NAME.len()).is_some_and(is_name_byte)
    })
}

// constraint: the parser reads a statement name as `include` only from those seven bytes in any case, from a quoted
// name that spells them with `~` escapes, and from a triple-quoted name that splits them with the CR bytes it drops;
// the last two are not spelled out here, so any text with a `~`, or with a CR and a triple quote, is parsed
fn may_hold_include(text: &[u8]) -> bool {
    text.windows(7)
        .any(|window| window.eq_ignore_ascii_case(b"include"))
        || text.contains(&b'~')
        || (text.contains(&b'\r')
            && (find(text, b"\"\"\"").is_some() || find(text, b"'''").is_some()))
}

pub(crate) fn find(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    haystack
        .windows(needle.len())
        .position(|window| window == needle)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_text_that_may_name_a_mount_block_is_parsed_without_an_include() {
        let parsed: &[&[u8]] = &[
            b"mountPoints{ a{ forSource:t=\"../a\"; } }\n",
            b"x:i=1\n\"mountPoints\" { a{ forSource:t=\"../a\"; } }\n",
            b"mountPoints-x{}\n",
        ];
        let skipped: &[&[u8]] = &[
            b"gun__mountPoints:t=\"x\"\n",
            b"mountPoints2{}\n",
            b"MountPoints{}\n",
        ];

        for source in parsed {
            assert_eq!(
                source_text(source),
                *source,
                "{:?}",
                String::from_utf8_lossy(source)
            );
        }
        for source in skipped {
            assert_eq!(
                source_text(source),
                b"",
                "{:?}",
                String::from_utf8_lossy(source)
            );
        }
    }

    #[test]
    fn the_engine_header_rules_tell_binary_from_text() {
        let binary: &[&[u8]] = &[
            b"\x01rest",
            b"\x02\xa5\x40\x03\x28\xb5\x2f\xfd",
            b"\x06",
            b"\0BBF\x03\0\0\0\x10\0\0\0",
            b"\0BBz\x10\0\0\0\x10\0\0\0",
            b"blk \x001.1\0\0\0\0",
            b"\0\0SB\0\0\0\0\0blk",
        ];
        let text: &[&[u8]] = &[
            b"",
            b"a:i=1",
            b"\0BBF",
            b"blk \x001.1\0\0\0",
            b"\0\0SB\0\0\0\0\0bl",
            b"\x07include x.blk\n",
            b"\xef\xbb\xbfa:i=1\nb:i=2\n",
            b"\0BBQ\x03\0\0\0\x10\0\0\0",
        ];

        for source in binary {
            assert!(!loads_as_text(source), "{source:?} loads as binary");
        }
        for source in text {
            assert!(loads_as_text(source), "{source:?} loads as text");
        }
    }

    #[test]
    fn the_prefilter_keeps_every_text_that_may_spell_the_keyword() {
        let skipped: &[&[u8]] = &[
            b"a:i=1\nb{c:t=\"incl ude\"}\n",
            b"block{\r\n  name:t=\"x\"\r\n}\r\n",
            b"name:t=\"\"\"inc\nlude\"\"\"\n",
        ];
        let parsed: &[&[u8]] = &[
            b"includes:i=1\n",
            b"a:t=\"x~y\"\n",
            b"name:t=\"\"\"x\"\"\"\r\n",
        ];

        for source in skipped {
            assert_eq!(
                source_text(source),
                b"",
                "{:?}",
                String::from_utf8_lossy(source)
            );
        }
        for source in parsed {
            assert_eq!(
                source_text(source),
                *source,
                "{:?}",
                String::from_utf8_lossy(source)
            );
        }
    }

    #[test]
    fn the_parser_reads_no_byte_of_a_binary_header() {
        let bbf3_header = b"\0BBF\x03\0\0\0\x10\0\0\0include x.blk\n";
        let obsolete_header = b"\0\0SB\0\0\0\0\0blkinclude x.blk\n";
        let short_bbf3_prefix = b"\0BBFinclude";

        assert_eq!(source_text(bbf3_header), b"");
        assert_eq!(source_text(obsolete_header), b"");
        assert_eq!(source_text(short_bbf3_prefix), short_bbf3_prefix);
    }
}
