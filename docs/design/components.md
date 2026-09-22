# The primitive components

The thirteen controls every panel in Epics #4 to #8 is assembled from, and the
rules they are built under. Implementation:
[`packages/ui/src/components/`](../../packages/ui/src/components/). Review them
in isolation with `pnpm storybook`.

- [The three rules](#the-three-rules)
- [The components](#the-components)
- [Why the primary action is not a gradient](#why-the-primary-action-is-not-a-gradient)
- [Tooltip timing](#tooltip-timing)
- [Where the accessible name comes from](#where-the-accessible-name-comes-from)
- [What the gates cover, and what they do not](#what-the-gates-cover-and-what-they-do-not)

## The three rules

**1. Radix underneath, always.** Focus management, escape handling, portal
behaviour, roving focus, typeahead and ARIA wiring all come from
[Radix UI](https://www.radix-ui.com). None of it is re-implemented, because
re-implementing it is where accessibility bugs come from — and because every one
of those behaviours is invisible until the person who needs it cannot use the
application.

**2. Not one literal value.** Every measurement reads a token.
`tools/design-token-gate.mjs` fails the build on a length, a duration or a font
weight anywhere in `packages/ui` outside the token files, and
`tools/colour-gate.mjs` does the same for colour. See
[`scales.md`](./scales.md) and [`colour-tokens.md`](./colour-tokens.md).

**3. Plain CSS, not Tailwind.** `packages/ui` may not depend on the application
(`CLAUDE.md` section 2) and Tailwind's theme is configured in the renderer. A
primitive styled with utility classes would render unstyled in Storybook, which
is exactly where it has to be reviewable. Classes are `bk-block__element--modifier`;
the prefix exists because these load into an application that also runs
Tailwind.

## The components

| Component      | Radix package         | What it is for                                    | Keyboard                                         |
| -------------- | --------------------- | ------------------------------------------------- | ------------------------------------------------ |
| `Button`       | —                     | Four variants: primary, secondary, ghost, danger  | Enter, Space                                     |
| `IconButton`   | — (wraps `Tooltip`)   | A control whose content is a glyph                | As Button; the label is required                 |
| `Tooltip`      | `react-tooltip`       | What a control does, plus its shortcut            | Opens on focus, not only on hover                |
| `Slider`       | `react-slider`        | Volume, denoise strength, speed, zoom             | Arrows, Page Up/Down, Home, End                  |
| `NumberInput`  | —                     | A numeric field whose label scrubs                | Arrows, Shift+arrow ×10, Home, End               |
| `Switch`       | `react-switch`        | A setting that takes effect immediately           | Space; the label is clickable                    |
| `Tabs`         | `react-tabs`          | The library panel's tab row                       | Roving focus: one Tab stop, arrows within        |
| `Select`       | `react-select`        | A closed, short list of options                   | Enter to open, typeahead, arrows, Escape         |
| `Popover`      | `react-popover`       | Controls anchored to the control that opened them | Escape closes, focus returns to the trigger      |
| `DropdownMenu` | `react-dropdown-menu` | The application bar's menus                       | Enter to open, arrows, Escape                    |
| `ContextMenu`  | `react-context-menu`  | Right click on a clip, a track, an asset          | Same items, same keys                            |
| `Dialog`       | `react-dialog`        | Export, and confirming an overwrite               | Escape closes, focus is trapped and returned     |
| `ScrollArea`   | `react-scroll-area`   | A thin scrollbar that belongs to the dark chrome  | Untouched — the region stays natively scrollable |

Sizes come from `--control-height-sm|md|lg` (24, 28, 32px). 28px is the default
because that is what a dense editing toolbar wants; 32px is a web form and puts
four fewer controls in the same toolbar.

### Two that are worth reading the source of

**`Slider` knows nothing about units.** It takes a range, a step and a
`formatValue`, and the caller owns the meaning — volume is decibels around a
meaningful zero, speed is a multiplier, zoom is exponential in pixels per
second. Designing for that spread now is the difference between one slider and
four near-identical ones.

**`DropdownMenu` and `ContextMenu` are one design in two files' worth of API.**
Items are declared as data and rendered by whichever package's primitives are
handed in, so a change to the item shape cannot reach one menu and miss the
other.

## Why the primary action is not a gradient

#17 asks for the primary button to carry the brand gradient. It does not, and
the reason is measured rather than aesthetic.

| Label colour         | On azure `#22a3fd` | On indigo `#4e52fc` | On violet `#8b46fb` |
| -------------------- | ------------------ | ------------------- | ------------------- |
| White `#ffffff`      | **2.72:1**         | 5.33:1              | 4.81:1              |
| Near-black `#0a0c10` | 7.20:1             | **3.67:1**          | **4.07:1**          |

A label is one colour and the surface under it is three. White fails at the
azure stop — it misses even the 3:1 that large text owes — and near-black fails
at the indigo and violet stops. **No label colour clears WCAG AA across the
sweep**, so the gradient cannot sit behind text at all.

So the primary action uses the flat `--accent`, whose three interaction states
were measured for exactly this job in #15: 5.33:1 resting, 4.71:1 hovered,
6.46:1 pressed.

The gradient keeps the surfaces where it carries no text — the logo, the
playhead handle, selection accents. `components.test.tsx` asserts the
measurement, so if the brand ever moves far enough for a label to be legible on
it, the decision is reopened by a failing test rather than by somebody
remembering it was made.

## Tooltip timing

#17 asks for two things that read as contradictory:

> the first is delayed, subsequent ones in the same group are immediate

> moving the pointer quickly across a row of icon buttons does not flicker

They are reconciled by **when the skip window starts**: only after a tooltip has
been shown and dismissed.

- A pointer sweeping across a cold toolbar never rests for
  `TOOLTIP_DELAY_MS` (500ms), so nothing opens, so no skip window starts, so
  nothing flickers.
- Once the user has deliberately rested on one control and read its tooltip,
  they are exploring — and for `TOOLTIP_SKIP_DELAY_MS` (300ms) the neighbours
  answer at once.

One `TooltipProvider` wraps the whole application, because that provider is the
group clock. Tooltips mounted without it each run their own timer, and a row of
independent timers is the flickering toolbar the delay exists to prevent.

## Where the accessible name comes from

The defect this component set is most exposed to is an icon button with no
accessible name. A timeline toolbar of those is announced as fourteen controls
called "button", and **nothing on screen looks wrong** — which is why it is
designed out rather than reviewed for.

- `IconButton` takes `label` as a **required** prop, and it becomes both the
  `aria-label` and the tooltip. One source, so the sighted user and the
  screen-reader user are told the same thing.
- `Slider`, `Switch`, `Select` and `Tabs` all require a `label` too.
- `Dialog` requires a `title`, which becomes its accessible name.
- `Slider` puts its name on the **thumb**, not the root, because that is where
  Radix puts `role="slider"`. The story accessibility suite caught that one; no
  amount of looking at the screen would have.

## What the gates cover, and what they do not

| Gate                  | Covers                                                             |
| --------------------- | ------------------------------------------------------------------ |
| `pnpm tokens:check`   | No hard-coded length, duration or font weight in `packages/ui`     |
| `pnpm colours:check`  | No colour value outside the token file                             |
| `stories.test.tsx`    | axe over every Storybook story — names, roles, labels, ARIA wiring |
| `components.test.tsx` | What the keyboard can do, component by component                   |
| `tokens.test.ts`      | Every documented contrast pair, measured from the token file       |

The accessibility suite runs axe in jsdom rather than driving a browser, and the
trade is deliberate: a browser buys real colour-contrast measurement and costs a
download plus a minute of the fifteen-minute pull-request budget — and contrast
is already measured from the token file. What jsdom cannot see is recorded
rather than quietly skipped:

- **Colour contrast** — no computed styles. `tokens.test.ts` owns it.
- **Focus order and visible focus** — both need layout.
- **Page-scope rules** (`region`, `landmark-one-main`, `page-has-heading-one`,
  `bypass`) — a story renders one control into a bare body, so all four fire on
  every story and none of them says anything about the control. They belong to
  the shell (#19), where they are **not** silenced.

The disable list is short and argued one item at a time, because a disable list
is how an audit quietly stops auditing. The guard on that is an injection case:
`stories.test.tsx` renders a deliberately unnamed icon button and fails if axe
does not catch it.
