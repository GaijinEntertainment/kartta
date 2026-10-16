use std::cmp::Ordering;
use std::hash::BuildHasher;

use foldhash::fast::FixedState;
use hashbrown::{HashMap, HashSet};
use rayon::prelude::{IndexedParallelIterator, IntoParallelIterator, ParallelIterator};

use crate::graph::{Definition, Graph, narrow};
use crate::stored::{StoredFile, SymbolAt};
use crate::words::{is_combining_mark, path_words, words};

const MAX_ASKED_WORDS: usize = 8;
const K1: f64 = 1.2;
const B: f64 = 0.75;
// constraint: above every score the words can give, so the exact-name lead stays a lead after hits of several graphs
// are merged by score
const MAX_WORD_SCORE: f64 = 500.0;
const EXACT_NAME_LEAD: f64 = 1000.0;
const PHRASE_LEAD: f64 = 2.0;
const GENERATED_DEMOTION: f64 = 0.5;
const VENDORED_DEMOTION: f64 = 0.75;
const MAX_DEFINITION_WORDS: u32 = u16::MAX as u32;
const FILES_PER_TASK: usize = 64;

/// A definition a search found, and how well it matched.
#[derive(Debug, Clone, Copy, PartialEq)]
#[non_exhaustive]
pub struct Hit<'g> {
    /// The definition.
    pub definition: Definition<'g>,
    /// What the hit ranks by, highest first: its word score, at most 500, halved for generated code and cut by a
    /// quarter for vendored code, and 1000 more when its name is the query. Each graph weighs words by its own
    /// definitions, so of the scores of several language families' graphs only the exact-name lead ranks the same.
    pub score: f64,
    /// Whether its name is the query, as written.
    pub exact: bool,
}

/// What a search found, and what it left out.
#[derive(Debug, Clone, PartialEq)]
#[non_exhaustive]
pub struct Search<'g> {
    /// The best hits, at most the query's limit, best first.
    pub hits: Vec<Hit<'g>>,
    /// The definitions that share a word with the query, or that the query names, before the limit.
    pub total: usize,
    /// Of `total`, those that their name or qualified name matched, rather than only the path of their file.
    pub named: usize,
    /// Of the words the search looked for, the first eight distinct words of the query, those no definition holds,
    /// lowercased as the search split them, in query order.
    pub unmatched_words: Vec<String>,
    /// The distinct words of the query past the first eight, which the search did not look for.
    pub dropped_words: usize,
}

impl Graph {
    /// The definitions whose names and file paths share the most words with `query`, best first, at most `limit` of
    /// them. With `limit` 0 the answer holds no hit, and its counts and its words are those of any other limit.
    ///
    /// A name, and each of the last three segments of its file's path, splits into words at every character that is
    /// not a letter or a digit, between a lowercase letter or a digit and the capital after it, and before the last
    /// capital of a run that a lowercase letter follows: `parseHTTPRequest` holds `parse`, `http` and `request`. Words
    /// are lowercased, and the extension is cut from the file's name alone. A definition holds the words of its
    /// qualified name, those of its own name once more, and those of its file's path. The query splits the same way,
    /// and its first eight distinct words are looked for.
    ///
    /// A definition scores by BM25 over the words it shares with the query, k1 1.2 and b 0.75, times the share it
    /// holds of the query's words that some definition holds, doubled when the query looks for two or more words and
    /// its qualified name holds them all in query order with no word between, capped at 500, halved for generated code
    /// and cut by a quarter for vendored code; one whose name is the query as written scores 1000 more, so it leads
    /// every other hit. Equal scores rank by path and line.
    ///
    /// The search reads the words of every file, in parallel on the rayon pool it is called on, and splits the names
    /// of the definitions only in the files whose words may hold one of the query's: past that scan, cost grows with
    /// how many files hold the query's words. A file's words are gathered by the first search that reads it and kept
    /// until the file is replaced or removed, so building the graph spends nothing on search and the first search
    /// splits every name once. A query with no words skips the scan.
    #[must_use]
    pub fn search(&self, query: &str, limit: usize) -> Search<'_> {
        let asked = Asked::new(query);
        let (scored, holders) = self.scored(query, &asked);

