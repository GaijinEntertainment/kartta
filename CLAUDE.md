# kartta

A code graph engine built from public Rust crates: a filesystem walk, a repository file index, and a path scorer. Each
crate is published to crates.io and must stay useful on its own. The target is a tree of three million entries or more,
so cost per entry is the design pressure on every surface.

<performance>

- kartta must be as fast and as memory-efficient as possible. Treat a change that costs time or memory on a large tree
  as a defect unless the change states the trade and why it pays.
- The best code is code that does not exist.
- The fastest code is code that is not executed.
- Memory that is never allocated occupies the least.
- The best change deletes code, because it shrinks what has to be supported later.
- Measure structural work — the walk, the index, refresh, watch, graph storage — on a real tree of three million entries
  or more: on macOS the data volume, `/System/Volumes/Data`; a home directory is closer to two million. A small fixture
  hides costs that only appear at scale: a cost that is 2% of a build on 66k entries can be half of it on 1.4M.
- Measure a change to one language's extraction or resolution on real repositories that hold that language, not on the
  multi-million-entry trees.
- Profile before optimizing. CPU: `samply`, or macOS `sample` for a text call tree. Memory: `dhat` as the global
  allocator of a scratch harness outside the repository, and `/usr/bin/time -l` for peak footprint.
- Decide a design choice by measuring the candidates built, each tuned before the comparison, never by an estimate. A
  fix for one issue builds the candidate its root cause points to first, and a second only when the first misses its
  bar.
- While a call stays under ~500 ms, memory and simplicity outrank speed, and a sub-millisecond difference decides
  nothing. Check each candidate's worst case against 500 ms before ranking it on memory.
- Report every candidate on every measured row in absolute units (ms, MB). A ratio goes beside the values, never in
  place of them.
- Take no time or memory figure inside a Linux container on a macOS host: its filesystem sits in the VM's memory, so the
  figure describes neither Linux nor macOS. Use such a container for clippy and tests only.
- `cargo run --release --example probe -- <path> [query]` indexes a real tree and times the build and a path query.

</performance>

<answers>

- kartta answers LLM agents first. Design every answer, a failure included, to carry the most useful facts the engine
  already holds, so the agent can choose its next call without guessing.
- An answer that leaves something out says so, and says how much wherever the engine can count it without extra work: a
  cap, a depth bound, a collapse or an exclusion reports what it turned away. A partial answer without that reads as a
  complete one.
- A fact that costs extra work per call is a performance trade under `<performance>`; state it as one.
- Judge a change to the graph's edges by coverage and correctness together. Count the edges it adds and removes, which
  is the coverage it moves. Classify a random sample of 30 edges from each side (all, when fewer) against the source: an
  added edge is right when the source holds the relation, a removed one when it does not. Apply the change when about
  90% or more of each side is right, a side with no edges meeting the bar; below that, build two or three other
  implementations of the approximation and take the best, binding at a lower confidence tier where it stays below the
  bar. A false edge costs the agent one read; a missing edge leaves it blind.

</answers>

<capability-map>

- `crates/kartta-walk` — parallel traversal and the tree it discovers. The entry model, ignore semantics, path lookup
  and queries belong to the consumer, through the caller's `Policy`.
- `crates/kartta-fs` — the repository file index: ignore rules, the snapshot, and every query answered from it.
- `crates/kartta-fuzz` — the path scorer.
- `crates/kartta-languages` — every language kartta ships, as one set and as a selection by name. A new language crate
  joins it.
- `crates/kartta` — the engine's entry point. It carries no API.
- `design-docs/adr/` — one decision per file, indexed in `design-docs/ADR.md`. Read the ADR that covers an area before
  changing how that area behaves. The index is imported below on purpose, so every session sees each decision in force
  before it proposes an option an ADR settled.

</capability-map>

<adr-index>
@design-docs/ADR.md
</adr-index>

<constraints>

- Pin every version the build uses: the Rust toolchain, each GitHub Action by its full commit SHA with the tag in a
  comment, and dependencies through the committed `Cargo.lock` under `--locked`. Never depend on `latest`, `stable` or
  another moving tag.
- Do not name or cite a predecessor implementation this code was ported from, in any file, commit message or ADR. State
  the behavior and its reason directly.
- Before reporting work that changes a hot path's cost landscape as done, compare its time and peak memory against
  `main` on the same tree, the one `<performance>` names for that kind of work. The landscape changes with a new stored
  structure or field, a new pass, a step whose complexity changes, or a new allocation or parse per item on the build
  path. Any other change states its added work instead: more records of an existing kind as their count times their
  size, and other work in one line.

</constraints>
