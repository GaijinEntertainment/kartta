use rayon::prelude::{ParallelIterator, ParallelSlice};

use crate::graph::{Definition, Graph, narrow};
use crate::resolve::{Call, Cascade};
use crate::stored::{Entry, Site, SymbolAt};

/// Why a call binds no definition the graph holds.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
#[non_exhaustive]
pub enum Miss {
    /// The receiver, or the callee of a bare call that the caller's file does not answer where a package is not a
    /// directory, names an import of a module outside the repository, so the call reaches code the graph does not hold.
    ExternalImport,
    /// The receiver names an import of a directory the graph holds, whose code outside its tests exports no callable of
    /// the name.
    NotExported,
    /// No definition the graph holds has the name as the call writes it: a builtin, or code outside the graph.
    Undefined,
    /// Definitions of the name exist, and the call can bind none of them: a field, a value where calls bind no value,
    /// a definition local to another body, a member of an owner the call does not reach, a member of another class
    /// than the one outside the graph that the receiver names, or, for a call from outside test code, a test definition
    /// that is the one callable of the name.
    NotAdmitted,
    /// Several definitions of the name may be meant, and neither scope nor nearness picks one, or, for a call from
    /// outside test code, nearness picks a test definition.
    Ambiguous,
}

/// The call sites that bound no definition, by reason.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
#[non_exhaustive]
pub struct Misses {
    /// The sites [`Miss::ExternalImport`] names.
    pub external_import: usize,
    /// The sites [`Miss::NotExported`] names.
    pub not_exported: usize,
    /// The sites [`Miss::Undefined`] names.
    pub undefined: usize,
    /// The sites [`Miss::NotAdmitted`] names.
    pub not_admitted: usize,
    /// The sites [`Miss::Ambiguous`] names.
    pub ambiguous: usize,
    /// Of the undefined and the not-admitted sites, those made through any receiver, one that names an import
    /// included: most often a call of a member of a value whose type the graph does not define, a library's.
    pub through_a_receiver: usize,
}

impl Misses {
    const fn add(&mut self, miss: Miss, call: &Call) {
        let count = match miss {
            Miss::ExternalImport => &mut self.external_import,
            Miss::NotExported => &mut self.not_exported,
            Miss::Undefined => &mut self.undefined,
            Miss::NotAdmitted => &mut self.not_admitted,
            Miss::Ambiguous => &mut self.ambiguous,
        };
        *count += 1;
        if matches!(miss, Miss::Undefined | Miss::NotAdmitted) && call.receiver.is_some() {
            self.through_a_receiver += 1;
        }
    }

    const fn merge(&mut self, other: &Self) {
        self.external_import += other.external_import;
        self.not_exported += other.not_exported;
        self.undefined += other.undefined;
        self.not_admitted += other.not_admitted;
        self.ambiguous += other.ambiguous;
        self.through_a_receiver += other.through_a_receiver;
    }
}

/// A call site that bound no definition.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub struct Unbound<'g> {
    /// The definition whose body makes the call.
    pub caller: Definition<'g>,
    /// The one-based line of the call.
    pub line: u32,
    /// The callee as the call writes it.
    pub callee: &'g str,
    /// The receiver as the call writes it, empty for a value the caller declares; `None` for a call with none.
    pub receiver: Option<&'g str>,
    /// Why it bound none.
    pub miss: Miss,
}

/// How a graph's call sites bind, every one of them resolved.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub struct Resolution<'g> {
    /// The call sites resolved: every call and every local call inside a definition.
    pub sites: usize,
    /// The sites that bound a definition, by the [`confidence`](crate::confidence) of the tier that bound them, the
    /// surest first, every confidence no site bound at left out.
    pub bound: Vec<(u8, usize)>,
    /// The pairs of a caller and the definition it calls, each pair once, a declaration counting as the definition it
    /// links to.
    pub edges: usize,
    /// The sites that bound none, by reason.
    pub misses: Misses,
    /// Sites that bound none, in path order and within a file, its calls by name before its local calls, each in the
    /// order of the definitions that make them, at most the query's limit.
    pub unbound: Vec<Unbound<'g>>,
    /// The sites that bound none and that `unbound` does not list.
    pub unlisted: usize,
}

