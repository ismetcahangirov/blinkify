Closes #

## What changed

<!-- Why, not what. The diff already says what. -->

## How it was verified

<!-- The commands you ran and what they printed. "Tested locally" is not
     evidence. If a gate does not exist yet, say so and say which issue owns
     it — an unchecked box with a reason is fine, a checked box that is not
     true is not. -->

```

```

## Definition of Done

<!-- CLAUDE.md section 8. Answer honestly. -->

- [ ] Every requirement in the issue is implemented, or explicitly deferred with
      a linked follow-up issue
- [ ] Every acceptance criterion in the issue is demonstrably met
- [ ] Tests cover the new behaviour, including the failure cases, and they fail
      if the behaviour is removed
- [ ] `pnpm verify` passes locally
- [ ] CI is green on every gate
- [ ] The project graph is regenerated and committed if the source tree moved
- [ ] Documentation updated: ADR for a decision, `docs/` for a concept,
      doc-comments for a public API
- [ ] No new dependency without `CLAUDE.md` section 10 satisfied
- [ ] No `TODO`, commented-out code, or debug logging left behind

## If this touches the media path

<!-- Delete this section if it does not. -->

- [ ] The tier decision (copy / smart-cut / re-encode) is unchanged, or the
      change is recorded and justified
- [ ] No intermediate media file is written
- [ ] Any new re-encode carries a recorded reason that reaches the export report
- [ ] No source file is modified or deleted
- [ ] The losslessness suite still passes
