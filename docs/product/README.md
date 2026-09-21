# Product

What Blinkify does for a user, and what it deliberately does not do.

## What belongs here

- The problem statement: quality loss on export, and who it hurts
- User-facing behaviour of each capability, in the user's words
- The honesty contract: what the user is told, and when, about quality
- Explicit non-goals and the reasoning behind each
- Competitive positioning and the licensing constraints that shape it

## What does not belong here

- **Implementation** of any kind
- **Schedule** — that is
  [`../project-management/`](../project-management/)

## The one thing that cannot change here

The binding constraint from [`../../CLAUDE.md`](../../CLAUDE.md) section 1:

> No operation in Blinkify may degrade media quality that did not have to be
> degraded.

Product documents may describe how that constraint is expressed to a user. They
may not soften it. A feature that cannot be built within it does not ship — it
gets declined in the interface, with a reason the user can read.

## Non-goals, standing

Blinkify is not a compositor, not a colour grading suite, and not a
motion-graphics tool. Feature requests that require every frame to be
re-encoded are evaluated against the constraint first and the roadmap second.
