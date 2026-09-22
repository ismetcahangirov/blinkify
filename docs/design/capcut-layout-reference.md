# The CapCut desktop layout, as a reference for the Blinkify shell

The owner has settled that Blinkify follows CapCut's **structure**, because that
is what the target user already knows. "Similar to CapCut" is not buildable.
This document is the buildable version: every zone, what it contains, what size
it starts at, and how small it may get.

It is written so that someone who has never opened CapCut can build the shell
from it alone. Where a statement describes CapCut, it is sourced — see
[Sources](#sources). Where a statement describes Blinkify, it is a decision, and
the reason is given.

**What is copied is the structure. Nothing else.** Not the icons, not the
artwork, not the wording, not the colour. The visual language is Blinkify's own,
derived from the logo — [`colour-tokens.md`](./colour-tokens.md) and the type,
spacing, radius, elevation and motion scales of #16 — and `CLAUDE.md` section 17
states that split.

- [The principle that makes the layout work](#the-principle-that-makes-the-layout-work)
- [The zone map](#the-zone-map)
- [Sizes: defaults, minimums and what happens at the edges](#sizes-defaults-minimums-and-what-happens-at-the-edges)
- [Zone 0 — the application bar](#zone-0--the-application-bar)
- [Zone 1 — library](#zone-1--library)
- [Zone 2 — player](#zone-2--player)
- [Zone 3 — inspector](#zone-3--inspector)
- [Zone 4 — timeline](#zone-4--timeline)
- [The lossless indicator](#the-lossless-indicator)
- [What Blinkify deliberately does not copy](#what-blinkify-deliberately-does-not-copy)
- [Sources](#sources)

## The principle that makes the layout work

CapCut's chrome is deliberately low-contrast, and the preview dominates. That is
not a style choice to admire and move past — it is the reason the layout works,
and it is the first thing lost when someone decides a panel needs more presence.

An editor is a room the user sits in for hours judging one thing: the picture.
Every surface around that picture is competing with it for the eye's white
balance and for attention. So the chrome stays on the neutral ramp, the
saturated brand gradient never becomes a panel background, and the only
saturated things on screen are the ones that mean something — the playhead, the
selection, the primary action, the lossless indicator.

This is recorded in [`colour-tokens.md`](./colour-tokens.md) as a colour rule. It
is repeated here because it is a **layout** rule as well: it decides which zone
gets the space when the window is small, and the answer is always the player.

## The zone map

Four zones plus a bar, in a two-row grid. CapCut's arrangement: a media panel
top-left, a preview centred above, a settings panel on the right, and the
timeline across the bottom.

```
┌─────────────────────────────────────────────────────────────────────────┐
│  0  APPLICATION BAR                                                     │
│  ≡ menus   Project name          ↶ ↷   ◆ Lossless   [ Export ]   – □ ×  │
├───────────────┬─────────────────────────────────┬───────────────────────┤
│               │                                 │                       │
│  1  LIBRARY   │  2  PLAYER                      │  3  INSPECTOR         │
│               │                                 │                       │
│  ┌ tab row ┐  │  ┌───────────────────────────┐  │  driven entirely by   │
│  │Media|Aud│  │  │                           │  │  the timeline         │
│  └─────────┘  │  │       video surface       │  │  selection            │
│  ┌ search  ┐  │  │                           │  │                       │
│  └─────────┘  │  └───────────────────────────┘  │  ┌─ section ───────┐  │
│               │  00:00:04;12 / 00:01:37;00      │  │                 │  │
│  ┌──┐ ┌──┐    │  ⏮ ◀| ▶ |▶ ⏭    quality  16:9  │  └─────────────────┘  │
│  └──┘ └──┘    │                                 │  ┌─ section ───────┐  │
│   asset grid  │                                 │  └─────────────────┘  │
│               │                                 │                       │
├───────────────┴─────────────────────────────────┴───────────────────────┤
│  4  TIMELINE                                                            │
│  ✂ split  ⌫ delete  ⧉ duplicate  ❄ freeze  ↺ reverse        ⊖ ──── ⊕   │
│  ├────────┼────────┼────────┼────────┼────────┼────────┼────────┤ ruler │
│  │▓▓▓▓▓▓▓▓▓▓▓▓▓│   ┃        │▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓│              V1      │
│  │░░░░░░░░░░░░░│   ┃        │░░░░░░░░░░░░░░░░░░░░│              A1      │
│                    playhead                                             │
└─────────────────────────────────────────────────────────────────────────┘
```

Zones are named in the codebase exactly as they are named here: `appBar`,
`library`, `player`, `inspector`, `timeline`. A zone that is called something
else in the code is a zone nobody can find from this document.

Three splitters, and no more:

| Splitter    | Between                | Axis       |
| ----------- | ---------------------- | ---------- |
| `library`   | library and player     | vertical   |
| `inspector` | player and inspector   | vertical   |
| `timeline`  | upper row and timeline | horizontal |

The application bar is not resizable. A bar the user can drag is a bar the user
can lose.

## Sizes: defaults, minimums and what happens at the edges

Every number here is a Blinkify decision, not a measurement of CapCut. They are
derived from one constraint: on the smallest window we support, the player must
still be large enough to judge a frame in, and every other zone must still be
usable rather than merely present.

**Window minimum: 1280 × 800.** Below that the four-zone layout stops being four
zones and starts being four slivers. The Tauri window declares this as its
minimum size, so the layout never has to handle a width it cannot serve.

| Zone        | Default                     | Minimum | Maximum | Notes                                                                                    |
| ----------- | --------------------------- | ------- | ------- | ---------------------------------------------------------------------------------------- |
| `appBar`    | 48px                        | 48px    | 48px    | Fixed. Not a splitter boundary.                                                          |
| `library`   | 22% of width                | 240px   | 40%     | Below 240px the asset grid drops to one column and stops being a grid.                   |
| `player`    | remainder                   | 480px   | —       | Takes every pixel the other two give up. It is the reason the application exists.        |
| `inspector` | 20% of width                | 260px   | 40%     | 260px is what a labelled control with a numeric field and a unit needs without wrapping. |
| `timeline`  | 38% of height below the bar | 180px   | 70%     | 180px holds the toolbar, the ruler and two tracks at their default height.               |

At the 1280px minimum width: 240 + 480 + 260 + two splitters = 988px, so there
is 292px of slack. The layout is never in a state where a minimum cannot be
honoured, which means a splitter always has somewhere to stop — and **stopping is
the required behaviour**. A splitter that collapses a zone to nothing has
deleted a zone the user then has to work out how to get back.

**Splitter geometry.** A 1px visual line using `--border`, with an 8px pointer
hit area centred on it. The visual line is thin because it is chrome; the hit
area is wide because it is a control, and a 1px drag target is a usability bug
that reads as the application being unresponsive.

**Splitters are keyboard-operable.** Tab reaches them, left/right or up/down
moves by one `--space-4` step, Home and End jump to the minimum and maximum.
This is how the 8px pointer target stops being the only way in, and it is the
reason the splitter is a real focusable control rather than a styled `div` with
a mouse listener.

**Persistence.** Splitter positions and window geometry are stored as a user
preference, outside the project file. A layout is a property of the person and
their monitor, not of the edit — copying a project to another machine must not
bring somebody else's panel widths with it. Restoring window geometry is
validated against the current monitor set first: a window restored onto a
monitor that is no longer attached is invisible, and the user's only recourse is
to know that this class of bug exists. Details in #19.

## Zone 0 — the application bar

Full width, 48px, fixed. Left to right:

1. **Menu button or menu bar** — File, Edit, View, Help. Blinkify draws its own
   window chrome (#19), so the menus live here rather than in a native menu bar.
2. **Project name** — the current project, editable in place. Unsaved state is
   shown here and nowhere else, so there is one place to look.
3. **Undo and redo** — with the name of the action they would undo in the
   tooltip. An undo stack the user cannot read is an undo stack they do not
   trust. Backed by #37.
4. **The lossless indicator** — see [below](#the-lossless-indicator).
5. **Export** — the primary action, at the far right, carrying the brand
   treatment. It is the only brand-gradient surface in the shell.
6. **Window controls** — minimise, maximise, close. Custom chrome means these
   are ours to draw, and double-click-to-maximise and snap behaviour are ours to
   implement rather than to inherit (#19).

## Zone 1 — library

### The tab row

Horizontal, across the top of the zone. CapCut's tabs are Media, Audio, Text,
Stickers, Effects, Transitions, Filters and Adjustment.

**Blinkify ships two of them: Media and Audio.** The rest are documented here
because this is a reference to CapCut, and are declined in
[What Blinkify deliberately does not copy](#what-blinkify-deliberately-does-not-copy)
— every one of them changes pixels, which makes every clip they touch a tier-3
re-encode, and `CLAUDE.md` section 1 evaluates a feature against that before it
evaluates it against the roadmap.

The tab row is built to hold more tabs. The shell does not need to know which
ones, and a row built for exactly two is a row that gets rewritten.

### The local-versus-library split

CapCut's Media tab separates the user's own files from a stock and cloud
library.

**Blinkify has no second half.** There is no stock library, because there is no
network: `CLAUDE.md` section 20 rule 8 allows exactly one outbound request, the
update check on launch. Everything in the library came off this machine. The
split is not hidden or greyed out — it does not exist, and the zone is designed
around one source rather than around one source and an empty tab.

### The asset grid

Thumbnails with duration, resolution and codec. Three columns at the default
width, reflowing to two and then to a list as the zone narrows; at 240px it is a
single column and that is the floor the minimum is set from.

An asset shows what Blinkify knows about it because Blinkify probed it (#23) —
container, codec, frame rate, and whether the frame rate is variable. That last
one is not decoration: a variable-frame-rate phone capture behaves differently
under every operation in the product, and the user finding out at export is the
worst possible moment.

### The search field

Above the grid, filtering the current tab. Filename and, once the probe has run,
codec and resolution.

Drag from the grid onto the timeline is the primary way media reaches an edit
(#53).

## Zone 2 — player

### The video surface

The frame, letterboxed inside the zone at the sequence's aspect ratio, on the
application background. Nothing is drawn over it that is not part of the picture
— no watermark, no logo, no overlay that is not the user's own content.

### The transport row

Below the surface, in this order:

1. **Timecode: current over total.** `00:00:04;12 / 00:01:37;00`. Set in the
   `timecode` type role from #16, which exists precisely so these digits do not
   change width as they count.
2. **Play and pause**, as one toggle.
3. **Frame step**, back and forward. Frame-accurate, which is a claim about the
   seek implementation (#29) rather than about the button.
4. **Fullscreen.**
5. **Preview quality selector.** Full, half, quarter. This is a preview-side
   decision only and it can never affect the output file: `CLAUDE.md` section 20
   rule 1 forbids an intermediate media file, and an optional preview proxy is a
   content-addressed cache that no export path reads.
6. **Aspect-ratio control**, showing the sequence's ratio. Changing it is a
   sequence setting (#57), not a per-clip transform.

## Zone 3 — inspector

### The rule

**The inspector's content is driven entirely by the current timeline selection,
and by nothing else.** It has no tabs of its own, no mode, and no memory of what
the user was last looking at. One selection, one panel.

This is the rule to hold onto when the zone gets busy. The moment the inspector
has a state the selection does not determine, the user has two places to look
for one answer.

### Sections by selection type

**Nothing selected — the sequence.** Resolution, frame rate, and the rule
binding those to copy eligibility (#57). This is the honest home for it: a
sequence setting that disagrees with the source is the single most common reason
a segment cannot be stream-copied, and burying it in a preferences dialog is how
a user ends up with a fully re-encoded export and no idea why.

**A video clip selected.** In order:

| Section   | Contents                                                | Tier                                                                                        |
| --------- | ------------------------------------------------------- | ------------------------------------------------------------------------------------------- |
| Clip      | Source file, in and out points, duration                | Tier 1 or 2 — a trim is a cut, not a filter.                                                |
| Speed     | Constant speed change (#56, #42)                        | Tier 1. Implemented by rescaling timestamps, so the pixels are untouched.                   |
| Transform | Crop, scale, rotation — reserved, not yet built         | **Tier 3.** Any of these changes pixels and forces a re-encode of the segment.              |
| Audio     | The clip's own audio: gain, and detaching it (#36, #46) | Its own tier. Audio is a separate stream and a gain filter must not drag video into tier 3. |

Every section that would force a re-encode says so, in the section, before the
user commits to it — not in the export dialog afterwards. `CLAUDE.md` section 20
rule 3: if the output will differ from the source, the user is told before the
export starts.

**An audio clip selected.** Volume and gain with true-peak limiting (#46), noise
reduction strength (#47), and loudness normalisation (#48), with a live preview
of the filter chain (#49).

**A transition or an effect selected.** Not applicable. Blinkify has neither —
see below.

## Zone 4 — timeline

Full width, below the upper row.

### The toolbar row

Above the tracks. CapCut's timeline toolbar carries split, delete left, delete
right, delete clip, add marker, crop, freeze, reverse, mirror and rotate, plus a
view group of zoom in, zoom out, zoom to fit, main-track magnet, auto-snapping
and linkage.

Blinkify's toolbar, left to right, is the subset that is either lossless or
honestly declared:

- **Split** at the playhead (#35)
- **Delete**, **duplicate** (#35)
- **Freeze frame**, **reverse** (#35)
- **Detach audio** from video (#36)
- **Snapping** and **linkage** toggles (#34, #36)

and at the **right-hand end of the toolbar**, the **zoom control**: out, a
slider, in, and zoom-to-fit. Right-hand end because the left-hand end is where
the eye starts and the operations belong there, and because a zoom control that
moves as the toolbar grows is a control the hand has to search for.

Each operation carries a keyframe indicator where the operation's tier depends
on keyframe alignment (#35) — a split that lands on a keyframe is tier 1 and one
that does not is tier 2, and the user can see which before they commit.

### The time ruler

Above the tracks, below the toolbar. Labelled in timecode at a density that
follows the zoom, in the `caption` type role using `--timeline-ruler-text`.

### The track stack

Video tracks above, audio tracks below, in one scrolling stack. Track headers on
the left carry the track name, a mute and a solo control, and a lock.

Clips are drawn by the canvas renderer (#33), not as DOM nodes. A DOM node per
clip does not survive a long timeline — `CLAUDE.md` section 3. The renderer
resolves its colours and sizes from the design tokens through
`getComputedStyle(document.documentElement)` at paint time, because canvas
cannot read CSS and the alternative is a renderer full of hard-coded values.

### The playhead

A full-height vertical line across every track, in `--timeline-playhead`, with a
handle in the ruler carrying the brand gradient. It is one of the few saturated
things on screen, and that is deliberate: it is the element the user's eye
tracks continuously.

## The lossless indicator

Blinkify has one element CapCut has no reason to have, and where it lives is a
decision this document owns.

**It lives in two places, at two granularities.**

**1. A summary in the application bar**, immediately to the left of Export.
One state plus a count:

- `Lossless` — every segment is a stream copy.
- `2 seams` — the cut points are not keyframe-aligned, so a fraction of a second
  is re-encoded at each. Tier 2.
- `3 segments re-encoded` — a filter forces the pixels to change. Tier 3.

Immediately left of Export because that is where the user's eye already is when
they are thinking about the output, and because the number is about the output.

**2. A per-segment marker on the timeline clip**, drawn by the canvas renderer.
The summary says how many; the timeline says **which**, and hovering one gives
the recorded reason it could not be copied.

Both exist because a count without a location is not actionable — a user told
that three segments will be re-encoded and not told which three cannot do
anything about it — and a location without a summary means the only way to know
the state of the project is to scan the whole timeline.

**Three rules that come with it:**

- **The renderer never computes a tier.** The export planner (#39) is the single
  source of truth, and the indicator displays what the planner returned.
  `CLAUDE.md` section 2.
- **It is visible without opening the export dialog**, and it updates as the
  edit changes. That is the whole requirement: a user must never have to guess
  whether quality was preserved.
- **Before the planner exists**, it shows that the tier has not been computed.
  It does not guess, and it does not show `Lossless` optimistically. A wrong
  claim here is worse than no claim, because it is the one claim the product is
  built on.

## What Blinkify deliberately does not copy

| Not copied                                                | Why                                                                                                                                                                                             |
| --------------------------------------------------------- | ----------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| The stock and cloud media library                         | There is no network. `CLAUDE.md` section 20 rule 8 allows one outbound request, the update check.                                                                                               |
| Templates, auto-captions and the AI tools                 | Several require uploading the user's footage. All of them are outside what `CLAUDE.md` section 1 describes as the product.                                                                      |
| The Text, Stickers, Effects, Transitions and Filters tabs | Every one burns pixels into the frame, which makes the clip a tier-3 re-encode. They are not forbidden forever, but each arrives with its tier declared in the interface or it does not arrive. |
| The Adjustment tab (colour correction)                    | Blinkify is not a grading suite — `CLAUDE.md` section 1, non-goals — and grading is a full re-encode of everything it touches.                                                                  |
| Switchable layout presets and the vertical workspace      | One layout, with splitters. A preset that rearranges the zones means this document describes one of several truths, and the next person has to ask which.                                       |
| Subscription and upsell surfaces                          | There is nothing to upsell in the editing window.                                                                                                                                               |
| The icons, artwork, wording and colour                    | Structure is what the user's muscle memory holds. The visual language is Blinkify's own and comes from the logo.                                                                                |

And one thing added that CapCut has no reason to have: the
[lossless indicator](#the-lossless-indicator).

## Sources

The structural claims about CapCut are taken from these, and a later reader
should be able to check any of them:

- Primal Video, _CapCut for PC & Mac: Beginner Tutorial_ —
  <https://primalvideo.com/guides/capcut-for-pc-mac-tutorial/> — the four-zone
  arrangement: import and assets top-left, preview centred, settings for the
  current selection on the right, timeline across the bottom.
- Filmora, _Master CapCut Timeline Settings_ —
  <https://filmora.wondershare.com/advanced-video-editing/capcut-timeline.html>
  — the timeline toolbar operations and the view group (zoom in and out, zoom to
  fit, main-track magnet, auto-snapping, linkage).
- CapCut, _How to Use CapCut_ — <https://www.capcut.com/resource/how-to-use-capcut>
  — the media panel's tab set.

Where the sources disagree or go quiet — none of them states a pixel dimension —
the numbers in [Sizes](#sizes-defaults-minimums-and-what-happens-at-the-edges)
are Blinkify's own and are labelled as such.
