---
paths:
  - "**/CLAUDE.md"
  - "**/CLAUDE.local.md"
  - "**/AGENTS.md"
  - "**/.claude/rules/**/*.md"
  - "**/.claude/instructions/**/*.md"
---

<agent-instructions>

- Invoke the `the-workflow:claude-md` skill before creating, editing, or auditing agent instructions. Read its scaffold
  reference when creating or restructuring a file. The rules below also apply when the skill is unavailable.
- Brief agents assigned to edit instruction files to load the same skill and applicable parent instructions.
- Keep change history out of agent instructions, including `AGENTS.md`, `CLAUDE.md`, and scoped rule files. State the
  rule, not what changed, who requested it, or how it was decided. Put history, postmortems, and session notes in commit
  messages or the issue tracker.
- Keep durable rules that change agent behavior. Do not repeat implementation details that the code already makes clear.
- For each file reference, state what the file covers and when to read it. Keep the detailed guidance in the referenced
  file.
- State each rule once, in short, concrete bullets. Use direct imperatives for instructions, not "you have to" or "you
  should". Keep facts and definitions declarative and in the present tense.
- Read the full file and applicable parent instructions before editing. Verify facts, paths, and commands against the
  repository.
- Update rules in place. Preserve their scope and exceptions when improving wording. Remove stale or duplicate rules.
  Resolve contradictions. Preserve unrelated sections.
- Remove a rule only after verifying that it is stale or redundant. Keep safety-critical, security-sensitive, and
  easy-to-violate rules even when they apply infrequently.
- Diagnose an ignored rule before changing it. Check whether it loaded, survived compaction, or conflicts with another
  rule. Do not repeat it for emphasis.

<scope>

- Place each rule in the narrowest instruction-file scope that includes every file the rule governs. Keep rules needed
  before file access in the repository's root instruction file.
- Put path- or filetype-scoped rules in `.claude/rules/` with `paths`. Keep a rule at the root when a path pattern
  includes unrelated files or excludes files the rule governs.
- Keep package-level instructions limited to rules that differ from or add to the root instructions.
- Put reusable procedures and domain expertise in skills. Put repository-specific procedures in `.claude/instructions/`.
  Give each procedure a read trigger in the root instruction file.
- A language rule states only what this repository adds to the language skill or overrides in it. Do not copy the skill:
  a copy drifts from its source.
- Use hooks and permissions for constraints that require enforcement. Instruction text does not enforce a prohibition.
- Put temporary state and planned work in the issue tracker, not in instruction files.
- Keep capability maps limited to ownership and placement rules. Do not list directories that the agent can discover.
- When a file exceeds 200 lines, classify each block: keep, move to a scoped rule, extract to a skill, or remove. Keep a
  block that remains necessary after this check; 200 lines is not a cap.

</scope>

<loading>

- `CLAUDE.md` loads from the working directory and its parents at launch. A file below the working directory loads when
  a file in its scope is read.
- `.claude/rules/` files with `paths` load when a matching file is read. Rules without `paths` load at launch. Rules
  directories are discovered recursively; nested rules load on demand.
- Load order is not precedence. Reconcile contradictions rather than relying on the last file to override another.
- After compaction, the root `CLAUDE.md` is read again. Package-level files and scoped rules reload when an applicable
  file is read. Instructions given only in conversation do not have that guarantee.
- `@path` imports expand at launch. An import separates files but does not reduce baseline context.
- When diagnosing loading, use `/context` and inspect Memory files.

</loading>

<formatting>

- Use XML tags to delimit instruction blocks. Keep opening and closing tags on separate lines, with blank lines between
  the tags and their content. Use bullets for independent rules inside a block.

</formatting>

</agent-instructions>
