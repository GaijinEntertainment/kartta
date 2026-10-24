---
paths:
  - "bindings/node/**"
---

<node-binding>

- Run formatting, lint, type checks, and tests through Vite+: `vp fmt`, `vp lint`, `vp run typecheck`, `vp test`.
- Move every dependency to its latest release, pinned in `package-lock.json`. A major bump is in policy: merge it when
  CI passes.

</node-binding>
