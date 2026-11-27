# kartta-cache

Content-addressed cache shared by concurrent processes, stored as immutable mapped packs.

Processes that index nearly identical trees compute the same per-file data over and over: checkouts that differ in a
hundred files out of a hundred thousand share everything else. This crate lets them share it. An entry is keyed by the
`ContentHash` of the bytes it was computed from and by a `Namespace` naming what was computed and by which version of
the code. Nothing in a key names a path or a tree, so identical contents share one entry wherever they sit.

## Packs

A process gathers the entries it computed in a `PackWriter` and publishes them together as one pack: payloads, an index
sorted by key, and a footer. `Store::open` maps every pack in the directory and `Store::get` binary-searches their
indexes, so a lookup opens no file and copies no byte. A store of one file per entry spends its time opening files —
on macOS, three million lookups that way took 47–49s against 0.05s in one mapped index.

## Sharing without coordination

A published pack is never written again. The writer stages it under a temporary name, syncs it, and hard-links it under
a name derived from its index, which creates the name only where none exists, so a pack appears whole or not at all and
is never replaced under a reader. Processes read and publish in one directory with no lock; a store keeps the packs it
mapped for its life and sees later ones when opened again. Every entry carries a 64-bit check over its key and payload,
verified on each lookup, so damage on disk escapes it only with a chance of 2^-64: a damaged entry is passed over and
another pack's sound copy answers, or the lookup misses. The check guards against damage, not against a process that
can write the directory.

The store's directory must allow hard links. Each publish of entries adds a pack, unless the same pack is already in
place, and `Store::merge` keeps their number bounded: past eight it merges every pack but the largest while the largest
holds more entries than all of them together, every pack otherwise, and removes what it merged; a store that mapped a
removed pack keeps answering from it, and one that opens during a merge lists the directory again when a listed pack
has gone. The caller decides when to merge.

## Expiry

Every entry carries the time it was published, and `Store::merge` removes the entries older than the time to live it
is given — `DEFAULT_TTL`, a week, unless the caller picks another. A lookup does not renew an entry, so one that every
build finds expires once per time to live and the next build computes it again, and no lookup ever writes. A pack is
rewritten without its expired entries once they hold half of its payload bytes, or when the merge takes it anyway;
until then they are still found, which serves nothing wrong. The merge also removes the temporary files of writers that
never published, once an hour has passed since they were last written and no writer holds their lock, and the packs of
another format version once they are older than both the time to live and an hour. `Merged` counts what it removed by
age, and what is past its time to live but still in place.

`design-docs/adr/2.1-shared-cache-store.md` records the store's rules and why the
alternatives were rejected, and `design-docs/adr/2.4-cache-pack-merge.md` the merge's;
PRs #26 to #28 and #81 hold what was measured.
