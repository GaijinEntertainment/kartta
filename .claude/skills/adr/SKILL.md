---
name: adr
description: >-
  Writes and amends kartta's architecture decision records in design-docs/adr and their index entries. Use when a
  change alters a rule an ADR states, a new decision needs a record, or an ADR is reviewed or reshaped.
paths:
  - "design-docs/**"
---

An ADR here is a living record. It states the decision as it holds now, why it holds, what it costs, what lost, and
what would reopen it. Its main readers are agents about to change the code it governs, so the current rule leads and
nothing replaced sits above it. The evidence behind a change lives in its PR, and the replaced wording lives in git.

## Shape

A new ADR starts from `${CLAUDE_SKILL_DIR}/assets/adr-template.md`. Every ADR holds these parts, in this order:

- **Header** — `Status` (accepted, or superseded by a linked ADR), `Date` (when accepted), `Updated` (the newest
  History date), `Initiative`, `Governs` (the crates, modules or public items the rule binds, as repository paths), and
  optionally `Pinned by` (the tests that fail when the rule breaks).
- **Summary** — one to three sentences: the context, the choice, and the trade it accepts.
- **Decision** — the current rule, in full sentences, each with its scope.
- **Why** — the forces behind the choice. At most one deciding figure per fact, with the tree and the date it was
  measured on. A sampled result is a share with its sample size: "72% of 57 sampled".
- **Consequences** — what the rule costs and what it gets wrong, one magnitude each, with the issue that tracks it, or
  "accepted" where no issue will.
- **Rejected** — one line per option a reader could propose again, with the fact that decided against it.
- **Revisit if** — the assumption whose change reopens the decision, and the confidence held in it. It fits the index's
  one line; a trigger that does not fit belongs to a second decision.
- **History** — newest first, one line per rule change: `- YYYY-MM-DD — what changed and why (#issue)`.

The file is `<number>-<the title in kebab case>.md`. A new ADR takes the next free number of its group on `main`, and a
branch that lands after another took that number renumbers on rebase.

## Amending

1. A change to a rule rewrites the Summary, Decision, Why, Consequences, Rejected and Revisit if so that the file reads
   as the current rule, sets `Updated`, and adds one History line. A change that alters no rule adds no History line.
2. Delete the replaced wording; git holds it. A replaced rule that a reader could propose again becomes a Rejected line,
   with the fact that decided against it.
3. The History line cites the issue the change resolves; a change without one gets an issue filed before the ADR
   commit. The PR does not exist when the ADR commit is made, and `gh issue view <n> --json
   closedByPullRequestsReferences` leads from the issue to the PR that holds the evidence.
4. A crate README or doc comment that states the same behavior as Decision is checked in the same commit, and the wrong
   text is fixed there.
5. Split off a sub-rule whose driver the parent's Why does not give and which has its own Rejected options. Sub-rules
   that share one driver stay together, and a rule whose reversal would meet the parent's Revisit if stays in the
   parent. The parent's Decision links the split ADR's file by relative path. The split ADR keeps the Date and the
   History lines of the rules it takes, and the parent keeps a History line only where the same change also altered a
   rule the parent states.
6. A reversal of the core decision is a new ADR that stands alone. The old ADR is marked `superseded by` and frozen,
   and the two link each other.
7. A change to an ADR's title, status, Governs or Revisit if updates its index entry in the same commit.

## Never in an ADR

- A `Revised <date>` block, a "(revised below)" marker, or a sentence that sets the old rule against the new one.
- Tables of deltas by tree, language or tier; tallies of samples beyond the one deciding figure; parity counts; cost
  runs with ranges or machine load; an account of how the variants were built and measured. These belong to the PR
  body.
- A comparison with the engine this code was ported from. CLAUDE.md bars citing it.
- A restatement of another ADR's rule. Link that ADR.

## Size

Aim for 65 lines or fewer. At 100 lines, look for a second decision to split off. Past 150 lines, split or cut before
committing.

## The index

`design-docs/ADR.md` lists every ADR under its status, in number order. An entry links the number and title, then
gives the Governs paths and the Revisit if trigger, one line each.
