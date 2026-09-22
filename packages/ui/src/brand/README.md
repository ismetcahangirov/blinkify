# The Blinkify mark

The logo, and the two vector files every icon in the product is generated from.

| File                       | What it is                                                          |
| -------------------------- | ------------------------------------------------------------------- |
| `blinkify-logo-master.png` | The owner's artwork, as supplied. 1254 × 1254, transparent.         |
| `blinkify-mark.svg`        | The full mark: eye, iris, play triangle. Source for 24px and above. |
| `blinkify-mark-small.svg`  | The eye alone, 10% larger. Source for 16px and 20px.                |

Run `pnpm icons` to regenerate every raster from these. `pnpm icons:check` is a
blocking gate that fails if what is committed no longer matches.

## The SVGs are traced, not drawn

**This matters, so it is the first thing in the file.** The artwork arrived as a
PNG. The vectors beside it were produced by tracing that PNG, not by opening the
designer's original.

The trace is faithful, and "faithful" here is a number rather than an opinion.
Rendering `blinkify-mark.svg` back at the master's own resolution and comparing
pixel for pixel:

| Measurement                                  | Result   |
| -------------------------------------------- | -------- |
| Pixels whose opacity differs from the master | 0.35%    |
| Mean per-channel colour difference           | 13 / 255 |
| Worst per-channel colour difference          | 52 / 255 |

The 0.35% is the antialiased edge — the silhouette, which is the part anyone
recognises, is essentially exact. The colour difference is the gradient, and it
is explained below.

**If the original vector file exists, it should replace these.** Drop it in as
`blinkify-mark.svg`, re-run `pnpm icons`, and every raster follows. Nothing else
in the repository refers to the geometry.

## How the trace was made

Recorded so that it can be redone rather than re-invented.

1. **Silhouette.** The master's alpha channel was thresholded at 50%, split into
   connected components — the eye ring and the play triangle are separate shapes
   — and each component's boundary was walked as a lattice contour.
2. **Simplification.** Douglas–Peucker at a 0.8px tolerance on a 1254px canvas,
   which is 0.06% of the mark's width. That reduces the eye from 3105 boundary
   points to 557.
3. **Smoothing.** The simplified vertices are emitted as Catmull-Rom cubics
   rather than straight segments. Verified not to cost fidelity: the alpha
   mismatch moved from 0.32% to 0.35%, and without it the outline is visibly
   faceted at 256px.
4. **Gradient.** The mark's own colours were sampled and the axis fitted. Three
   findings, in order of usefulness:
   - The colour is a clean function of `x + y`. Testing every angle from 0° to
     90°, the within-band variation is lowest at 43° and barely different at 45°
     — so the axis is the brand gradient's own 135°, confirmed rather than
     assumed.
   - The sampled ramp runs `#1da1fb → #4053fb → #7d49fc`, which is the brand
     triad in [`../tokens/tokens.css`](../tokens/tokens.css) — `#22a3fd`,
     `#4e52fc`, `#8b46fb` — read off the same logo by #15. So the SVG uses the
     **tokens**, not sampled values: the icon and the interface are provably one
     brand, and a future change to the brand moves both.
   - The middle stop sits at 0.28, not 0.5, and the axis extends well past the
     shape. Both were fitted; with them the mean error is 3.2/255 across the
     ramp.

**What the trace does not reproduce.** The residual colour error is concentrated
in the lower-right of the eye, where the master is bluer than any single linear
gradient predicts — the green channel fits to ±2, the red channel does not. The
master was most likely built from two overlapping shapes with separate
gradients. Reproducing that would be guesswork, and at icon sizes the difference
is not perceptible. It is written down here rather than left for somebody to
rediscover.

## Why there are two marks

At 16px the play triangle is four pixels of mud. This was rendered and looked at
rather than assumed — side by side, the full mark's centre is an indistinct
purple smear where the simplified mark reads clearly as an eye.

#20 gives the rule for exactly this case: if something must be dropped at small
sizes, drop the inner play triangle before the silhouette. So the 16px and 20px
entries in the `.ico` come from `blinkify-mark-small.svg`, which is the eye alone
at 110% scale — with no inner detail to protect, the silhouette can be bigger.

Everything from 24px up uses the full mark.

## The wordmark is not here

The master contains "Blinkify" beneath the mark, and it is deliberately not
traced. "Blink" is near-white and disappears on a light background, which is the
observation Epic #2 opens with and the reason the product is dark-first. An
application icon is a mark, not a lockup, and a wordmark at 16px is a grey
smudge.

If a wordmark is ever needed — a splash screen, an About dialog — it belongs
here as a third file, with the same provenance note.

## Licence

The mark is Blinkify's own. It is not third-party artwork and does not appear in
[`THIRD_PARTY.md`](../../../../THIRD_PARTY.md).
