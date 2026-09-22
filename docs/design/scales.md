# Scales: type, spacing, radius, elevation and motion

The non-colour half of the Blinkify design system. Colour is documented
separately in [`colour-tokens.md`](./colour-tokens.md); everything else is here.

Source of truth: [`packages/ui/src/tokens/scales.css`](../../packages/ui/src/tokens/scales.css).
This document is checked against that file by `scales.test.ts`, which fails if a
token exists without its value appearing here. A design system documented
wrongly is worse than one not documented at all, because people believe it.

- [Why scales](#why-scales)
- [Typefaces](#typefaces)
- [Type scale](#type-scale)
- [Spacing](#spacing)
- [Radius](#radius)
- [Elevation](#elevation)
- [Motion](#motion)
- [Reduced motion](#reduced-motion)
- [How to consume a token](#how-to-consume-a-token)
- [What is deliberately absent](#what-is-deliberately-absent)

## Why scales

Without a fixed set of steps, a component that needs "a bit more space here"
gets 13px, the next gets 14px, and eighteen months later the interface is a
collection of screens that were each individually reasonable. A scale is not a
restriction on judgement — it is what makes one person's judgement usable by the
next person a year later.

Two layers, mirroring the colour file:

| Layer     | What it names | Example                                        |
| --------- | ------------- | ---------------------------------------------- |
| 1 — scale | a size        | `--space-4`, `--duration-fast`                 |
| 2 — role  | a job         | `--type-body-size`, `--elevation-2-background` |

Read layer 2 wherever a layer-2 token exists for what you are doing. Spacing is
the deliberate exception: padding and gap vary too much by context to be
usefully named, so components read `--space-*` directly.

## Typefaces

Two families, both bundled and committed —
[`packages/ui/src/fonts/`](../../packages/ui/src/fonts/) holds the files, their
licences and their provenance.

| Token                | Value                                                                  | Used for                   |
| -------------------- | ---------------------------------------------------------------------- | -------------------------- |
| `--type-family-sans` | `"Inter", "Segoe UI Variable Text", "Segoe UI", system-ui, sans-serif` | Everything a user reads    |
| `--type-family-mono` | `"JetBrains Mono", "Cascadia Mono", Consolas, monospace`               | Timecode, and nothing else |

**Why two.** Inter's digits are proportional. Measured from the exact file we
ship, its `1` advances 833 units against `4`'s 1323 on a 2048 em, so a timecode
set in Inter changes width as it counts — and a running timecode that jitters is
a visible defect in an editor, because the user is watching that number move.
JetBrains Mono advances every digit, the colon, the semicolon and the full stop
at exactly 600 units on a 1000 em.

That is asserted, not assumed: `fonts.test.ts` parses the WOFF2 binary and
compares the advances. Replacing the timecode family with a proportional one
fails the build.

**Why bundled.** A desktop application must open on a machine with no network.
The files sit in the repository, `@font-face` points at them by relative path,
and `tools/offline-assets-gate.mjs` fails the build on any remote asset URL
anywhere in `apps/` or `packages/`.

**The subset is Latin.** Text outside it — a filename in Cyrillic, Arabic or CJK,
which a media library will certainly meet — falls through to the system UI font
by design. A full-Unicode face is measured in megabytes.

## Type scale

Six roles. Each earns its place by being needed somewhere in the four zones; a
seventh would be a decision nobody can later reconstruct.

Sizes are `rem` against a 16px root, so a future interface-density control is one
root-font-size change rather than a sweep through every token. Windows display
scaling arrives as the WebView's device pixel ratio and does not interact with
this.

| Role       | Size               | Line              | Weight | Tracking  | Used for                                                        |
| ---------- | ------------------ | ----------------- | ------ | --------- | --------------------------------------------------------------- |
| `display`  | `1.75rem` (28px)   | `2.125rem` (34px) | `600`  | `-0.02em` | Empty states, the first-run screen. Never inside a panel.       |
| `title`    | `1.125rem` (18px)  | `1.5rem` (24px)   | `600`  | `-0.01em` | Panel headings, dialog titles, inspector section headers.       |
| `body`     | `0.875rem` (14px)  | `1.25rem` (20px)  | `400`  | `0em`     | Running text and inspector values.                              |
| `label`    | `0.8125rem` (13px) | `1rem` (16px)     | `500`  | `0em`     | Control labels, buttons, tabs, menu items. Most of the product. |
| `caption`  | `0.6875rem` (11px) | `0.875rem` (14px) | `400`  | `0.01em`  | Ruler numbers, hints, secondary metadata. The floor.            |
| `timecode` | `0.8125rem` (13px) | `1rem` (16px)     | `400`  | `0.01em`  | Playhead position, in and out points, durations, progress.      |

Tokens are `--type-<role>-size`, `--type-<role>-line`, `--type-<role>-weight`
and `--type-<role>-tracking`, so the full set is `--type-display-size`,
`--type-display-line`, `--type-display-weight`, `--type-display-tracking`,
`--type-title-size`, `--type-title-line`, `--type-title-weight`,
`--type-title-tracking`, `--type-body-size`, `--type-body-line`,
`--type-body-weight`, `--type-body-tracking`, `--type-label-size`,
`--type-label-line`, `--type-label-weight`, `--type-label-tracking`,
`--type-caption-size`, `--type-caption-line`, `--type-caption-weight`,
`--type-caption-tracking`, `--type-timecode-size`, `--type-timecode-line`,
`--type-timecode-weight` and `--type-timecode-tracking`.

Each role also ships as a class — `.type-display` through `.type-timecode` — and
that is the form to use. A role is four or six properties that belong together,
and composing them at each call site is how one of them eventually loses
`font-variant-numeric: tabular-nums` on a timecode.

**The interface runs small on purpose.** This is a dense editing tool whose
chrome exists to stay out of the way of the picture (`CLAUDE.md` section 17), not
a document reader. `label` at medium weight rather than regular is because light
text on a dark surface reads thinner than the same weight on a light one.

## Spacing

A 4px base. The step number is the multiple: `--space-3` is 12px, always.

| Token        | Value     | px  | Used for                                       |
| ------------ | --------- | --- | ---------------------------------------------- |
| `--space-0`  | `0rem`    | 0   | A named zero, so "no gap" is still a decision. |
| `--space-1`  | `0.25rem` | 4   | Icon to its label. The tightest legal gap.     |
| `--space-2`  | `0.5rem`  | 8   | Inside a control.                              |
| `--space-3`  | `0.75rem` | 12  | Between controls in a toolbar.                 |
| `--space-4`  | `1rem`    | 16  | Panel padding. The default.                    |
| `--space-5`  | `1.25rem` | 20  |                                                |
| `--space-6`  | `1.5rem`  | 24  | Between sections in the inspector.             |
| `--space-8`  | `2rem`    | 32  |                                                |
| `--space-10` | `2.5rem`  | 40  |                                                |
| `--space-12` | `3rem`    | 48  |                                                |
| `--space-16` | `4rem`    | 64  | Empty-state breathing room, and nothing else.  |

The steps thin out above 6 because 28px and 36px serve nothing 24px or 32px does
not, and an unused token is a token someone eventually uses for the wrong thing.
`scales.test.ts` asserts every step is a whole multiple of 4px: a 4px grid with
one 6px exception in it is not a grid, and the exception always arrives in good
faith.

## Radius

| Token           | Value    | Used for                                        |
| --------------- | -------- | ----------------------------------------------- |
| `--radius-none` | `0px`    | Timeline ruler, track bed, anything that tiles. |
| `--radius-sm`   | `2px`    | Chips, tags, the timeline clip body.            |
| `--radius-md`   | `4px`    | Buttons, inputs, selects. The default.          |
| `--radius-lg`   | `8px`    | Dialogs, popovers, cards.                       |
| `--radius-full` | `9999px` | Slider thumbs and anything genuinely circular.  |

Small numbers, deliberately. A video editor is a tool, and a large radius on the
default control is the single change that most makes an editor look like a toy.

One border width in the system: `--border-width`, `1px`. A second width is how
two panels end up looking like they came from different applications.

## Elevation

Depth by surface and border, never by shadow.

| Level | Background token           | Value                    | Border token           | Value                  | Used for                        |
| ----- | -------------------------- | ------------------------ | ---------------------- | ---------------------- | ------------------------------- |
| 0     | `--elevation-0-background` | `var(--background)`      | `--elevation-0-border` | `transparent`          | The application window.         |
| 1     | `--elevation-1-background` | `var(--surface)`         | `--elevation-1-border` | `var(--border)`        | A panel.                        |
| 2     | `--elevation-2-background` | `var(--surface-raised)`  | `--elevation-2-border` | `var(--border)`        | Toolbar, input, card.           |
| 3     | `--elevation-3-background` | `var(--surface-overlay)` | `--elevation-3-border` | `var(--border-strong)` | Menu, popover, dialog, tooltip. |

Also available as classes `.elevation-0` through `.elevation-3`, which is the
form to use: the background and the border were chosen together, and
`--surface-overlay` with `--border` reads as a panel that failed to separate
rather than as a menu.

**Why no shadow.** A drop shadow works by darkening what is behind it, and on a
near-black background there is nothing left to darken. The shadow is invisible
and its only surviving effect is to push every raised element onto its own
compositing layer: the interface pays the GPU and the user sees nothing. Depth
comes out of the neutral ramp instead, which was built for exactly this — each
step is a visible lift against the one below, measured in
[`colour-tokens.md`](./colour-tokens.md).

Level 3 is the only level taking `--border-strong`, because an overlay has to be
separable from the panel it covers and, without a shadow, the border is the only
thing doing that work.

If Blinkify ever gains a shadow it arrives as an ADR, not as a property someone
added to a card. `scales.test.ts` fails on `box-shadow` in this stylesheet.

## Motion

Short. Shorter than a website, deliberately: the user of an editor operates the
tool continuously rather than reading it, opening the same menu forty times an
hour, and anything above roughly 150ms on a panel or a menu stops reading as
polish and starts reading as the application being slow.

| Token                | Value   | Used for                                          |
| -------------------- | ------- | ------------------------------------------------- |
| `--duration-instant` | `0ms`   | State changes that must not animate at all.       |
| `--duration-fast`    | `80ms`  | Hover, focus, press — anything under the pointer. |
| `--duration-normal`  | `120ms` | A menu or a popover opening.                      |
| `--duration-slow`    | `180ms` | A dialog entering. The ceiling.                   |

`--duration-slow` exists for the one case that genuinely benefits — a dialog
taking over the window, where the motion says where the dialog came from. Using
it on a hover is a review failure.

| Token               | Value                        | Used for                                                       |
| ------------------- | ---------------------------- | -------------------------------------------------------------- |
| `--easing-standard` | `cubic-bezier(0.2, 0, 0, 1)` | Anything that starts and ends on screen.                       |
| `--easing-entrance` | `cubic-bezier(0, 0, 0, 1)`   | Something arriving: decelerates so the eye can catch it.       |
| `--easing-exit`     | `cubic-bezier(0.3, 0, 1, 1)` | Something leaving: accelerates away, stops competing.          |
| `--easing-linear`   | `linear`                     | Anything representing real elapsed time — progress, buffering. |

`--easing-linear` matters more than it looks. An eased progress bar misreports how
much work is left, and in an export that runs for four minutes the user is
reading that bar as a promise.

## Reduced motion

Under `prefers-reduced-motion: reduce`, two mechanisms run:

1. **Every duration token is redefined to `0ms`.** Anything that animated by
   reading a `--duration-*` — which is everything in `packages/ui` — stops
   moving, keeps its end state, and needs no knowledge of the preference.
   `scales.test.ts` derives the list from the root block rather than from a
   hand-kept list, so a duration token added without an override fails on the
   commit that adds it.
2. **A blanket rule underneath**, setting `transition-duration` and
   `animation-duration` to `1ms !important`. Radix animations, third-party
   markup and hand-written keyframes do not read our tokens, and a user who
   asked the operating system for less motion did not ask only the parts of the
   interface we remembered to wire up.

`1ms` rather than `0s` is deliberate: a zero-duration animation never fires
`animationend`, and a component that unmounts on that event would stay on screen
forever. A reduced-motion setting that breaks menus is one the user turns off.

**The exit.** `[data-motion="essential"]` opts an element and its subtree out of
the blanket rule. Essential motion carries information no static frame carries —
an indeterminate progress indicator saying work is still happening. Decoration is
not essential, and marking something essential to keep an effect you liked is the
misuse this paragraph exists to name.

## How to consume a token

**In the renderer**, prefer the Tailwind utilities. `apps/desktop/src/styles.css`
publishes every scale into the Tailwind theme under the same names, so
`--space-4` is `p-4`, `--radius-md` is `rounded-md`, `--duration-fast` is
`duration-fast`, and the type roles are `text-display` through `text-timecode`.

**In `packages/ui`**, read the custom property directly:

```css
transition: background-color var(--duration-fast) var(--easing-standard);
border-radius: var(--radius-md);
```

**On canvas** — the timeline renderer (#33) — resolve through
`getComputedStyle(document.documentElement)` at paint time, exactly as the colour
tokens are resolved. Canvas cannot read CSS, and a renderer that hard-codes a
size because reading one was awkward is the failure the whole token set exists to
prevent.

## What is deliberately absent

| Absent              | Why                                                                                                                    |
| ------------------- | ---------------------------------------------------------------------------------------------------------------------- |
| A shadow scale      | Invisible on a near-black background; costs a compositing layer. See [Elevation](#elevation).                          |
| A light theme       | Out of scope for Epic #2. The structure supports it: layer 2 is redefined, layer 1 and every component stay.           |
| Italic faces        | Nothing in an editor's chrome is set in italic, and the face would be 48 KB bought for nothing.                        |
| A seventh type role | Six cover the four zones. A role added for one screen is a role nobody else can place.                                 |
| Named spacing roles | `--space-panel-padding` and forty siblings would be a set of tokens used once each, which is a scale with extra steps. |
