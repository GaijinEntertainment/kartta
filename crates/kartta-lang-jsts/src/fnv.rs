pub(crate) const OFFSET: u64 = 0xcbf2_9ce4_8422_2325;

const PRIME: u64 = 0x0100_0000_01b3;

pub(crate) fn field(hash: u64, bytes: &[u8]) -> u64 {
    let length = u64::try_from(bytes.len()).unwrap_or(u64::MAX);

    fnv1a(fnv1a(hash, &length.to_le_bytes()), bytes)
}

fn fnv1a(hash: u64, bytes: &[u8]) -> u64 {
    bytes.iter().fold(hash, |hash, &byte| {
        (hash ^ u64::from(byte)).wrapping_mul(PRIME)
    })
}
