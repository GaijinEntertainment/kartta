use foldhash::fast::FixedState;
use hashbrown::HashSet;

use crate::graph::{Definition, Graph, tails};
use crate::stored::SymbolAt;

/// The most characters a name [`each_one_edit`] edits holds: past it, a lookup's cost, in the square of the length,
/// buys nothing, the longest qualified name of 2.3 million definitions on the trees measured being 189 characters.
const LONGEST: usize = 256;

impl Graph {
    /// The definitions the names one edit from `name` select, as [`Graph::definitions`] reads a name, each once: those
    /// outside test code first, then those outside vendored code, then by path, line and qualified name.
    ///
    /// An edit swaps a character with the next, drops one, or replaces or adds one, a replacing or added character
    /// being an ASCII letter, a digit, `_` or one `name` holds: for each of its characters about twice as many names as
    /// that set holds characters, 128 for a name of those alone, and one more pair for each other character it holds.
    /// Each is looked up as a name but the empty one, which no definition a typo was made of bears. A name is built
    /// whole, and costs a probe of the graph's strings for itself and one for each part of it after a separator, so a
    /// lookup costs in proportion to the square of the length of `name`, and a `name` of more than 256 characters, far
    /// past the qualified names of real code, finds none and costs nothing. A qualified `name` keeps its last segment
    /// through an edit to what qualifies it, and the definitions of that segment are indexed once by the qualified
    /// names they end with, so a segment many definitions share costs each of them once, not once for each name.
    ///
    /// # Examples
    ///
    /// ```
    /// use kartta_graph::Graph;
    /// use kartta_lang::{Conventions, Extraction, Symbol, SymbolKind};
    ///
    /// let mut walk = Extraction::default();
    /// walk.file = "walk.go".to_owned();
    /// walk.symbols.push(Symbol::new("handle", SymbolKind::Function, 3, 9));
    ///
    /// let mut graph = Graph::new(Conventions::default());
    /// graph.insert(&walk);
    ///
    /// let near = graph.definitions_one_edit_from("hadnle");
    /// assert_eq!(near.len(), 1);
    /// assert_eq!(near[0].name, "handle");
    /// ```
    #[must_use]
    pub fn definitions_one_edit_from(&self, name: &str) -> Vec<Definition<'_>> {
        let last = tails(name).next();
        let mut segment: Option<Segment<'_>> = None;
        let mut found: HashSet<SymbolAt, FixedState> = HashSet::default();
        each_one_edit(name, |edited| {
            let mut tails = tails(edited);
            match tails.next() {
                Some(tail) if Some(tail) == last => {
                    let segment = segment.get_or_insert_with(|| Segment::of(self, tail));
                    found.extend(self.selected_with(edited, segment.selected(edited), tails));
                }
                _ => found.extend(self.selected(edited)),
            }
        });

        let mut found: Vec<(SymbolAt, Definition<'_>)> = found
            .into_iter()
            .map(|at| (at, self.definition_at(at)))
            .collect();
        found.sort_unstable_by_key(|&(at, definition)| {
            let flags = definition.flags;
            let place = (definition.file, definition.line, definition.qualified_name);
            (flags.is_test(), flags.is_vendored(), place, at)
        });
        found
            .into_iter()
            .map(|(_, definition)| definition)
            .collect()
    }
}

/// The definitions of one last segment, by the qualified names a name that ends with that segment selects them by,
/// sorted by that name.
///
/// A name selects a definition whose qualified name is the name or ends with it at a separator, and one whose
/// qualified name, not the segment alone, the name ends with at a separator: each definition is held under its
/// qualified name, marked whole, and under each part of it after a separator. A name longer than the segment selects
/// by none of the shorter ones, so those are left out.
struct Segment<'g> {
    by_ending: Vec<(&'g str, bool, SymbolAt)>,
}

