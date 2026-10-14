pub(crate) type Trigram = u32;

const BITS: u32 = 24;
const SPACE: usize = 1 << BITS;
pub(crate) const KELVIN_SIGN: [u8; 3] = [0xE2, 0x84, 0xAA];
pub(crate) const LONG_S: [u8; 2] = [0xC5, 0xBF];

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
}