        let unmatched_words = asked
            .words
            .iter()
            .zip(holders)
            .filter(|&(_, holders)| holders == 0)
            .map(|(word, _)| word.clone())
            .collect();
        let total = scored.len();
        let named = scored.iter().filter(|(_, held)| held.named).count();
        Search {
            hits: self.best(scored, limit),
            total,
            named,
            unmatched_words,
            dropped_words: asked.dropped,
        }
    }

    fn scored(
        &self,
        query: &str,
        asked: &Asked,
    ) -> (Vec<(SymbolAt, Scored)>, [usize; MAX_ASKED_WORDS]) {
        let (candidates, word_count) = self.candidates(asked);
        debug_assert!(candidates.is_sorted_by_key(|candidate| candidate.at));

        let mut holders = [0_usize; MAX_ASKED_WORDS];
        for candidate in &candidates {
            for (holder, &count) in holders.iter_mut().zip(&candidate.counts.held) {
                *holder += usize::from(count > 0);
            }
        }
        let definitions = real(self.symbol_count());
        let mean_words = real(word_count) / definitions.max(1.0);
        let weights = holders.map(|holders| {
            if holders == 0 {
                0.0
            } else {
                inverse_frequency(definitions, real(holders))
            }
        });

        let held_anywhere = real(holders.iter().filter(|&&holders| holders > 0).count());
        let mut scored: Vec<(SymbolAt, Scored)> = candidates
            .iter()
            .map(|candidate| {
                let words = f64::from(candidate.counts.words);
                let held = candidate
                    .counts
                    .held
                    .iter()
                    .zip(weights)
                    .filter(|&(&count, _)| count > 0);
                let (score, held): (f64, usize) =
                    held.fold((0.0, 0), |(score, held), (&count, weight)| {
                        (
                            weight.mul_add(saturation(f64::from(count), words, mean_words), score),
                            held + 1,
                        )
                    });
                let phrased = if candidate.phrase { PHRASE_LEAD } else { 1.0 };
                let coordinated = score * real(held) / held_anywhere * phrased;
                let held = Scored {
                    score: self.demoted(candidate.at, coordinated.min(MAX_WORD_SCORE)),
                    named: candidate.named,
                    ..Scored::default()
                };
                (candidate.at, held)
            })
            .collect();
        if let Some(name) = self.strings.get(query) {
            lead_exact(&mut scored, self.named(name));
        }
        (scored, holders)
    }

    // constraint: every file's word set is read, so the scan also sums the words of every definition, the mean a
    // definition's length is weighed against; the reduce keeps the order of the slots, so the candidates come sorted
    // by place
    fn candidates(&self, asked: &Asked) -> (Vec<Candidate>, usize) {
        if asked.words.is_empty() {
            return (Vec::new(), 0);
        }
        (0..self.files.len())
            .into_par_iter()
            .with_min_len(FILES_PER_TASK)
            .fold(Scan::default, |mut scan, slot| {
                self.file_candidates(narrow(slot), asked, &mut scan);
                scan
            })
            .map(|scan| (scan.found, scan.words))
            .reduce(
                || (Vec::new(), 0),
                |(mut left, left_words), (mut right, right_words)| {
                    left.append(&mut right);
                    (left, left_words + right_words)
                },
            )
    }

    // constraint: a file's word set is built by the first search that reads it, so a graph never searched spends
    // nothing on words and the first search splits every file's names once, in parallel
    fn word_set<'f>(&self, file: &'f StoredFile) -> &'f WordSet {
        file.words.get_or_init(|| {
            let names = file.symbols.iter().map(|symbol| {
                (
                    self.strings.resolve(symbol.qualified_name),
                    self.strings.resolve(symbol.name),
                )
            });
            word_set(self.strings.resolve(file.path), names)
        })
    }

    fn file_candidates(&self, slot: u32, asked: &Asked, scan: &mut Scan) {
        let Some(file) = &self.files[slot as usize] else {
            return;
        };
        let held = self.word_set(file);
        scan.words += held.count as usize;
        let Scan { found, word, .. } = scan;
        let holds_one = asked
            .hashes
            .iter()
            .any(|asked| held.hashes.binary_search(asked).is_ok());
        if !holds_one {
            return;
        }
        let path = self.strings.resolve(file.path);
        let path_may_hold = asked.may_hold(path);
        let mut path_counts: Option<Counts> = None;
        for (index, symbol) in file.symbols.iter().enumerate() {
            let name = self.strings.resolve(symbol.name);
            let qualified_name = self.strings.resolve(symbol.qualified_name);
            if !path_may_hold && !asked.may_hold(qualified_name) && !asked.may_hold(name) {
                continue;
            }
            let own = asked
                .count(|emit| words(qualified_name, word, emit))
                .joined(asked.count(|emit| words(name, word, emit)));
            let from_path = *path_counts
                .get_or_insert_with(|| asked.count(|emit| path_words(path, word, emit)));
            let counts = own.joined(from_path);
            if counts.held.iter().any(|&count| count > 0) {
                found.push(Candidate {
                    at: SymbolAt {
                        file: slot,
                        index: narrow(index),
                    },
                    counts,
                    named: own.held.iter().any(|&count| count > 0),
                    phrase: own.phrase,
                });
            }
        }
    }

    fn demoted(&self, at: SymbolAt, score: f64) -> f64 {
        let flags = self.sym(at).flags;
        let mut score = score;
        if flags.is_generated() {
            score *= GENERATED_DEMOTION;
        }
        if flags.is_vendored() {
            score *= VENDORED_DEMOTION;
        }
        score
    }

    fn best(&self, mut scored: Vec<(SymbolAt, Scored)>, limit: usize) -> Vec<Hit<'_>> {
        self.keep_best(&mut scored, limit);
        scored.sort_unstable_by(|left, right| self.ranked(left, right));
        scored
            .into_iter()
            .map(|(at, held)| Hit {
                definition: self.definition_at(at),
                score: held.score,
                exact: held.exact,
            })
            .collect()
    }

    fn keep_best(&self, scored: &mut Vec<(SymbolAt, Scored)>, limit: usize) {
        if limit < scored.len() {
            if let Some(last) = limit.checked_sub(1) {
                scored.select_nth_unstable_by(last, |left, right| self.ranked(left, right));
            }
            scored.truncate(limit);
        }
    }

    fn ranked(&self, left: &(SymbolAt, Scored), right: &(SymbolAt, Scored)) -> Ordering {
        right
            .1
            .score
            .total_cmp(&left.1.score)
            .then_with(|| self.order_of(left.0).cmp(&self.order_of(right.0)))
    }
}

