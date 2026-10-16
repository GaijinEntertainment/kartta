use crate::content_index::{put_varint, read_varint};

pub(crate) type Trigram = u32;

const BITS: u32 = 24;
const SPACE: usize = 1 << BITS;
pub(crate) const KELVIN_SIGN: [u8; 3] = [0xE2, 0x84, 0xAA];
pub(crate) const LONG_S: [u8; 2] = [0xC5, 0xBF];
const FOLDED_SEQUENCES: [&[u8]; 2] = [&KELVIN_SIGN, &LONG_S];

// constraint: a cache shared with builds of other versions keys its trigrams by this, and the probe holds every byte
// value and every sequence `fold` folds, so a change to the folding, the window or `Trigrams::encode` changes it
pub(crate) fn fingerprint() -> Vec<u8> {
    let mut probe: Vec<u8> = (0..=u8::MAX).collect();
    for sequence in FOLDED_SEQUENCES {
        probe.push(b'a');
        probe.extend_from_slice(sequence);
        probe.push(b'b');
    }
    let mut trigrams = Trigrams::new();
    trigrams.feed(&probe);

    let mut fingerprint = Vec::new();
    trigrams.encode(&mut fingerprint);
    fingerprint
}

pub(crate) const fn pack(first: u8, second: u8, third: u8) -> Trigram {
    ((first as Trigram) << 16) | ((second as Trigram) << 8) | third as Trigram
}

// constraint: Unicode's simple case folding joins exactly these two non-ASCII code points to an ASCII letter, so folding
// them here lets a case-insensitive `k` or `s` narrow by the same trigrams whichever spelling a file holds
pub(crate) const fn fold(rest: &[u8]) -> (u8, usize) {
    match rest {
        [0xE2, 0x84, 0xAA, ..] => (b'k', KELVIN_SIGN.len()),
        [0xC5, 0xBF, ..] => (b's', LONG_S.len()),
        [byte, ..] => (byte.to_ascii_lowercase(), 1),
        [] => (0, 0),
    }
}

// constraint: `fold` folds a whole sequence to one letter, so bytes that end a chunk partway into one have to wait for
// the next chunk to complete it
pub(crate) fn unfinished_fold(chunk: &[u8]) -> usize {
    FOLDED_SEQUENCES
        .into_iter()
        .flat_map(|sequence| (1..sequence.len()).filter(|&len| chunk.ends_with(&sequence[..len])))
        .max()
        .unwrap_or(0)
}

pub(crate) struct Trigrams {
    seen: Box<[u64]>,
    found: Vec<Trigram>,
    window: Trigram,
    filled: u8,
}

impl Trigrams {
    pub(crate) fn new() -> Self {
        Self {
            seen: vec![0; SPACE / 64].into_boxed_slice(),
            found: Vec::new(),
            window: 0,
            filled: 0,
        }
    }

    pub(crate) fn feed(&mut self, mut bytes: &[u8]) {
        while !bytes.is_empty() {
            let (folded, width) = fold(bytes);
            bytes = &bytes[width..];

            self.window = ((self.window << 8) | Trigram::from(folded)) & ((1 << BITS) - 1);
            if self.filled < 2 {
                self.filled += 1;
                continue;
            }

            let (word, bit) = ((self.window / 64) as usize, 1 << (self.window % 64));
            if self.seen[word] & bit == 0 {
                self.seen[word] |= bit;
                self.found.push(self.window);
            }
        }
    }

    pub(crate) fn found(&self) -> &[Trigram] {
        &self.found
    }

    pub(crate) fn encode(&mut self, out: &mut Vec<u8>) {
        self.found.sort_unstable();
        let mut previous = 0;
        for &trigram in &self.found {
            put_varint(out, trigram - previous);
            previous = trigram;
        }
    }

    pub(crate) fn decode(&mut self, encoded: &[u8]) -> bool {
        let mut at = 0;
        let mut previous: Trigram = 0;
        while at < encoded.len() {
            let ascending = !self.found.is_empty();
            let next = read_varint(encoded, &mut at)
                .filter(|&delta| delta > 0 || !ascending)
                .and_then(|delta| previous.checked_add(delta))
                .filter(|&trigram| trigram >> BITS == 0);
            let Some(trigram) = next else {
                self.found.clear();
                return false;
            };
            self.found.push(trigram);
            previous = trigram;
        }

        true
    }

