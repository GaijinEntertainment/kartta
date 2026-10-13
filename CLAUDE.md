# kartta

A code graph engine built from public Rust crates: a filesystem walk, a repository file index, and a path scorer. Each
crate is published to crates.io and must stay useful on its own. The workload is a whole home directory — about two
million entries — so cost per entry is the design pressure on every surface.

<performance>

- kartta must be as fast and as memory-efficient as possible. Treat a change that costs time or memory on a large tree
  as a defect unless the change states the trade and why it pays.
- The best code is code that does not exist.
- The fastest code is code that is not executed.
- Memory that is never allocated occupies the least.
- The best change deletes code, because it shrinks what has to be supported later.
- Measure magnitude on a large real tree such as `$HOME`. A small fixture hides costs that only appear at scale: a cost
  that is 2% of a build on 66k entries can be half of it on 1.4M.
- Profile before optimizing. CPU: `samply`, or macOS `sample` for a text call tree. Memory: `dhat` as the global
  allocator of a scratch harness outside the repository, and `/usr/bin/time -l` for peak footprint.
- `cargo run --release --example probe -- <path> [query]` indexes a real tree and times the build and a path query.

</performance>

<capability-map>

- `crates/kartta-walk` — parallel traversal and the tree it discovers. The entry model, ignore semantics, path lookup
  and queries belong to the consumer, through the caller's `Policy`.
- `crates/kartta-fs` — the repository file index: ignore rules, the snapshot, and every query answered from it.
- `crates/kartta-fuzz` — the path scorer.
- `crates/kartta` — the engine's entry point. It carries no API.
- `design-docs/adr/` — one decision per file, indexed in `design-docs/ADR.md`. Read the ADR that covers an area before
  changing how that area behaves.

</capability-map>

<constraints>

- Pin every version the build uses: the Rust toolchain, each GitHub Action by its full commit SHA with the tag in a
  comment, and dependencies through the committed `Cargo.lock` under `--locked`. Never depend on `latest`, `stable` or
  another moving tag.
- Do not name or cite a predecessor implementation this code was ported from, in any file, commit message or ADR. State
  the behavior and its reason directly.
- Before reporting work that touches a hot path as done, compare its time and peak memory against `main` on the same
  large tree.

</constraints>