#[derive(Debug, Clone, Copy, Default)]
struct Scored {
    score: f64,
    exact: bool,
    named: bool,
}

/// The words a file holds, and how many its definitions hold.
#[derive(Debug, Default)]
pub(crate) struct WordSet {
    // constraint: the hashes of the file's words, sorted and distinct, so a search tells whether the file may hold a
    // word without splitting its names; a collision only adds a file whose names the search then splits and counts
    hashes: Box<[u32]>,
    // constraint: the words of every definition summed, a definition's own words and its file's counted for each
    count: u32,
}

// constraint: a definition's words are counted as `file_candidates` counts them, its qualified name's then its name's,
// so the mean the length of every definition is weighed against is taken over the same lengths; a file with no
// definition holds no word, since no hit can come from it; a run of letters and digits splits into the same words
// wherever it stands, so each run a file spells is split and hashed once, and an owner its members repeat costs a
// lookup per member
fn word_set<'s>(
    path: &str,
    mut names: impl ExactSizeIterator<Item = (&'s str, &'s str)>,
) -> WordSet {
    if names.len() == 0 {
        return WordSet::default();
    }
    let mut runs = Runs {
        counted: HashMap::with_hasher(FixedState::default()),
        word: String::new(),
        held: Vec::new(),
    };
    let mut path_words_held = 0_u32;
    path_words(path, &mut runs.word, &mut |text| {
        path_words_held += 1;
        runs.held.push(word_hash(text));
    });
    let mut count = 0_u32;
    for (qualified_name, name) in names.by_ref() {
        let own = runs.words_of(qualified_name) + runs.words_of(name);
        count = count.saturating_add((own + path_words_held).min(MAX_DEFINITION_WORDS));
    }
    let mut hashes = runs.held;
    hashes.sort_unstable();
    hashes.dedup();
    WordSet {
        hashes: hashes.into_boxed_slice(),
        count,
    }
}

#[expect(
    clippy::cast_possible_truncation,
    reason = "a word set keeps 32 bits of the hash; a collision only adds a file the search then splits and counts"
)]
fn word_hash(word: &str) -> u32 {
    FixedState::default().hash_one(word) as u32
}

struct Runs<'s> {
    counted: HashMap<&'s str, u32, FixedState>,
    word: String,
    held: Vec<u32>,
}

impl<'s> Runs<'s> {
    fn words_of(&mut self, text: &'s str) -> u32 {
        let mut count = 0;
        for run in text
            .split(|c: char| !c.is_alphanumeric() && !is_combining_mark(c))
            .filter(|run| !run.is_empty())
        {
            if let Some(&counted) = self.counted.get(run) {
                count += counted;
                continue;
            }
            let held = &mut self.held;
            let mut words_in_run = 0;
            words(run, &mut self.word, &mut |text| {
                words_in_run += 1;
                held.push(word_hash(text));
            });
            self.counted.insert(run, words_in_run);
            count += words_in_run;
        }
        count
    }
}

