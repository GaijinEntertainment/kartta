use std::fmt;

/// The blake3 hash of a file's bytes: the content half of every key.
///
/// The hash is the identity of the content, so two files with the same bytes share every entry cached for them,
/// wherever they sit and whichever tree holds them. Collision resistance is load-bearing: the bytes come from
/// repositories, which are not trusted input, and a collision would hand one file the entry computed for another.
#[derive(Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct ContentHash([u8; 32]);

impl ContentHash {
    /// Hashes the whole of `bytes`.
    #[must_use]
    pub fn of(bytes: &[u8]) -> Self {
        Self(*blake3::hash(bytes).as_bytes())
    }

    /// Wraps a hash computed elsewhere, such as one a [`ContentHasher`] finished or one read back from storage.
    #[must_use]
    pub const fn from_bytes(bytes: [u8; 32]) -> Self {
        Self(bytes)
    }

    /// The hash's 32 bytes.
    #[must_use]
    pub const fn as_bytes(&self) -> &[u8; 32] {
        &self.0
    }
}

impl fmt::Debug for ContentHash {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "ContentHash({})",
            blake3::Hash::from_bytes(self.0).to_hex()
        )
    }
}

/// Hashes a file's bytes as they arrive, so a reader that streams a file in chunks has its [`ContentHash`] when the
/// last chunk is in, without holding the file whole.
///
/// The hash is the one [`ContentHash::of`] gives for the same bytes, however they were split.
#[derive(Debug, Clone, Default)]
pub struct ContentHasher(blake3::Hasher);

impl ContentHasher {
    /// A hasher that has seen no bytes.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Feeds the next bytes of the content.
    pub fn update(&mut self, bytes: &[u8]) -> &mut Self {
        self.0.update(bytes);
        self
    }

    /// The hash of every byte fed so far. The hasher stays usable, so more bytes can follow.
    #[must_use]
    pub fn finish(&self) -> ContentHash {
        ContentHash(*self.0.finalize().as_bytes())
    }
}

/// Which producer, at which version, an entry belongs to: the other half of every key.
///
/// Entries of different namespaces never answer for each other, so one store holds every kind of data cached per
/// content side by side, and two versions of one producer keep their own entries rather than overwriting each other's
/// while both run against the same store.
#[derive(Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct Namespace([u8; 32]);

impl Namespace {
    /// The namespace of `kind`, the data being cached (`"trigrams"`, `"graph/cpp"`), as computed by the code
    /// `fingerprint` identifies.
    ///
    /// The fingerprint must change whenever what the producer computes from the same bytes can change — its rules, its
    /// grammar, its encoding — and a producer derives it from those inputs rather than maintaining it by hand: an
    /// entry computed by older code then sits in another namespace and is never served to newer code.
    #[must_use]
    pub fn new(kind: &str, fingerprint: &[u8]) -> Self {
        let mut hasher = blake3::Hasher::new_derive_key("kartta-cache 2026-10-16 namespace");
        hasher.update(&u64::try_from(kind.len()).unwrap_or(u64::MAX).to_le_bytes());
        hasher.update(kind.as_bytes());
        hasher.update(fingerprint);

        Self(*hasher.finalize().as_bytes())
    }

    pub(crate) const fn from_bytes(bytes: [u8; 32]) -> Self {
        Self(bytes)
    }

    pub(crate) const fn as_bytes(&self) -> &[u8; 32] {
        &self.0
    }
}

impl fmt::Debug for Namespace {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "Namespace({})",
            blake3::Hash::from_bytes(self.0).to_hex()
        )
    }
}

pub(crate) fn check(namespace: &Namespace, content: &ContentHash, payload: &[u8]) -> u64 {
    let mut hasher = blake3::Hasher::new();
    hasher.update(namespace.as_bytes());
    hasher.update(content.as_bytes());
    hasher.update(payload);
    let mut head = [0; 8];
    head.copy_from_slice(&hasher.finalize().as_bytes()[..8]);

    u64::from_le_bytes(head)
}
