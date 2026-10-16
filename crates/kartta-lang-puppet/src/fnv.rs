pub(crate) const FNV_OFFSET: u64 = 0xcbf2_9ce4_8422_2325;

pub(crate) const fn fnv(mut digest: u64, bytes: &[u8]) -> u64 {
    const PRIME: u64 = 0x0000_0100_0000_01b3;

    let mut at = 0;
    while at < bytes.len() {
        digest ^= bytes[at] as u64;
        digest = digest.wrapping_mul(PRIME);
        at += 1;
    }

    digest
}
