---
paths:
  - "**/*.md"
---

<markdown>

- Format and lint Markdown with rumdl alone: after editing a file, run `rumdl fmt <file>`, then `rumdl check <file>`.
  No other formatter runs on Markdown. The settings are in `.rumdl.toml`.
- Install the rumdl version `.github/workflows/ci.yml` pins: `cargo install --locked rumdl@<version>`.
- If `rumdl fmt` changes lines outside the edit, commit that reflow alone, before the content edit.

</markdown>
