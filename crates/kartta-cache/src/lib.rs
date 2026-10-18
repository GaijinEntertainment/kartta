//! Content-addressed cache shared by concurrent processes, stored as immutable mapped packs.
//!
//! Processes that index nearly identical trees compute the same per-file data over and over: two checkouts that differ
//! in a hundred files out of a hundred thousand share everything else. This crate lets them share it. Every entry is
//! keyed by what it was computed from — the [`ContentHash`] of a file's bytes — and by what computed it — a
//! [`Namespace`] naming the kind of data and the version of the code that produced it. Nothing in a key names a path or
//! a tree, so identical contents share one entry wherever they sit, and a namespace keeps each producer, and each
//! version of one, apart.
//!
//! # Packs
//!
//! Entries are not stored one file each. Opening a file per lookup is what such a store spends its time on, and on
//! macOS a file open costs more than computing a small entry again. A process instead gathers the entries it computed
//! in a [`PackWriter`] and publishes them together as one pack: payloads, then an index sorted by key, then a footer.
//! [`Store::open`] maps every pack in the directory, and [`Store::get`] binary-searches their indexes, so a lookup
//! opens nothing and copies nothing: the payload is a borrow of the mapping.
//!
//! # Sharing without coordination
//!
//! A published pack is never written again. The writer stages a pack under a temporary name in the store's
//! directory, syncs it, and links it under a name derived from its index; a link creates a name only where none exists,
//! so a published pack is never replaced, and a pack is visible either whole or not at all. Processes therefore read
//! and publish in the same directory at once with no lock and no shared state beyond the files: a reader keeps the
//! packs it mapped for as long as it is open, and sees a pack published after it opened only when it opens again.
//!
//! Each entry carries a 64-bit check over its key and payload, verified on every lookup, so a changed payload byte, or
//! an index entry that points at the wrong bytes, escapes it only with a chance of 2^-64: the entry is passed over,
//! another pack's sound copy answers instead, or the lookup misses and is counted in [`Stats::damaged`]. A changed byte
//! in an entry's key makes the entry look absent, so its lookup misses and counts as a miss. Either way no wrong data
//! is served, and a cache that served wrong data would turn into wrong answers downstream. The check guards against
//! damage, not against a writer: a process that can write the directory can publish any pack it likes.
//!
//! Each publish of entries adds a pack, unless the same pack is already in place, and [`Store::merge`] keeps their
//! number bounded: past eight it merges every pack but the largest into one while the largest holds more entries than
//! all of them together, every pack otherwise, and removes what it merged. The caller decides when to merge — after
//! publishing is the natural moment — because only it knows how many processes share the directory.
//!
//! # Expiry
//!
//! Every entry carries the time it was published, and a merge removes the entries older than the time to live its
//! caller gives — [`DEFAULT_TTL`], a week, unless the caller has reason to pick another — so entries for contents no
//! tree holds any more, and every entry of a namespace no producer computes any more, leave the store. A lookup does
//! not renew an entry: one every build finds expires once per time to live, and the next build computes it again,
//! which keeps every lookup free of writes. A pack is rewritten without its expired entries once they hold half of
//! its payload bytes, or when it is merged anyway; until then a lookup still finds them, which serves nothing wrong,
//! since an entry's key names exactly what it was computed from. The merge also removes the temporary files of writers
//! that never published, an hour after they were last written, and the packs of another format version once they are
//! older than both the time to live and an hour.
//!
//! # Examples
//!
//! ```
//! use kartta_cache::{ContentHash, Namespace, Store};
//!
//! # fn main() -> std::io::Result<()> {
//! let dir = tempfile::tempdir()?;
//! let trigrams = Namespace::new("trigrams", b"fold rules 1");
//! let source = b"fn main() {}";
//! let content = ContentHash::of(source);
//!
//! let store = Store::open(dir.path())?;
//! assert_eq!(store.get(&trigrams, &content), None);
//! let writer = store.writer();
//! writer.insert(&trigrams, &content, b"what the producer computed")?;
//! writer.publish()?;
//!
//! // A store opened afterwards, in this process or any other, finds the entry.
//! let store = Store::open(dir.path())?;
//! assert_eq!(store.get(&trigrams, &content), Some(&b"what the producer computed"[..]));
//! # Ok(())
//! # }
//! ```

mod key;
mod pack;
mod store;
mod writer;

pub use crate::key::{ContentHash, ContentHasher, Namespace};
pub use crate::store::{DEFAULT_TTL, Merged, Stats, Store};
pub use crate::writer::PackWriter;
