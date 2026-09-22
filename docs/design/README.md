# Design

The visual language of Blinkify and the structure of its interface.

## Contents

- [`colour-tokens.md`](./colour-tokens.md) — the palette, the role of every
  token, and the measured contrast of every pair a user reads
- [`components.md`](./components.md) — the thirteen primitives, what each is
  for, how each is operated by keyboard, and what the gates around them cover
- [`scales.md`](./scales.md) — type, spacing, radius, elevation and motion: the
  non-colour half of the system, and the typefaces it is set in
- [`capcut-layout-reference.md`](./capcut-layout-reference.md) — the four-zone
  structure the shell is built to, zone by zone, with the default and minimum
  size of every one and an explicit list of what Blinkify does not copy

## What belongs here

- Brand tokens derived from the logo: palette, semantic colour roles
- The typography, spacing, radius, elevation and motion scales
- The CapCut desktop layout reference, zone by zone, and how the Blinkify shell
  maps onto it
- Component specifications: states, sizes, keyboard behaviour, focus treatment
- Iconography and the application icon derivation
- Accessibility requirements: contrast ratios, focus visibility, target sizes

## What does not belong here

- **Component implementation** — that is `packages/ui`. Documents here describe
  what a component must do; the code does it.
- **Why a decision was made** when it constrains other work — that is an ADR in
  [`../decisions/`](../decisions/).

## Principles carried from the rulebook

From [`../../CLAUDE.md`](../../CLAUDE.md) section 17:

- The **layout structure** follows CapCut deliberately, so a user arriving from
  CapCut is not re-learning where things are. The **visual language** is
  Blinkify's own, derived from the logo.
- Dark theme first.
- The lossless tier is visible in the interface, always — before the export in
  the dialog, after it in the report.
- Where Blinkify cannot do something well, it declines with a reason rather than
  silently substituting a worse result.

## Conventions

- Tokens are documented with their value **and their semantic role**. A palette
  entry without a role gets used arbitrarily and the system dissolves.
- Every layout reference names a zone the way the codebase names it.