impl<'g> Segment<'g> {
    fn of(graph: &'g Graph, segment: &str) -> Self {
        let mut by_ending = Vec::new();
        for &at in graph.named_as(segment) {
            let own = graph.strings.resolve(graph.sym(at).qualified_name);
            if own.len() > segment.len() {
                by_ending.push((own, true, at));
            }
            by_ending.extend(
                tails(own)
                    .filter(|ending| ending.len() > segment.len())
                    .map(|ending| (ending, false, at)),
            );
        }
        by_ending.sort_unstable();
        Self { by_ending }
    }

    /// Those of the definitions `name`, a qualified name that ends with the segment, selects.
    fn selected<'s>(&'s self, name: &'s str) -> impl Iterator<Item = SymbolAt> + 's {
        let ending_with = self.held(name).map(|&(_, _, at)| at);
        let ended_by = tails(name).skip(1).flat_map(move |ending| {
            self.held(ending)
                .filter(|&&(_, whole, _)| whole)
                .map(|&(_, _, at)| at)
        });
        ending_with.chain(ended_by)
    }

    fn held<'s>(
        &'s self,
        ending: &'s str,
    ) -> impl Iterator<Item = &'s (&'g str, bool, SymbolAt)> + 's {
        let start = self
            .by_ending
            .partition_point(|&(held, _, _)| held < ending);
        self.by_ending[start..]
            .iter()
            .take_while(move |&&(held, _, _)| held == ending)
    }
}

/// Calls `each` once with every string one edit from `name`, as [`Graph::definitions_one_edit_from`] edits a name.
///
/// An edit swaps a character with the next, drops one, or replaces or adds one, a replacing or added character being an
/// ASCII letter, a digit, `_` or one `name` holds. A character dropped from a run of it, or added beside its like, is
/// dropped or added at the run's end alone, so no string comes twice, and none is `name` itself or the empty one. A
/// `name` of more than 256 characters gets no call.
///
/// # Examples
///
/// ```
/// let mut edited = Vec::new();
/// kartta_graph::each_one_edit("mian.rs", |near| edited.push(near.to_owned()));
/// assert!(edited.iter().any(|near| near == "main.rs"));
/// ```
pub fn each_one_edit(name: &str, mut each: impl FnMut(&str)) {
    if name.chars().nth(LONGEST).is_some() {
        return;
    }
    let chars: Vec<char> = name.chars().collect();
    let mut added: Vec<char> = ('a'..='z')
        .chain('A'..='Z')
        .chain('0'..='9')
        .chain(['_'])
        .chain(chars.iter().copied())
        .collect();
    added.sort_unstable();
    added.dedup();
    let mut edited = String::with_capacity(name.len() + 4);
    let mut make = |head: &[char], middle: &[char], tail: &[char]| {
        edited.clear();
        edited.extend(head.iter().chain(middle).chain(tail));
        if !edited.is_empty() {
            each(&edited);
        }
    };
    for at in 0..chars.len() {
        if chars.get(at + 1) != Some(&chars[at]) {
            make(&chars[..at], &[], &chars[at + 1..]);
        }
    }
    for at in 1..chars.len() {
        if chars[at - 1] != chars[at] {
            make(
                &chars[..at - 1],
                &[chars[at], chars[at - 1]],
                &chars[at + 1..],
            );
        }
    }
    for (at, &held) in chars.iter().enumerate() {
        for &replacing in added.iter().filter(|&&replacing| replacing != held) {
            make(&chars[..at], &[replacing], &chars[at + 1..]);
        }
    }
    for at in 0..=chars.len() {
        for &adding in added
            .iter()
            .filter(|&&adding| chars.get(at) != Some(&adding))
        {
            make(&chars[..at], &[adding], &chars[at..]);
        }
    }
}

#[cfg(test)]
mod tests {
    use kartta_lang::{Conventions, Extraction, Symbol, SymbolKind};

    use super::*;

