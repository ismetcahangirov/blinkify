# GitHub workflow

How work moves from an issue to `main`. The binding rules are in
[`../../CLAUDE.md`](../../CLAUDE.md) sections 5 and 6; this document is the
practical version.

## The shape of the work

```
Epic #1  ─── sub-issue #9   ─── branch docs/9-…    ─── PR ─── squash ─── main
         ├── sub-issue #10  ─── branch feat/10-…   ─── PR ─── squash ─── main
         └── sub-issue #11  ─── branch chore/11-…  ─── PR ─── squash ─── main
                      │
                      └─ Epic #1 closes when its sub-issues do
```

**One issue per branch, one branch per issue, one pull request per branch.** A
pull request that closes two issues is two pull requests.

Epics never receive code. They are containers, linked to their sub-issues using
GitHub's sub-issue relationship so the Epic shows real progress rather than a
checklist someone has to tick by hand.

## Branch naming

```
<type>/<issue-number>-<short-slug>
```

The type matches the Conventional Commit type of the work:

| Branch                                | Issue                                       |
| ------------------------------------- | ------------------------------------------- |
| `feat/40-keyframe-aligned-cut`        | Implement keyframe-aligned lossless cutting |
| `fix/112-vfr-drift-on-import`         | Timestamps drift on VFR phone footage       |
| `chore/12-issue-templates-and-labels` | Add the issue templates and labels          |
| `docs/9-engineering-rulebook`         | Write the rulebook and docs skeleton        |

The issue number is in the branch name so that six weeks later a stale branch
can be traced to its context without archaeology.

## Commits

[Conventional Commits](https://www.conventionalcommits.org/). Scope is the area
label where one applies.

```
feat(export): plan a smart-cut when a cut point is not keyframe-aligned
fix(engine): prefer stream-side rotation over container rotation
docs: record the encoder strategy as ADR-0003
chore(ci): cache the Cargo registry keyed on Cargo.lock
```

Types: `feat`, `fix`, `docs`, `chore`, `refactor`, `test`, `perf`, `build`,
`ci`.

**The body says why, not what.** The diff already says what. A good body
answers: what forced this, what else was tried, what a reader would otherwise
assume was a mistake.

Breaking changes to the IPC contract carry `!` after the scope and a
`BREAKING CHANGE:` footer explaining what a consumer must now do differently.

## Pull requests

- **`Closes #N`** on the first line, so the issue closes on merge rather than
  being forgotten open.
- **Fill in the Definition of Done honestly.** An unchecked box with a sentence
  explaining why is fine — and is often the most useful thing in the PR. A
  checked box that is not true is not fine.
- **Say how it was verified**, with the commands and what they printed. "Tested
  locally" is not evidence.
- **Squash merge.** `main` keeps a linear history, one commit per issue.
- **Delete the branch on merge.**

### Stacked pull requests

Where an issue is blocked by another that is still in review, branch from the
blocking branch and set the pull request's base to it. State the dependency in
the PR body. When the base merges, GitHub retargets the stacked PR to `main`
automatically.

Use this sparingly. A stack three deep is a sign the issues were sliced wrong.

## Branch protection on `main`

Configured in [#13](https://github.com/ismetcahangirov/blinkify/issues/13)
alongside the CI pipeline:

- No direct pushes, including by the owner
- CI required, every gate
- Linear history required
- Force-push and deletion blocked

Until that issue lands, treat these as conventions and follow them anyway. A
rule that only holds once a robot enforces it was never a rule.

## Labels

Every issue carries one `type:`, one `area:`, one `priority:` and one `size:`.
An `epic` label replaces `size:` on a tracking issue.

The set lives in [`../../.github/labels.yml`](../../.github/labels.yml) and is
reconciled with:

```bash
node tools/apply-labels.mjs           # apply; re-running is a no-op
node tools/apply-labels.mjs --check   # fail if GitHub has drifted
```

Creating a label in the GitHub UI without adding it there is how a label set
rots. `--check` reports labels that exist on GitHub but are not managed, so the
drift is visible rather than accumulating quietly.

## Issue templates

Three, and blank issues are disabled:

| Template           | For                                                                        |
| ------------------ | -------------------------------------------------------------------------- |
| **Epic**           | A tracking issue that holds sub-issues. Never receives code.               |
| **Implementation** | A unit of work that produces code. Belongs to an Epic.                     |
| **Bug**            | Something is broken. Media bugs require the source file's characteristics. |

All three arrive assigned to the owner with a starting label set. What belongs
in each section — and specifically the difference between requirements and
acceptance criteria — is in
[`../project-management/issue-rules.md`](../project-management/issue-rules.md).

## The loop, end to end

1. Pick an issue whose blockers are actually done, not merely closed.
2. Branch: `<type>/<number>-<slug>`.
3. `node tools/project-graph/query.mjs <file>` before touching existing code.
4. Test first, then implement.
5. `pnpm verify` locally. Do not delegate first discovery of a failure to CI.
6. Open the PR with `Closes #N` and an honest Definition of Done.
7. Squash merge on green. Delete the branch.
