# Releasing kartta

One version `X.Y.Z` goes out once, in lockstep, on every channel
([ADR 4.46](/design-docs/adr/4.46-release-scope.md)): the crates on crates.io, the npm packages, the Go module
`dev.gaijin.team/go/kartta`, and the GitHub release with the static libraries. The tag `vX.Y.Z` runs
`.github/workflows/release.yml`, which builds every target and then publishes along two branches that fail apart
([ADR 4.55](/design-docs/adr/4.55-release-workflow.md), [ADR 4.65](/design-docs/adr/4.65-npm-publish.md)):

- `build` → `checksums` → `crates` → `publish`, where `crates` uploads the crates, and `publish` pushes the Go tag
  `bindings/go/vX.Y.Z` and creates the GitHub release.
- `build` → `npm-pack` → `npm-publish`, where `npm-publish` uploads the seven npm packages, `kartta` last.

## What cannot be undone

- **Tags.** The ruleset `protect-release-tags` refuses to delete, move or update a `v*` or `bindings/go/v*` tag, and
  has no bypass actor. A tag names its commit for good.
- **crates.io.** A version can never be overwritten, and its code cannot be deleted. A yank stops new resolutions from
  picking it, and a `Cargo.lock` that holds it still builds (Cargo's `publishing.md` and `cargo-yank.md`).
- **npm.** A name and version can never be reused, even after an unpublish (npm's `npm-unpublish.md`). A deprecation
  warns whoever installs it (`npm-deprecate.md`).
- **Go.** A version is an immutable snapshot, and a proxy keeps serving it. A later version's `go.mod` retracts it
  (the Go modules reference, "retract directive").
- **Re-runs.** A re-run builds the same commit and ref as the run it repeats, up to 30 days after it (GitHub's
  "Re-running workflows and jobs"). A re-run of failed jobs also re-runs the jobs that depend on them.

So a re-run can finish a cut whose tree is right, and nothing can change what `X.Y.Z` holds. **A fix that needs a new
commit is a lockstep `X.Y.(Z+1)` of every artifact**, cut from the start below, whatever channels `X.Y.Z` reached.

## Cut a release

1. On a branch, run `scripts/version.sh X.Y.Z`. Its pull request runs the release workflow's builds and both publishes
   as dry runs. Merge it once they pass.
2. Tag the merge commit on `main` and push the tag: `git tag vX.Y.Z <commit>`, then `git push origin vX.Y.Z`.
3. Approve the deployments to the `crates-io` and `npm` environments when the run asks for them.
4. Once both branches finish, run the install checks from the registries on every target:
   `gh workflow run verify-release.yml --ref vX.Y.Z -f version=X.Y.Z`. A check that fails on what the release published
   is a defect in it, below.

## When a cut fails

Read which jobs failed in the run. The two branches fail apart, so take the section of each branch that stopped: a run
can need both. "Re-run" means **Re-run failed jobs** on that run, or `gh run rerun <run-id> --failed`.

### Nothing uploaded: `versions` or `build` failed

Every other job waits on these, so no registry and no release holds `X.Y.Z`. A runner or network fault: re-run. A
defect in the tree: fix it on `main` and cut `X.Y.(Z+1)`. The tag `vX.Y.Z` stays on a commit no channel ships.

### The crates are not out: `checksums` or `crates` failed

`crates` waits on `checksums`, and `publish` on `crates`, so the Go tag and the GitHub release are not out either.

- **`checksums` failed**: nothing of this branch uploaded. Re-run.
- **Before any upload**, for want of a token, or an approval refused: set the `crates-io` environment's secret
  `CARGO_REGISTRY_TOKEN` (#1058), then re-run. A crate that fails to package or verify is a defect in the tree:
  `X.Y.(Z+1)`.
- **Midway**, some crates uploaded: re-run. `scripts/publish-crates.sh` leaves out each crate whose version the index
  lists. A re-run that comes before the index lists the last upload tries that crate again and crates.io refuses it;
  re-run again later.
- **Nothing more can upload** from this tree: cut `X.Y.(Z+1)`. The crates already at `X.Y.Z` stay, and they build,
  because each one uploaded after the crates it depends on.

### The crates are out, the GitHub release is not: `publish` failed

Re-run. A push of `bindings/go/vX.Y.Z` to the commit it already names changes nothing, and the ruleset allows creating
the tag. Until the release exists, the Go module resolves and links against no archive.

`gh release create` makes a draft, uploads each asset, then publishes it, in separate requests (`gh release create
--help`). When a failed upload left a draft, delete it before the re-run: `gh release delete vX.Y.Z` deletes the
release and keeps the tag.

### npm is not out: `npm-pack` or `npm-publish` failed

npm resolves no `kartta@X.Y.Z` until `kartta` itself is out. What the crates branch published serves its consumers
meanwhile.

- **`npm-pack` failed**: nothing of this branch uploaded. A runner or network fault: re-run. A defect in the tree:
  `X.Y.(Z+1)`.
- **Before any upload**, for want of a token, or an approval refused: set the `npm` environment's secret `NPM_TOKEN`
  (#1058), then re-run. The re-run job references the environment, so it waits for an approval as the first did
  (GitHub's "Deployments and environments", "Required reviewers").
- **Midway**, some target packages uploaded and `kartta` not: re-run. `scripts/npm-publish.sh` skips each version the
  registry holds and publishes `kartta` last, so no install of `kartta` reaches a missing target's package.
- **Nothing more can upload** from this tree, or the 30 days of re-runs are over: cut `X.Y.(Z+1)`. The other channels'
  `X.Y.Z` stay as they are, and so do the target packages already at `X.Y.Z`, which no `kartta` reaches.

### A defect in a published release

Fix it on `main` and cut `X.Y.(Z+1)`, with these in the same release:

- `retract vX.Y.Z // <the defect>` in `bindings/go/go.mod`, so `go get` passes over `X.Y.Z`.

Once `X.Y.(Z+1)` is out:

- Yank each crate the defect reaches: `cargo yank --version X.Y.Z <crate>`. A lockfile that holds it still builds.
- Deprecate each npm package the defect reaches: `npm deprecate <package>@X.Y.Z "<the defect>; use X.Y.(Z+1)"`.
- Edit the `vX.Y.Z` release's notes to name the defect and the version that fixes it.

A version without the defect stays as it is: a yank or a deprecation is for the consumers of a defect.

## The settings a cut relies on

The maintainer sets these when the repository goes public (#144), before the first tag, and reads them back before each
cut.

- [ ] Secret scanning and its push protection are on:
      `gh api repos/GaijinEntertainment/kartta --jq .security_and_analysis`.
- [ ] Dependabot alerts are on: `gh api repos/GaijinEntertainment/kartta/vulnerability-alerts` succeeds, and fails
      with 404 while they are off (GitHub's REST "Check if vulnerability alerts are enabled for a repository").
- [ ] Dependabot security updates are on:
      `gh api repos/GaijinEntertainment/kartta --jq .security_and_analysis.dependabot_security_updates`.
- [ ] The default `GITHUB_TOKEN` permission is read, under Settings → Actions → General → Workflow permissions:
      `gh api repos/GaijinEntertainment/kartta/actions/permissions/workflow --jq .default_workflow_permissions`.
- [ ] Every action must be pinned to a full commit SHA, under Settings → Actions → General:
      `gh api repos/GaijinEntertainment/kartta/actions/permissions --jq .sha_pinning_required`.
- [ ] The description and the homepage are set:
      `gh api repos/GaijinEntertainment/kartta --jq '.description, .homepage'`.
- [ ] The `crates-io` and `npm` environments exist, each with a required reviewer and its secret (#1058):
      `gh api repos/GaijinEntertainment/kartta/environments --jq '.environments[] | {name, protection_rules}'`, then
      `gh secret list --env crates-io` names `CARGO_REGISTRY_TOKEN` and `gh secret list --env npm` names `NPM_TOKEN`,
      without their values (GitHub's REST "List environment secrets").
- [ ] The ruleset `protect-release-tags` stands as "What cannot be undone" relies on: active on `refs/tags/v*` and
      `refs/tags/bindings/go/v*`, with the rules `deletion`, `non_fast_forward` and `update` and no bypass actor.
      `gh api repos/GaijinEntertainment/kartta/rulesets --jq '.[] | select(.name == "protect-release-tags") | .id'`
      names its id, then `gh api repos/GaijinEntertainment/kartta/rulesets/<id>
      --jq '{enforcement, bypass_actors, include: .conditions.ref_name.include, rules: [.rules[].type]}'` reads it.
