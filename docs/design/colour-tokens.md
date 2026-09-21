# Colour tokens

The Blinkify palette, the role each token plays, and the measured contrast of
every pair a user reads.

Source of truth: [`packages/ui/src/tokens/tokens.css`](../../packages/ui/src/tokens/tokens.css).
It is the only file in `apps/` or `packages/` permitted to contain a colour
value, and `pnpm colours:check` fails the build on any other.

Issue [#15](https://github.com/ismetcahangirov/blinkify/issues/15).

---

## The shape of the system

Two layers, and a component may only see the second.

| Layer          | Names                                                                     | What it is                                                                     |
| -------------- | ------------------------------------------------------------------------- | ------------------------------------------------------------------------------ |
| 1 — primitives | `--brand-*`, `--neutral-1` … `--neutral-13`, `--hue-*`                    | Raw values. They name a **colour**.                                            |
| 2 — semantic   | `--background`, `--surface`, `--text-primary`, `--accent`, `--timeline-*` | Role to value, each one a `var()` reference into layer 1. They name a **job**. |

A component that reads `--neutral-3` instead of `--surface` is a review
failure, because it is the one thing that stops a light theme from being a
value swap. `tokens.test.ts` asserts that every semantic token is a reference
and never a literal, which is the machine-checkable half of the same rule.

---

## Brand

Measured from the logo. These three values **are** the brand and are not
adjusted to taste.

| Token              | Value                                                            | Role                                                            |
| ------------------ | ---------------------------------------------------------------- | --------------------------------------------------------------- |
| `--brand-azure`    | `#22a3fd`                                                        | The cyan end of the logo's sweep. Accent text, focus, playhead. |
| `--brand-indigo`   | `#4e52fc`                                                        | The centre. The primary action's fill.                          |
| `--brand-violet`   | `#8b46fb`                                                        | The violet end. Gradient terminus.                              |
| `--brand-gradient` | `linear-gradient(135deg, #22a3fd 0%, #4e52fc 50%, #8b46fb 100%)` | The logo's own sweep, at the logo's own angle.                  |

Two derived steps exist for interaction states, moved as far as the contrast
table allows and no further:

| Token                 | Value     | Role                  |
| --------------------- | --------- | --------------------- |
| `--brand-indigo-lift` | `#5a5efd` | Accent fill, hovered. |
| `--brand-indigo-deep` | `#4245e8` | Accent fill, pressed. |

### Where the gradient may appear

`CLAUDE.md` section 17 and Epic [#2](https://github.com/ismetcahangirov/blinkify/issues/2):
**the gradient is a brand surface, not a UI surface.**

Permitted: the logo, the splash, the primary action's brand treatment, the
playhead handle, selection accents.

Forbidden: any panel background. The editing chrome sits beside the video the
user is judging by eye, and a saturated panel shifts their perception of the
image. This is why professional tools look grey, and it is a decision rather
than a lack of ambition.

The gradient is also **not a text-bearing surface**, and this is measured
rather than asserted. White text on the azure end measures 2.72:1 and
near-black text on the violet end measures 3.40:1 — there is no text colour
that clears AA across the whole sweep. Text goes on `--accent`, which is solid.

---

## Neutral ramp

Thirteen steps, darkest to lightest. Not pure grey: every step carries a slight
blue cast (hue ≈ 215°, very low saturation) so the chrome reads as related to
the brand, kept under the threshold where it would tint the eye's white point
and change how the user judges the preview.

| Step           | Value     | Used as                                        |
| -------------- | --------- | ---------------------------------------------- |
| `--neutral-1`  | `#0a0c10` | `--background` — the application window        |
| `--neutral-2`  | `#0e1116` | `--surface-sunken` — the timeline track bed    |
| `--neutral-3`  | `#12161c` | `--surface` — panels                           |
| `--neutral-4`  | `#171c23` | `--surface-raised` — cards, inputs, toolbars   |
| `--neutral-5`  | `#1d222a` | `--surface-overlay` — menus, popovers, dialogs |
| `--neutral-6`  | `#242a33` | `--surface-hover`                              |
| `--neutral-7`  | `#2c333d` | `--border`                                     |
| `--neutral-8`  | `#39424e` | `--surface-active`                             |
| `--neutral-9`  | `#616d7e` | `--border-strong`, `--text-disabled`           |
| `--neutral-10` | `#8a94a3` | `--text-secondary`, `--timeline-ruler-text`    |
| `--neutral-11` | `#b8c0cb` | Reserved — high-emphasis secondary text        |
| `--neutral-12` | `#e7ebf1` | `--text-primary`, `--timeline-clip-label`      |
| `--neutral-13` | `#ffffff` | `--accent-foreground` only                     |

Step 13 is pure white and exists for one reason: on the accent fill, step 12
measures 4.46:1 and misses AA by four hundredths. Body text stays on step 12 on
purpose — pure white on near-black is harsh to read for the hours an edit takes.

---

## Semantic roles

| Token                 | Resolves to           | Role                                                       |
| --------------------- | --------------------- | ---------------------------------------------------------- |
| `--background`        | `--neutral-1`         | The application window itself                              |
| `--surface-sunken`    | `--neutral-2`         | Recessed surfaces: the timeline track bed                  |
| `--surface`           | `--neutral-3`         | Panels                                                     |
| `--surface-raised`    | `--neutral-4`         | Cards, inputs, toolbars                                    |
| `--surface-overlay`   | `--neutral-5`         | Menus, popovers, dialogs                                   |
| `--surface-hover`     | `--neutral-6`         | Neutral control, hovered                                   |
| `--surface-active`    | `--neutral-8`         | Neutral control, pressed or selected                       |
| `--border`            | `--neutral-7`         | Decorative separator between surfaces                      |
| `--border-strong`     | `--neutral-9`         | The outline that makes a control identifiable as a control |
| `--text-primary`      | `--neutral-12`        | Body text                                                  |
| `--text-secondary`    | `--neutral-10`        | Supporting text, labels                                    |
| `--text-disabled`     | `--neutral-9`         | Text inside an inactive control                            |
| `--text-accent`       | `--brand-azure`       | Links and accent icons on a dark surface                   |
| `--accent`            | `--brand-indigo`      | The primary action's fill                                  |
| `--accent-hover`      | `--brand-indigo-lift` | Primary action, hovered                                    |
| `--accent-pressed`    | `--brand-indigo-deep` | Primary action, pressed                                    |
| `--accent-foreground` | `--neutral-13`        | The label on an accent fill                                |
| `--focus-ring`        | `--brand-azure`       | The focus indicator, everywhere                            |
| `--focus-ring-width`  | `2px`                 | Ring thickness                                             |
| `--focus-ring-offset` | `2px`                 | Gap between control and ring — **load-bearing, see below** |
| `--success`           | `--hue-success`       | Something finished                                         |
| `--warning`           | `--hue-warning`       | Something needs attention                                  |
| `--danger`            | `--hue-danger`        | Something failed, or will destroy work                     |
| `--lossless`          | `--hue-lossless`      | The lossless indicator, and nothing else                   |

### Two roles for the accent, on purpose

`--accent` (indigo) is a **fill**. `--text-accent` (azure) is **text and
icons**. They are different tokens because indigo as text on a dark panel
measures 3.67:1 and fails AA, while azure as a fill cannot carry a white label
at 2.72:1. Using either in the other's place ships an unreadable control.

### `--lossless` is reserved

It is used by the lossless indicator and nothing else. If it appears anywhere
that is not a statement about whether pixels were preserved, the signal it
carries is gone — `CLAUDE.md` section 17. It is deliberately not a second
green: "this export finished" and "this export changed no pixels" are different
claims, and only one of them is the product.

The indicator never relies on colour alone. It carries a label, so a viewer who
cannot separate the teal from the success green reads the same information.

---

## Timeline

The canvas renderer ([#33](https://github.com/ismetcahangirov/blinkify/issues/33))
cannot read a stylesheet, so it resolves these at paint time:

```ts
const style = getComputedStyle(document.documentElement);
const playhead = style.getPropertyValue("--timeline-playhead").trim();
```

This is why `--timeline-playhead` is a solid colour rather than the gradient: a
gradient token cannot be handed to a canvas fill, and one would push #33 into
hard-coding the very value this file exists to own. The gradient belongs on the
playhead's DOM handle, which is an element and can carry it.

| Token                             | Resolves to        | Role                  |
| --------------------------------- | ------------------ | --------------------- |
| `--timeline-track`                | `--surface-sunken` | The track bed         |
| `--timeline-clip-video`           | `#273358`          | Video clip fill       |
| `--timeline-clip-video-border`    | `#55639b`          | Video clip edge       |
| `--timeline-clip-audio`           | `#1b4740`          | Audio clip fill       |
| `--timeline-clip-audio-border`    | `#3f7f73`          | Audio clip edge       |
| `--timeline-clip-label`           | `--neutral-12`     | Clip name             |
| `--timeline-clip-selected-border` | `--brand-azure`    | Selected clip edge    |
| `--timeline-playhead`             | `--brand-azure`    | The playhead line     |
| `--timeline-ruler-text`           | `--neutral-10`     | Timecode on the ruler |

Clip fills are dark and desaturated deliberately. A timeline of bright clips
competes with the preview above it, and a clip is a container for a thumbnail
strip rather than a thing to look at in itself. The **edge**, not the fill, is
what carries the 3:1 that makes a clip identifiable against the track.

---

## Contrast

Measured to WCAG 2.2, relative luminance from linearised sRGB. Every row below
is computed from `tokens.css` by `tokens.test.ts`, which fails if a token moves
and this table does not. Ratios are truncated, not rounded: a pair measuring
4.4996 must not be published as `4.50` beside a 4.5 threshold it does not meet.

Duties:

- **Text** — WCAG §1.4.3, 4.5:1. Body-sized text a user reads.
- **Non-text** — WCAG §1.4.11, 3:1. A component boundary or state indicator.
- **Disabled text** — exempt from §1.4.3 as an inactive component. Held to 3:1
  here anyway, because it is still read by someone working out why a control
  will not respond.

| Foreground                        | Background              | Usage                                             | Duty          | Measured | Required |
| --------------------------------- | ----------------------- | ------------------------------------------------- | ------------- | -------- | -------- |
| `--text-primary`                  | `--background`          | Body text on the application background           | Text          | 16.35:1  | 4.5:1    |
| `--text-primary`                  | `--surface`             | Body text on a panel                              | Text          | 15.16:1  | 4.5:1    |
| `--text-primary`                  | `--surface-raised`      | Body text on a card, input or toolbar             | Text          | 14.30:1  | 4.5:1    |
| `--text-primary`                  | `--surface-overlay`     | Body text in a menu, popover or dialog            | Text          | 13.35:1  | 4.5:1    |
| `--text-secondary`                | `--background`          | Supporting text on the application background     | Text          | 6.37:1   | 4.5:1    |
| `--text-secondary`                | `--surface`             | Supporting text on a panel                        | Text          | 5.91:1   | 4.5:1    |
| `--text-secondary`                | `--surface-raised`      | Supporting text on a card or toolbar              | Text          | 5.57:1   | 4.5:1    |
| `--text-secondary`                | `--surface-overlay`     | Supporting text in a menu or dialog               | Text          | 5.20:1   | 4.5:1    |
| `--text-accent`                   | `--background`          | Link or accent icon on the application background | Text          | 7.19:1   | 4.5:1    |
| `--text-accent`                   | `--surface`             | Link or accent icon on a panel                    | Text          | 6.67:1   | 4.5:1    |
| `--text-accent`                   | `--surface-raised`      | Link or accent icon on a card or toolbar          | Text          | 6.29:1   | 4.5:1    |
| `--text-accent`                   | `--surface-overlay`     | Link or accent icon in a menu or dialog           | Text          | 5.87:1   | 4.5:1    |
| `--accent-foreground`             | `--accent`              | Label on the primary action                       | Text          | 5.33:1   | 4.5:1    |
| `--accent-foreground`             | `--accent-hover`        | Label on the primary action, hovered              | Text          | 4.71:1   | 4.5:1    |
| `--accent-foreground`             | `--accent-pressed`      | Label on the primary action, pressed              | Text          | 6.46:1   | 4.5:1    |
| `--success`                       | `--surface`             | Success message on a panel                        | Text          | 7.14:1   | 4.5:1    |
| `--success`                       | `--surface-raised`      | Success message on a card                         | Text          | 6.73:1   | 4.5:1    |
| `--warning`                       | `--surface`             | Warning message on a panel                        | Text          | 8.26:1   | 4.5:1    |
| `--warning`                       | `--surface-raised`      | Warning message on a card                         | Text          | 7.80:1   | 4.5:1    |
| `--danger`                        | `--surface`             | Error message on a panel                          | Text          | 6.04:1   | 4.5:1    |
| `--danger`                        | `--surface-raised`      | Error message on a card                           | Text          | 5.70:1   | 4.5:1    |
| `--lossless`                      | `--surface`             | Lossless indicator on a panel                     | Text          | 10.22:1  | 4.5:1    |
| `--lossless`                      | `--surface-raised`      | Lossless indicator in the export dialog           | Text          | 9.64:1   | 4.5:1    |
| `--timeline-clip-label`           | `--timeline-clip-video` | Clip name on a video clip                         | Text          | 10.31:1  | 4.5:1    |
| `--timeline-clip-label`           | `--timeline-clip-audio` | Clip name on an audio clip                        | Text          | 8.68:1   | 4.5:1    |
| `--timeline-ruler-text`           | `--timeline-track`      | Timecode on the timeline ruler                    | Text          | 6.16:1   | 4.5:1    |
| `--text-disabled`                 | `--surface`             | Label on a disabled control on a panel            | Disabled text | 3.45:1   | 3.0:1    |
| `--text-disabled`                 | `--surface-raised`      | Label on a disabled control on a card             | Disabled text | 3.25:1   | 3.0:1    |
| `--text-disabled`                 | `--surface-overlay`     | Label on a disabled item in a menu                | Disabled text | 3.04:1   | 3.0:1    |
| `--border-strong`                 | `--surface`             | Input or control outline on a panel               | Non-text      | 3.45:1   | 3.0:1    |
| `--border-strong`                 | `--surface-raised`      | Input or control outline on a card                | Non-text      | 3.25:1   | 3.0:1    |
| `--focus-ring`                    | `--background`          | Focus ring against the application background     | Non-text      | 7.19:1   | 3.0:1    |
| `--focus-ring`                    | `--surface`             | Focus ring against a panel                        | Non-text      | 6.67:1   | 3.0:1    |
| `--focus-ring`                    | `--surface-raised`      | Focus ring against a card or toolbar              | Non-text      | 6.29:1   | 3.0:1    |
| `--focus-ring`                    | `--surface-overlay`     | Focus ring against a menu or dialog               | Non-text      | 5.87:1   | 3.0:1    |
| `--accent`                        | `--background`          | Edge of the primary action against the background | Non-text      | 3.66:1   | 3.0:1    |
| `--accent-hover`                  | `--background`          | Edge of the primary action, hovered               | Non-text      | 4.15:1   | 3.0:1    |
| `--accent-pressed`                | `--background`          | Edge of the primary action, pressed               | Non-text      | 3.02:1   | 3.0:1    |
| `--timeline-clip-video-border`    | `--timeline-track`      | Video clip edge against the track bed             | Non-text      | 3.28:1   | 3.0:1    |
| `--timeline-clip-audio-border`    | `--timeline-track`      | Audio clip edge against the track bed             | Non-text      | 4.04:1   | 3.0:1    |
| `--timeline-clip-selected-border` | `--timeline-track`      | Selected clip edge against the track bed          | Non-text      | 6.95:1   | 3.0:1    |
| `--timeline-playhead`             | `--timeline-track`      | Playhead against the track bed                    | Non-text      | 6.95:1   | 3.0:1    |

### What the table does not contain, and why

**`--border` against anything.** It separates one dark panel from another.
Nothing about identifying a control depends on seeing it, so §1.4.11 does not
apply, and forcing it to 3:1 would draw a heavier line than the interface
wants. `--border-strong` is the token that outlines controls, and it is
measured on both surfaces a control can sit on.

**`--focus-ring` against `--accent`.** It measures 1.96:1, and no colour fixes
it: a ring bright enough to sit on a saturated brand fill vanishes on the dark
surface beside it. It is fixed by geometry. `--focus-ring-offset` is non-zero,
so the ring is drawn on the surface **behind** the control rather than against
the control, and the pairs that actually occur are the four measured above.
`tokens.test.ts` asserts the offset stays non-zero, because setting it to zero
would silently restore a failure that no colour test would catch.

---

## How this is enforced

| Gate                      | What it catches                                                                                |
| ------------------------- | ---------------------------------------------------------------------------------------------- |
| `pnpm test:ts`            | A token moved below its threshold, or this table drifting from the tokens.                     |
| `pnpm colours:check`      | A colour value written anywhere but `tokens.css`.                                              |
| `pnpm colours:check:test` | That the colour gate still fires — it writes a violation, asserts the failure, and removes it. |

See [`../engineering/architecture-gates.md`](../engineering/architecture-gates.md)
for the rest of the gate set and for what each one does not cover.

## Still outstanding

One testing requirement in #15 is not met by any of the above and is not
mechanisable: **visual review of the palette against a real video frame**,
confirming the chrome does not tint perception of the image. It needs a decoded
frame in the preview zone, which arrives with Epic
[#3](https://github.com/ismetcahangirov/blinkify/issues/3) and
[#4](https://github.com/ismetcahangirov/blinkify/issues/4). It is tracked as
[#74](https://github.com/ismetcahangirov/blinkify/issues/74) rather than ticked
off here, because a checkbox claiming a review that nobody performed is worse
than an open issue.