    /// Every string one edit from `name`, built edit by edit with no shortcut, the name itself and the empty one aside.
    fn one_edit_by_hand(name: &str) -> HashSet<String, FixedState> {
        let chars: Vec<char> = name.chars().collect();
        let added: Vec<char> = ('a'..='z')
            .chain('A'..='Z')
            .chain('0'..='9')
            .chain(['_'])
            .chain(chars.iter().copied())
            .collect();
        let mut edited: HashSet<String, FixedState> = HashSet::default();
        for at in 0..=chars.len() {
            for &other in &added {
                let mut adding = chars.clone();
                adding.insert(at, other);
                edited.insert(adding.into_iter().collect());
            }
            if at == chars.len() {
                break;
            }
            let mut dropping = chars.clone();
            dropping.remove(at);
            edited.insert(dropping.into_iter().collect());
            for &other in &added {
                let mut replacing = chars.clone();
                replacing[at] = other;
                edited.insert(replacing.into_iter().collect());
            }
            if at + 1 < chars.len() {
                let mut swapping = chars.clone();
                swapping.swap(at, at + 1);
                edited.insert(swapping.into_iter().collect());
            }
        }
        edited.remove(name);
        edited.remove("");
        edited
    }

    #[test]
    fn every_string_one_edit_away_comes_once_and_the_empty_one_never() {
        for name in [
            "a",
            "aa",
            "aab",
            "balloon",
            "Kartta.#handel",
            "ns::Owner::parse",
            "café",
        ] {
            let mut edited = Vec::new();
            each_one_edit(name, |text| edited.push(text.to_owned()));

            let distinct: HashSet<String, FixedState> = edited.iter().cloned().collect();
            assert_eq!(distinct.len(), edited.len(), "for {name}");
            assert_eq!(distinct, one_edit_by_hand(name), "for {name}");
        }
    }

    #[test]
    fn the_indexed_segment_selects_what_each_name_one_edit_away_selects_alone() {
        let mut parse = Extraction::default();
        parse.file = "parse.go".to_owned();
        let qualified = [
            "parse",
            "Owner.parse",
            "Other.parse",
            "Ownr.parse",
            "x.Owner.parse",
            "ns::Owner::parse",
            "ns:Owner::parse",
            "a/b.parse",
            "Kartta.#parse",
            "Owner.parse.parse",
        ];
        for (line, qualified) in (1..).zip(qualified) {
            let mut symbol = Symbol::new("parse", SymbolKind::Function, line, line);
            qualified.clone_into(&mut symbol.qualified_name);
            parse.symbols.push(symbol);
        }
        for (line, named) in [(20, "Owner.parse"), (21, "Lone.parse")] {
            let mut owner = Symbol::new(named, SymbolKind::Method, line, line);
            named.clone_into(&mut owner.qualified_name);
            parse.symbols.push(owner);
        }
        let mut graph = Graph::new(Conventions::default());
        graph.insert(&parse);

        for name in [
            "Ownre.parse",
            "Owner.parse",
            "x.Ownre.parse",
            "x.Owne.parse",
            "Lnoe.parse",
            "x.Lnoe.parse",
            "ns::Ownre::parse",
            "ns:Owner:parse",
            "a/bparse",
            "a/b.parse",
            "Kartta.#parse",
            "Kartta#parse",
            "Owner.parse.prase",
            ".parse",
            "Owner.",
            "#parse",
        ] {
            let mut alone: HashSet<SymbolAt, FixedState> = HashSet::default();
            each_one_edit(name, |edited| alone.extend(graph.selected(edited)));
            let mut alone: Vec<Definition<'_>> = alone
                .into_iter()
                .map(|at| graph.definition_at(at))
                .collect();
            alone.sort_unstable_by_key(|definition| (definition.line, definition.qualified_name));

            let mut indexed = graph.definitions_one_edit_from(name);
            indexed.sort_unstable_by_key(|definition| (definition.line, definition.qualified_name));

            assert_eq!(indexed, alone, "for {name}");
        }
        let named = |name: &str| -> Vec<(&str, &str)> {
            let found = graph.definitions_one_edit_from(name);
            found
                .iter()
                .map(|definition| (definition.name, definition.qualified_name))
                .collect()
        };
        assert!(
            named("Lnoe.parse").contains(&("Lone.parse", "Lone.parse")),
            "a name a separator holds"
        );
        assert!(
            named("x.Owne.parse").contains(&("parse", "Owner.parse")),
            "a qualified name it ends with"
        );
    }
}