impl Graph {
    /// Resolves every call site the graph holds, and answers how they bind: by the confidence of the tier that bound
    /// each, by the reason each of the others bound none, and how many caller and callee pairs they make. At most
    /// `limit` of the sites that bound none are listed; 0 answers the counts alone.
    ///
    /// The graph resolves calls only when a query asks, so this one costs what resolving every call costs: it runs in
    /// parallel on the rayon pool it is called on, each task resolving a run of files with a memo of its own, and takes
    /// about 0.3 s for 2.7 million call sites on 18 threads. A call or a use made outside every definition is not a
    /// site and is not resolved.
    #[must_use]
    pub fn resolution(&self, limit: usize) -> Resolution<'_> {
        let mut slots: Vec<u32> = self.file_slots().collect();
        if limit > 0 {
            slots.sort_unstable_by_key(|&slot| self.path_rank(slot));
        }
        let runs = rayon::current_num_threads().saturating_mul(RUNS_PER_THREAD);
        let tally = slots
            .par_chunks(slots.len().div_ceil(runs).max(1))
            .map(|run| {
                let mut tally = Tally::new(self, limit);
                tally.files(run);
                tally
            })
            .reduce(|| Tally::new(self, limit), Tally::merge);
        let listed = tally.unbound.len();
        let missed = tally.missed();
        Resolution {
            sites: tally.bound.iter().sum::<usize>() + missed,
            bound: tally
                .bound
                .iter()
                .enumerate()
                .rev()
                .filter(|&(_, &count)| count > 0)
                .map(|(confidence, &count)| (narrow_confidence(confidence), count))
                .collect(),
            edges: tally.edges,
            misses: tally.misses,
            unbound: tally.unbound,
            unlisted: missed - listed,
        }
    }
}

const RUNS_PER_THREAD: usize = 4;

const CONFIDENCES: usize = 101;

struct Tally<'g> {
    graph: &'g Graph,
    limit: usize,
    bound: [usize; CONFIDENCES],
    edges: usize,
    misses: Misses,
    unbound: Vec<Unbound<'g>>,
}

impl<'g> Tally<'g> {
    fn new(graph: &'g Graph, limit: usize) -> Self {
        Self {
            graph,
            limit,
            bound: [0; CONFIDENCES],
            edges: 0,
            misses: Misses::default(),
            unbound: Vec::new(),
        }
    }

    const fn missed(&self) -> usize {
        let misses = &self.misses;
        misses.external_import
            + misses.not_exported
            + misses.undefined
            + misses.not_admitted
            + misses.ambiguous
    }

    fn files(&mut self, run: &[u32]) {
        let cascade = Cascade::new(self.graph);
        let mut pairs = Vec::new();
        for &slot in run {
            self.file(&cascade, slot, &mut pairs);
            pairs.sort_unstable();
            pairs.dedup();
            self.edges += pairs.len();
            pairs.clear();
        }
    }

    fn file(&mut self, cascade: &Cascade<'g>, slot: u32, pairs: &mut Vec<(u32, SymbolAt)>) {
        let file = self.graph.file(slot);
        let calls = (0..file.calls.len()).map(|index| Entry::Call(narrow(index)));
        let local_calls = (0..file.local_calls.len()).map(|index| Entry::LocalCall(narrow(index)));
        for entry in calls.chain(local_calls) {
            let call = cascade.call(Site::new(slot, entry));
            match cascade.resolve_call(&call) {
                Ok((callee, confidence)) => {
                    self.bound[usize::from(confidence)] += 1;
                    pairs.push((call.caller, cascade.canonical(callee)));
                    let twins = cascade.twins(&call, (callee, confidence));
                    pairs.extend(twins.into_iter().map(|twin| (call.caller, twin)));
                }
                Err(miss) => self.miss(miss, &call),
            }
        }
    }

    fn miss(&mut self, miss: Miss, call: &Call) {
        self.misses.add(miss, call);
        if self.unbound.len() < self.limit {
            let strings = &self.graph.strings;
            self.unbound.push(Unbound {
                caller: self.graph.definition_at(call.caller_at()),
                line: call.line,
                callee: strings.resolve(call.callee),
                receiver: call.receiver.map(|receiver| strings.resolve(receiver)),
                miss,
            });
        }
    }

    // constraint: rayon's reduce over an indexed iterator hands each merge the tally of earlier runs first, so the
    // unbound sites a merge keeps are the first in path order
    fn merge(mut self, mut other: Self) -> Self {
        for (held, more) in self.bound.iter_mut().zip(other.bound) {
            *held += more;
        }
        self.edges += other.edges;
        self.misses.merge(&other.misses);
        let room = self.limit - self.unbound.len();
        other.unbound.truncate(room);
        self.unbound.append(&mut other.unbound);
        self
    }
}

#[expect(
    clippy::expect_used,
    reason = "a confidence is at most 100, the size of the tally less one"
)]
fn narrow_confidence(confidence: usize) -> u8 {
    u8::try_from(confidence).expect("a confidence fits a byte")
}
