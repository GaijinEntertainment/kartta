use std::hash::BuildHasher;
use std::num::NonZeroU32;

use foldhash::fast::FixedState;
use hashbrown::HashTable;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub(crate) struct Str(NonZeroU32);

impl Str {
    const fn slot(self) -> usize {
        self.0.get() as usize - 1
    }
}

// shortcut: a string is never removed, so a graph refreshed many times keeps the names of files it no longer holds —
// compact the arena once a refresh can count how much of it is unreferenced
#[derive(Debug, Default)]
pub(crate) struct Strings {
    bytes: String,
    spans: Vec<(u32, u32)>,
    table: HashTable<Str>,
    state: FixedState,
}

impl Strings {
    pub(crate) fn intern(&mut self, text: &str) -> Str {
        let hash = self.state.hash_one(text);
        let Self {
            bytes,
            spans,
            table,
            state,
        } = self;
        let resolve = |id: &Str| span_text(bytes, spans, *id);

        if let Some(&id) = table.find(hash, |id| resolve(id) == text) {
            return id;
        }

        let start = narrow(bytes.len());
        bytes.push_str(text);
        spans.push((start, narrow(bytes.len())));
        let id = Str(NonZeroU32::MIN.saturating_add(narrow(spans.len() - 1)));
        table.insert_unique(hash, id, |id| state.hash_one(span_text(bytes, spans, *id)));
        id
    }

    pub(crate) fn get(&self, text: &str) -> Option<Str> {
        let hash = self.state.hash_one(text);
        self.table
            .find(hash, |&id| self.resolve(id) == text)
            .copied()
    }

    pub(crate) fn resolve(&self, id: Str) -> &str {
        span_text(&self.bytes, &self.spans, id)
    }
}

fn span_text<'a>(bytes: &'a str, spans: &[(u32, u32)], id: Str) -> &'a str {
    let (start, end) = spans[id.slot()];
    &bytes[start as usize..end as usize]
}

#[expect(
    clippy::expect_used,
    reason = "a graph holds fewer than 4 GiB of distinct names, far past what any tree's definitions spell"
)]
fn narrow(len: usize) -> u32 {
    u32::try_from(len).expect("fewer than 4 GiB of interned text")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_string_interned_twice_is_one_string() {
        let mut strings = Strings::default();

        let first = strings.intern("parse");
        let other = strings.intern("render");
        let again = strings.intern("parse");

        assert_eq!(first, again);
        assert_ne!(first, other);
        assert_eq!(strings.resolve(other), "render");
        assert_eq!(strings.get("parse"), Some(first));
        assert_eq!(strings.get("absent"), None);
    }
}