#[derive(Debug, Default)]
struct Scan {
    found: Vec<Candidate>,
    word: String,
    words: usize,
}

#[derive(Debug, Clone, Copy)]
struct Candidate {
    at: SymbolAt,
    counts: Counts,
    named: bool,
    phrase: bool,
}

#[derive(Debug, Clone, Copy, Default)]
struct Counts {
    // constraint: wide enough to count a word every time the longest definition, `MAX_DEFINITION_WORDS` words, holds it
    held: [u16; MAX_ASKED_WORDS],
    words: u32,
    phrase: bool,
}

impl Counts {
    fn joined(self, other: Self) -> Self {
        let mut held = self.held;
        for (count, &more) in held.iter_mut().zip(&other.held) {
            *count = count.saturating_add(more);
        }
        Self {
            held,
            words: (self.words + other.words).min(MAX_DEFINITION_WORDS),
            phrase: self.phrase,
        }
    }
}

struct Asked {
    words: Vec<String>,
    hashes: Vec<u32>,
    dropped: usize,
    ascii: bool,
}

impl Asked {
    fn new(query: &str) -> Self {
        let mut asked: Vec<String> = Vec::new();
        let mut dropped: HashSet<String, FixedState> = HashSet::with_hasher(FixedState::default());
        words(query, &mut String::new(), &mut |word| {
            if asked.iter().any(|held| held == word) || dropped.contains(word) {
                return;
            }
            if asked.len() < MAX_ASKED_WORDS {
                asked.push(word.to_owned());
            } else {
                dropped.insert(word.to_owned());
            }
        });
        Self {
            hashes: asked.iter().map(|word| word_hash(word)).collect(),
            ascii: asked.iter().all(|word| word.is_ascii()),
            dropped: dropped.len(),
            words: asked,
        }
    }

    // constraint: a cheap test that never turns away a text holding an asked word, so it may only skip splitting; a
    // text outside ASCII can lowercase to an ASCII word, the Kelvin sign to `k`, so only an ASCII text is tested
    fn may_hold(&self, text: &str) -> bool {
        !self.ascii || !text.is_ascii() || self.words.iter().any(|word| holds_ascii(text, word))
    }

    // constraint: a phrase is every asked word, two or more, in query order with no other word between them
    fn count(&self, split: impl FnOnce(&mut dyn FnMut(&str))) -> Counts {
        let mut counts = Counts::default();
        let mut run = 0_usize;
        split(&mut |word| {
            counts.words += 1;
            let asked = counts.held.iter_mut().zip(&self.words);
            for (count, _) in asked.filter(|(_, asked)| *asked == word) {
                *count = count.saturating_add(1);
            }
            let continues = |at: usize| self.words.get(at).is_some_and(|asked| asked == word);
            run = if continues(run) {
                run + 1
            } else {
                usize::from(continues(0))
            };
            counts.phrase |= run == self.words.len() && run > 1;
        });
        counts.words = counts.words.min(MAX_DEFINITION_WORDS);
        counts
    }
}

// constraint: a name that is the query holds every word the query asks, so its definitions are word matches already
// unless the query holds no word, and those are added
fn lead_exact(scored: &mut Vec<(SymbolAt, Scored)>, exact: &[SymbolAt]) {
    let matched = scored.len();
    for &at in exact {
        let found = scored[..matched]
            .binary_search_by_key(&at, |&(at, _)| at)
            .unwrap_or_else(|_| {
                scored.push((at, Scored::default()));
                scored.len() - 1
            });
        let held = &mut scored[found].1;
        held.score += EXACT_NAME_LEAD;
        held.exact = true;
        held.named = true;
    }
}

fn holds_ascii(text: &str, word: &str) -> bool {
    let (text, word) = (text.as_bytes(), word.as_bytes());
    text.len() >= word.len()
        && text
            .windows(word.len())
            .any(|window| window.eq_ignore_ascii_case(word))
}

fn inverse_frequency(definitions: f64, holders: f64) -> f64 {
    ((definitions - holders + 0.5) / (holders + 0.5)).ln_1p()
}

fn saturation(count: f64, words: f64, mean_words: f64) -> f64 {
    (count * (K1 + 1.0)) / K1.mul_add(B.mul_add(words / mean_words, 1.0 - B), count)
}

#[expect(
    clippy::cast_precision_loss,
    reason = "counts of definitions and of their words stay far below 2^52, past which an f64 drops units"
)]
const fn real(count: usize) -> f64 {
    count as f64
}