    pub(crate) fn clear(&mut self) {
        for &trigram in &self.found {
            self.seen[(trigram / 64) as usize] = 0;
        }
        self.found.clear();
        self.window = 0;
        self.filled = 0;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn trigram([a, b, c]: [u8; 3]) -> Trigram {
        pack(a, b, c)
    }

    fn found(text: &str) -> Vec<Trigram> {
        let mut trigrams = Trigrams::new();
        trigrams.feed(text.as_bytes());
        let mut found = trigrams.found().to_vec();
        found.sort_unstable();
        found
    }

    #[test]
    fn a_file_yields_each_folded_trigram_once() {
        assert_eq!(found("AbCd"), [trigram(*b"abc"), trigram(*b"bcd")]);
        assert_eq!(found("aaaaa"), [trigram(*b"aaa")], "a repeat is found once");
        assert_eq!(found("ab"), [], "two bytes hold no trigram");
    }

    #[test]
    fn the_two_code_points_folding_to_an_ascii_letter_fold_to_it() {
        assert_eq!(
            found("a\u{212A}\u{17F}b"),
            [trigram(*b"aks"), trigram(*b"ksb")]
        );
    }

    #[test]
    fn other_bytes_past_ascii_stay_as_they_are() {
        let mut expected = vec![trigram([b'a', 0xC3, 0xA9]), trigram([0xC3, 0xA9, b'b'])];
        expected.sort_unstable();

        assert_eq!(found("aéb"), expected);
    }

    #[test]
    fn a_file_fed_in_pieces_yields_what_it_yields_whole() {
        let mut trigrams = Trigrams::new();
        trigrams.feed(b"abc");
        trigrams.feed(b"de");

        let mut pieces = trigrams.found().to_vec();
        pieces.sort_unstable();

        assert_eq!(pieces, found("abcde"));
    }

    #[test]
    fn a_chunk_ending_partway_into_a_folded_sequence_holds_that_part_back() {
        assert_eq!(unfinished_fold(b"ab\xE2"), 1);
        assert_eq!(unfinished_fold(b"ab\xE2\x84"), 2);
        assert_eq!(unfinished_fold(b"ab\xC5"), 1);
        assert_eq!(
            unfinished_fold("ab\u{212A}".as_bytes()),
            0,
            "a whole sequence"
        );
        assert_eq!(unfinished_fold(b"ab\x84"), 0, "no sequence starts there");
        assert_eq!(unfinished_fold(b""), 0);
    }

    #[test]
    fn a_cleared_extractor_starts_the_next_file_afresh() {
        let mut trigrams = Trigrams::new();
        trigrams.feed(b"abcd");
        trigrams.clear();
        trigrams.feed(b"xyabc");

        let mut next = trigrams.found().to_vec();
        next.sort_unstable();

        assert_eq!(
            next,
            found("xyabc"),
            "neither the window nor the seen set carries over"
        );
    }

    #[test]
    fn encoded_trigrams_decode_to_the_set_they_came_from() {
        let text = "\0\0\0 AbCd a\u{e9}b \u{212A}elvin";
        let mut source = Trigrams::new();
        source.feed(text.as_bytes());
        let mut encoded = Vec::new();
        source.encode(&mut encoded);

        let mut decoded = Trigrams::new();
        let whole = decoded.decode(&encoded);

        assert!(whole);
        assert_eq!(
            decoded.found(),
            found(text),
            "ascending, the NUL trigram first"
        );
    }

    #[test]
    fn the_fingerprint_holds_what_each_folded_sequence_folds_to() {
        let mut probed = Trigrams::new();

        let whole = probed.decode(&fingerprint());

        assert!(whole);
        for folded in [*b"akb", *b"asb", *b"abc", [0xFD, 0xFE, 0xFF]] {
            assert!(probed.found().contains(&trigram(folded)), "{folded:?}");
        }
    }

    #[test]
    fn an_encoding_no_encoder_writes_decodes_to_nothing() {
        let truncated: &[u8] = &[0x80];
        let repeated: &[u8] = &[0x05, 0x00];
        let wider_than_a_trigram: &[u8] = &[0x80, 0x80, 0x80, 0x08];

        for encoded in [truncated, repeated, wider_than_a_trigram] {
            let mut decoded = Trigrams::new();
            let whole = decoded.decode(encoded);

            assert!(!whole, "{encoded:?}");
            assert_eq!(decoded.found(), [], "{encoded:?}");
        }
    }
}
