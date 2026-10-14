const SCAN_BITS: usize = 56;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Shape {
    len: u32,
    universe: u32,
    low_bits: u32,
    bitmap: bool,
}

impl Shape {
    pub(crate) fn new(len: u32, universe: u32) -> Self {
        let low_bits = (universe / len.max(1)).checked_ilog2().unwrap_or(0);
        let elias_fano_bits = elias_fano_bits(len, universe, low_bits);

        Self {
            len,
            universe,
            low_bits,
            bitmap: elias_fano_bits >= universe as usize,
        }
    }

    pub(crate) const fn bytes(self) -> usize {
        let bits = if self.bitmap {
            self.universe as usize
        } else {
            elias_fano_bits(self.len, self.universe, self.low_bits)
        };

        bits.div_ceil(8)
    }

    const fn low_len(self) -> usize {
        self.len as usize * self.low_bits as usize
    }
}

const fn elias_fano_bits(len: u32, universe: u32, low_bits: u32) -> usize {
    len as usize * (low_bits as usize + 1) + (universe >> low_bits) as usize
}

pub(crate) struct Writer<'a> {
    shape: Shape,
    out: &'a mut [u8],
    pushed: usize,
    lowest_next: u32,
}

impl<'a> Writer<'a> {
    pub(crate) fn new(shape: Shape, out: &'a mut [u8]) -> Self {
        debug_assert_eq!(
            out.len(),
            shape.bytes(),
            "a list is written into exactly its own bytes"
        );
        debug_assert!(
            out.iter().all(|&byte| byte == 0),
            "a list is written over zeroes"
        );

        Self {
            shape,
            out,
            pushed: 0,
            lowest_next: 0,
        }
    }

    pub(crate) fn push(&mut self, id: u32) {
        debug_assert!(id < self.shape.universe, "an id lies inside the universe");
        debug_assert!(
            id >= self.lowest_next,
            "ids arrive in ascending order, each once, which both layouts depend on"
        );
        debug_assert!(
            self.pushed < self.shape.len as usize,
            "a list holds no more ids than its shape"
        );
        self.lowest_next = id.saturating_add(1);

        if self.shape.bitmap {
            or_bits(self.out, id as usize, 1);
        } else {
            let low_bits = self.shape.low_bits;
            let low = u64::from(id) & ((1 << low_bits) - 1);
            or_bits(self.out, self.pushed * low_bits as usize, low);
            let high = (id >> low_bits) as usize;
            or_bits(self.out, self.shape.low_len() + high + self.pushed, 1);
        }
        self.pushed += 1;
    }
}

pub(crate) fn for_each(shape: Shape, bytes: &[u8], mut each: impl FnMut(u32)) {
    if shape.bitmap {
        for_each_set(bytes, 0, shape.universe as usize, |at| {
            each(narrow(at as u64));
        });
        return;
    }

    let low_bits = shape.low_bits;
    let low_mask = (1 << low_bits) - 1;
    let mut seen = 0;
    for_each_set(bytes, shape.low_len(), bytes.len() * 8, |at| {
        let high = (at - shape.low_len() - seen) as u64;
        let low = bits_at(bytes, seen * low_bits as usize) & low_mask;
        each(narrow((high << low_bits) | low));
        seen += 1;
    });
}

fn for_each_set(bytes: &[u8], from: usize, to: usize, mut each: impl FnMut(usize)) {
    let mut at = from;

    while at < to {
        let mut chunk = bits_at(bytes, at) & ((1 << SCAN_BITS) - 1);
        while chunk != 0 {
            let found = at + chunk.trailing_zeros() as usize;
            if found >= to {
                return;
            }
            each(found);
            chunk &= chunk - 1;
        }
        at += SCAN_BITS;
    }
}

// constraint: `value` holds at most 57 bits, so shifted by the bit offset inside its first byte it still fits one word
fn or_bits(out: &mut [u8], at: usize, value: u64) {
    let shifted = value << (at % 8);

    for (byte, part) in out[at / 8..].iter_mut().zip(shifted.to_le_bytes()) {
        *byte |= part;
    }
}

fn bits_at(bytes: &[u8], at: usize) -> u64 {
    let rest = bytes.get(at / 8..).unwrap_or_default();
    let mut word = [0; 8];
    let filled = rest.len().min(word.len());
    word[..filled].copy_from_slice(&rest[..filled]);

    u64::from_le_bytes(word) >> (at % 8)
}

#[expect(
    clippy::cast_possible_truncation,
    reason = "a decoded id lies inside a universe that is itself a u32"
)]
const fn narrow(id: u64) -> u32 {
    id as u32
}

#[cfg(test)]
mod tests {
    use super::*;

    fn written(ids: &[u32], universe: u32) -> (Shape, Vec<u8>) {
        let shape = Shape::new(u32::try_from(ids.len()).expect("a short list"), universe);
        let mut bytes = vec![0; shape.bytes()];
        let mut writer = Writer::new(shape, &mut bytes);
        for &id in ids {
            writer.push(id);
        }

        (shape, bytes)
    }

    fn read(shape: Shape, bytes: &[u8]) -> Vec<u32> {
        let mut ids = Vec::new();
        for_each(shape, bytes, |id| ids.push(id));
        ids
    }

    #[test]
    fn a_list_reads_back_as_written_across_densities() {
        for universe in [1, 2, 7, 8, 9, 63, 64, 65, 1000, 168_885] {
            let lens = [1, 2, 3, 10, 100, 1000, 50_000, 168_885];
            for len in lens.into_iter().filter(|&len| len <= universe) {
                let step = universe / len;
                let ids: Vec<u32> = (0..len)
                    .map(|at| at * step + (at % 3).min(step - 1))
                    .collect();

                let (shape, bytes) = written(&ids, universe);

                assert_eq!(read(shape, &bytes), ids, "{len} ids out of {universe}");
            }
        }
    }

    #[test]
    fn a_dense_list_is_a_bitmap_and_a_sparse_one_is_not() {
        assert!(Shape::new(500, 1000).bitmap, "half the universe");
        assert!(!Shape::new(10, 1000).bitmap, "a hundredth of it");
        assert_eq!(
            Shape::new(500, 1000).bytes(),
            125,
            "one bit per id in the universe"
        );
    }

    #[test]
    fn a_list_takes_the_fewer_bytes_of_its_two_layouts() {
        for len in [1, 10, 100, 200, 300, 400, 1000] {
            let shape = Shape::new(len, 1000);
            let elias_fano = elias_fano_bits(len, 1000, shape.low_bits).div_ceil(8);

            assert_eq!(shape.bytes(), elias_fano.min(125), "{len} ids out of 1000");
        }
    }

    #[test]
    fn low_bits_that_straddle_a_word_read_back_whole() {
        let ids: Vec<u32> = (0..40).map(|at| at * 100_003 + 99_999).collect();

        let (shape, bytes) = written(&ids, 4_100_000);

        assert!(shape.low_bits > 8, "each id's low part spans several bytes");
        assert_eq!(read(shape, &bytes), ids);
    }

    #[test]
    #[cfg(debug_assertions)]
    #[should_panic(expected = "ascending order")]
    fn an_id_pushed_out_of_order_is_refused() {
        written(&[5, 3], 100);
    }

    #[test]
    fn the_last_id_of_the_universe_reads_back() {
        for universe in [1, 64, 1000, 4096] {
            let (shape, bytes) = written(&[universe - 1], universe);

            assert_eq!(read(shape, &bytes), [universe - 1], "out of {universe}");
        }
    }
}
