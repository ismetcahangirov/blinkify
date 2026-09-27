# ADR-0020 — A crop is re-encoded; the codec's cropping window is not a lossless crop

- **Status**: Accepted
- **Date**: 2026-09-27
- **Context issue**: [#126](https://github.com/ismetcahangirov/blinkify/issues/126),
  Epic [#125](https://github.com/ismetcahangirov/blinkify/issues/125)

## Context

Epic #125 plans crop as tier 3: decode, crop, re-encode the clip's pictures.
The crop model of #127
([ADR-0019](./ADR-0019-a-crop-is-a-rectangle-per-clip-in-display-pixels.md))
forces `ReEncodeReason::FilterChangesPixels` on them. But H.264 and HEVC both carry a cropping window in the sequence
parameter set — H.264's `frame_cropping_flag` and
`frame_crop_{left,right,top,bottom}_offset`, HEVC's `conformance_window_flag`
and `conf_win_*_offset` — which tells the decoder to show only part of the
coded picture. It is how 1080 rows travel in 1088 coded rows. Rewriting it
changes a parameter set and no picture byte, and parameter sets are already
outside the hash boundary of #45. If players honour it, a crop is a copy, and
`CLAUDE.md` section 1 requires the planner to prefer it.

So it had to be measured. The trap #126 names is assuming every player honours
the window because FFmpeg does.

### The prototype

`crates/blinkify-engine/src/export/crop_window.rs` reads an SPS up to its
window with the parser of `export/sps.rs`, writes the new offsets, copies every
remaining bit of the payload, re-aligns the trailing bits and re-applies
emulation prevention. It rewrites the SPS out of band (`avcC`, `hvcC`, or Annex
B configuration) and in band (every packet that repeats it), and sets the NUT
stream header's size to the window's — the MP4 muxer writes `tkhd` and the
sample entry's size from it. Nothing in the planner or the executor calls it.

`crates/blinkify-engine/examples/crop_window.rs` is the tool the measurements
below were made with: the export's own shape — the sidecar reads a source's
packets out as NUT, the engine rewrites the header and packets, the sidecar
muxes — with every packet copied. An output ending in `.ts` is written as
MPEG-TS, where every SPS travels in band. `tests/crop_window.rs` runs the same
tool on the corpus.

Checked on bytes, on every corpus file (H.264 High closed and open GOP, High
10, HEVC Main with open GOP, Main 10, HDR10, the portrait phone file, the edit
list file):

- every picture packet of the output hashes identically to the source's, by
  the hash boundary of #45, out of band (MP4) and in band (MPEG-TS, compared
  with the sidecar's own plain copy into the same container, which adds the
  same access unit delimiters);
- the sidecar decodes every frame to exactly the source's pixels inside the
  window (frame MD5s against the source decoded through FFmpeg's `crop`
  filter), with an empty error stream;
- FFmpeg's `trace_headers` reads every SPS field of the output the same as the
  source's except the window;
- the SPS bytes are identical to those FFmpeg's own `h264_metadata` and
  `hevc_metadata` bitstream filters write for the same window.

The prototype is correct. The question is what players do with its output.

## What was measured

On this machine (Windows 11, 2026-09-27), every file below was decoded by each
player and the frame one second in was located inside the source picture by
exhaustive search (mean absolute error over every even placement, and over the
whole picture scaled), so each outcome is the rectangle the player actually
showed, not the size it reported.

**Files.** The corpus files cropped by a 16-pixel border (640×360 → 608×328;
the HDR and portrait files 1280×720 → 1248×688, the portrait file in its stored
landscape orientation). Measurement files made for this ADR by the corpus tool
(1920×1080, three seconds, everything outside the intended window painted pure
red, so a wrong rectangle shows red): a centred 608×1080 window (H.264 High,
H.264 Main, HEVC Main, HEVC Main 10), a centred 640×1080 window whose left edge
is a multiple of 64 (H.264, HEVC), a 16-pixel border (H.264, HEVC, and H.264
stored landscape with a 90° display matrix), a window anchored at the top-left
corner (1280×720: H.264, HEVC, and the 640×360 corpus H.264 and HEVC cropped
to 608×328 from the corner), and the H.264 centre crop written as MPEG-TS. The
same measurement files, uncropped, were the controls; every player showed them
whole. For the MP4 `clap` box: the uncropped measurement files with a `clap`
box added to the sample entry (centred 608×1080 and 1888×1048), with and
without `tkhd` set to the aperture's size.

**Players.**

| Player                                                                                                                                         | Build                 |
| ---------------------------------------------------------------------------------------------------------------------------------------------- | --------------------- |
| FFmpeg, the bundled sidecar                                                                                                                    | 8.1.3                 |
| Google Chrome, default (GPU decode where it applies)                                                                                           | 153.0.8010.49         |
| Google Chrome, `--disable-accelerated-video-decode` (its software decoder)                                                                     | 153.0.8010.49         |
| Microsoft Edge, default                                                                                                                        | 154.0.4258.37         |
| Microsoft Edge, `--disable-accelerated-video-decode`                                                                                           | 154.0.4258.37         |
| Chromium (Playwright's build)                                                                                                                  | 148.0.7778.96         |
| Firefox (Playwright's build; H.264 and HEVC through Windows Media Foundation)                                                                  | 150.0.2               |
| Windows Media Foundation, through WinRT `MediaComposition`: the system decoders the Windows apps use, the HEVC Video Extension 2.5.33 for HEVC | Windows 11 10.0.26200 |

**Outcomes.** `exact`: the window, where it should be. `origin`: the window's
size, taken from the top-left corner of the stored picture — the offsets
ignored. `whole`: the window ignored. `unpainted`: the frame at the full coded
size, the window's pixels in place and everything outside it unpainted
(green). A size and position is what was shown instead, in display
coordinates. `n/a`: the player cannot play the
uncropped source either (no software HEVC decoder in Chromium; High 10 H.264
fails in Firefox and in Media Foundation).

Windows with left or top offsets:

| File → window                           | FFmpeg | Chrome       | Chrome, software | Edge             | Edge, software  | Chromium     | Firefox     | Media Foundation |
| --------------------------------------- | ------ | ------------ | ---------------- | ---------------- | --------------- | ------------ | ----------- | ---------------- |
| H.264 High closed GOP, 608×328 @16,16   | exact  | 624×328 @0,0 | 624×328 @0,0     | 624×328 @0,0     | 624×328 @0,0    | 624×328 @0,0 | origin      | exact            |
| H.264 High open GOP, 608×328 @16,16     | exact  | 624×328 @0,0 | 624×328 @0,0     | 624×328 @0,0     | 624×328 @0,0    | 624×328 @0,0 | origin      | exact            |
| H.264 High, 512×328 @64,16              | exact  | origin       | origin           | origin           | origin          | origin       | origin      | exact            |
| H.264 High 10, 608×328 @16,16           | exact  | 624×328 @0,0 | 624×328 @0,0     | 624×328 @0,0     | 624×328 @0,0    | 624×328 @0,0 | n/a         | n/a              |
| H.264 portrait (90°), 1248×688 @16,16   | exact  | exact        | 688×1264 @0,16   | exact            | 688×1264 @0,16  | exact        | origin      | exact            |
| H.264 High 1080p, 608×1080 @656,0       | exact  | exact        | 624×1080 @0,0    | exact            | 624×1080 @0,0   | exact        | origin      | exact            |
| H.264 Main 1080p, 608×1080 @656,0       | exact  | exact        | 624×1080 @0,0    | exact            | 624×1080 @0,0   | exact        | origin      | exact            |
| H.264 1080p, 640×1080 @640,0            | exact  | exact        | origin           | exact            | origin          | exact        | origin      | exact            |
| H.264 1080p, 1888×1048 @16,16           | exact  | exact        | 1904×1048 @0,0   | exact            | 1904×1048 @0,0  | exact        | origin      | exact            |
| H.264 1080p portrait, 1048×1888 @16,16  | exact  | exact        | 1048×1904 @0,16  | exact            | 1048×1904 @0,16 | exact        | origin      | exact            |
| H.264 1080p as MPEG-TS, 608×1080 @656,0 | exact  | n/a (no TS)  | n/a (no TS)      | n/a (no TS)      | n/a (no TS)     | n/a (no TS)  | n/a (no TS) | exact            |
| HEVC Main open GOP, 608×328 @16,16      | exact  | exact        | n/a              | origin           | n/a             | exact        | whole       | unpainted        |
| HEVC Main 10, 608×328 @16,16            | exact  | exact        | n/a              | origin           | n/a             | exact        | whole       | unpainted        |
| HEVC HDR10, 1248×688 @16,16             | exact  | exact        | n/a              | origin           | n/a             | exact        | whole       | unpainted        |
| HEVC Main 1080p, 608×1080 @656,0        | exact  | exact        | n/a              | 608×1080 @1264,0 | n/a             | exact        | whole¹      | unpainted        |
| HEVC Main 10 1080p, 608×1080 @656,0     | exact  | exact        | n/a              | 608×1080 @54,0   | n/a             | exact        | whole¹      | unpainted        |
| HEVC 1080p, 640×1080 @640,0             | exact  | exact        | n/a              | 640×1080 @1280,0 | n/a             | exact        | whole       | unpainted        |
| HEVC 1080p, 1888×1048 @16,16            | exact  | exact        | n/a              | origin           | n/a             | exact        | whole¹      | unpainted        |

Windows anchored at the top-left corner (only right and bottom offsets — the
kind every encoder writes for 1088 → 1080):

| File → window              | FFmpeg | Chrome | Chrome, software | Edge  | Firefox | Media Foundation |
| -------------------------- | ------ | ------ | ---------------- | ----- | ------- | ---------------- |
| H.264 High, 608×328 @0,0   | exact  | exact  | exact            | exact | exact   | exact            |
| H.264 1080p, 1280×720 @0,0 | exact  | exact  | exact            | exact | exact   | exact            |
| HEVC Main, 608×328 @0,0    | exact  | exact  | n/a              | exact | exact   | unpainted        |
| HEVC 1080p, 1280×720 @0,0  | exact  | exact  | n/a              | exact | exact   | unpainted        |

MP4 `clap` box on the uncropped files (with and without `tkhd` set to the
aperture; the outcome was the same either way):

| File → aperture                | FFmpeg | Chrome | Edge  | Chromium | Firefox | Media Foundation |
| ------------------------------ | ------ | ------ | ----- | -------- | ------- | ---------------- |
| H.264 1080p, 608×1080 centred  | exact  | whole  | whole | whole    | whole   | unpainted        |
| H.264 1080p, 1888×1048 centred | exact  | whole  | whole | whole    | whole   | unpainted        |
| HEVC 1080p, 608×1080 centred   | exact  | whole  | whole | whole    | whole   | whole            |

¹ Firefox's HEVC playback on this machine is intermittent: on the first run
these three files failed to open ("Utility MF Media Engine CDM only support for
media engine playback"); on a second run they played as shown and the
_uncropped_ control failed the same way. The failure is Firefox's, not the
window's.

What the table shows, beyond the cells:

- **Chrome and Edge are two players each.** For small videos, and wherever GPU
  decode is unavailable, Chromium decodes H.264 with its FFmpeg-based software
  decoder, which ignores the window's left and top offsets and shows up to 16
  columns more than the window: 624 wide for a 608 window starting at 16 or at
  656, exactly 640 for one starting at 640. That is consistent with libavcodec
  rounding the left crop down to keep its planes 32-byte aligned (a multiple
  of 64 luma samples here) when `AV_CODEC_FLAG_UNALIGNED` is not set, and the
  offset then being lost. The same file is exact at 1080p on this machine's
  GPU and wrong at 640×360, or on a machine without GPU H.264 decode. Which one
  a viewer gets is not the file's to decide.
- **Edge's and Media Foundation's HEVC path** (the HEVC Video Extension) do not
  honour a window with offsets: Edge shows the right size at a wrong position
  (the region to the window's right, at 1264 instead of 656), Media Foundation
  shows the whole coded picture with the window's pixels in place and nothing
  painted around them.
- **Firefox** keeps the window's size and drops its offsets for H.264, and
  ignores HEVC windows with offsets entirely.
- **FFmpeg** honours everything, the window and `clap`. It is the one player
  of the list that says nothing about the others.

**Not measured**, and not claimed either way: VLC and mpv (not installed on
this machine), QuickTime and Safari, iOS and Android players, televisions, and
upload targets (YouTube and the like; no accounts). Films & TV was launched
once on the H.264 centre crop from the scratch directory and reported error
0xC00D36B4 before showing anything; it was not investigated further, so the
app's own renderer is not measured — Media Foundation above is the decoders it
uses. Playwright's Windows build of WebKit (26.4, not Safari) was run and is
indicative only: its canvas returned black frames for the 640×360 files and
colour-shifted frames otherwise; where readable, it showed H.264 windows in
place (including the MPEG-TS file), ignored HEVC windows, and did not apply the
display matrix. No real phone footage exists on this machine; the corpus's
portrait file (x264, stored landscape, 90° display matrix) stands in for it.

## Decision

**The planner never crops through the codec's cropping window. A crop of an
H.264 or HEVC clip is tier 3, as #127 and #128 build it; the SPS window is not
offered, not chosen automatically and not offered as an option. The MP4 `clap`
box is not used either.**

The rule of section 1 is the lowest tier _that satisfies the edit_. The edit is
"show this rectangle". A file whose shown rectangle depends on the player —
exact in FFmpeg and in Chromium's GPU path, the top-left corner of the picture
in Firefox and in Chromium's software path, the whole picture or a region to
its right through the Windows HEVC decoder — does not satisfy it. Worse, it
fails silently: the file plays everywhere, the export report would say "copied,
nothing lost", and the viewer sees a different picture from the one the user
framed. That is the most serious bug class `CLAUDE.md` names, reached from the
other side.

Two further facts would stand even if every player honoured the window:

- **The cropped-away pixels stay in the file.** The whole coded picture is
  still stored and decoded; any tool that rewrites or ignores the window shows
  it again. Users crop to remove things — a watermark, a stranger, a licence
  plate. A crop that hides instead of removing would have to say so, every
  time, and that is not what a user asking for a crop expects.
- **The file is no smaller** and decodes at the full coded size.

## Constraints, recorded for whoever revisits this

- **Offsets are in chroma units**: H.264 `CropUnitX = SubWidthC`,
  `CropUnitY = SubHeightC × (2 − frame_mbs_only_flag)` (1 and
  `2 − frame_mbs_only_flag` for monochrome or separate planes); HEVC
  `SubWidthC`, `SubHeightC`. In 4:2:0 every edge of the window is on an even
  luma sample; a field-coded H.264 stream needs multiples of 4 vertically.
  The prototype refuses anything else (`WindowError::Misaligned`) rather than
  rounding.
- **Only inside the picture shown.** The window can shrink what the source
  shows, never reveal the coded rows it hides (the 8 rows under 1080) or reach
  outside (`WindowError::OutsidePicture`). It composes with the window the source
  has: a 16-row bottom crop of 1080p writes 24 coded rows, 12 in chroma units.
- **The output's size is the window's.** It fits only a sequence whose settings
  are exactly the window's size; ADR-0008's `copy_eligibility` would have to
  learn it, or the planner decline it.
- **The rectangle is in the stored orientation.** A portrait phone clip stored
  landscape with a display matrix is cropped in landscape coordinates; the crop
  model
  ([ADR-0019](./ADR-0019-a-crop-is-a-rectangle-per-clip-in-display-pixels.md))
  stores display pixels, so every consumer would need the exact mapping.
- **One window per SPS.** Clips with different windows cannot share one SPS in
  one stream; with the same window they must already have identical parameter
  sets to be copied into one stream at all (the incompatible-encoding case of
  #39). A new SPS may only take effect at an IDR picture, so a clip starting on
  an open-GOP keyframe could not change it anyway.
- **Parameter sets that change mid-stream** — an in-band SPS describing a
  different picture from the configuration's — are refused
  (`WindowError::ParameterSetsChange`); one window cannot mean the same rectangle
  in both.
- **The container states the size too.** `tkhd` and the sample entry must be
  the window's size, or players that trust the container scale the picture.
- **The only subset every measured player honoured** is an H.264 window with
  no left or top offset (right and bottom only). Media Foundation fails even
  that for HEVC. It serves trimming the right and bottom edges and nothing
  else — not a vertical version, not a centred frame, not a border.

## Alternatives considered

### Adopt: the planner chooses the window wherever it applies — rejected

What #126 set out to test. The measurements above show a file that is right in
some players and silently wrong in others, including two mainstream browsers
depending on the video's size and the machine's GPU. Choosing it automatically
would ship that without the user knowing.

### Adopt as an explicit option ("crop without re-encoding — some players ignore it") — rejected

It moves the decision to the user without the information to make it: nobody
knows which player their viewer will use, and section 17 says a user must never
have to guess what they will get. It also ships the hidden pixels. An option
that is wrong for most real crops (every crop with a left or top edge) is not a
lossless crop; it is a different product.

### Restrict it to H.264 windows anchored at the top-left corner — rejected for now

The one subset every measured player showed correctly. But it covers only
trimming the right and bottom edges, the players not measured are exactly the
ones this subset would have to be proved on (VLC, QuickTime and Safari, phones,
televisions, upload transcoders), and the pixels still stay in the file. If the
question is ever reopened, this is where to start, with the prototype and the
method above.

### The MP4 `clap` (clean aperture) box — rejected

It leaves the stream alone and describes the window in the container instead.
Every browser measured ignored it and showed the whole picture, with or without
`tkhd` set to the aperture; Media Foundation showed the aperture in place
inside an unpainted full-size frame for H.264 and ignored it for HEVC; only
FFmpeg applied it. It exists only in MP4 and MOV, and the pixels stay in the
file.

### HEVC's VUI default display window — rejected without measuring further

A second, advisory window in HEVC's VUI (`default_display_window_flag`). FFmpeg,
the one player that honoured everything above, ignores it by default
(`apply_defdispwin` is off in its HEVC decoder), so it fails the first row of
the table before any other player is asked.

### FFmpeg's `h264_metadata` / `hevc_metadata` crop options as the rewriter — moot

The sidecar ships both bitstream filters, and their SPS bytes are identical to
the prototype's. Had the window been adopted, they would have been a
competitor to the engine's own rewriter. The decision makes the choice moot.

### Tier 3: decode, crop, re-encode — accepted

The crop costs quality, confined to the cropped clip's pictures, stated before
the export and in the report (#127, #128). It is honest: what the user framed
is what every player shows, and the cropped-away pixels are gone.

## Consequences

### What this makes easy

- Crop has one meaning everywhere: the rectangle, rendered. #127 and #128 build
  it with no second route, and the report's "what could have been done
  instead" for a cropped segment can cite this ADR: nothing lossless that every
  player shows.

### What this makes hard

- A crop of a 4K phone clip costs a generation. That is the price section 1
  accepts where the pixels have to change, and here they do — for the viewer.

### What we accept

- The prototype stays in the engine, called by nothing but its tests and the
  measurement tool, so the measurement can be repeated when players change
  (`cargo run -p blinkify-engine --example crop_window -- IN OUT X Y W H`, then
  play the output and locate the frame shown inside the source's). Rerunning it
  is the first step of any ADR that supersedes this one.
- No implementation issue is opened under Epic #125: there is nothing to
  implement.
