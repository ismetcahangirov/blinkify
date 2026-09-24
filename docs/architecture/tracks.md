# Tracks, compositing and detached sound

From [#36](https://github.com/ismetcahangirov/blinkify/issues/36). Code:
`Track` in `crates/blinkify-engine/src/project/mod.rs`, `Timeline::picture`
in `project/evaluate.rs`, the track edits and `DetachAudio` in
`project/edit.rs`, `PlaybackPlan::from_timeline`, and
`apps/desktop/src/timeline/TrackHeaders.tsx`.

## The compositing order

Tracks are listed **top to bottom**, and that list is the compositing order:
a video track higher in the list is **in front** of the ones below it. v1
composites whole, opaque frames, so at any frame the picture is the top
visible video clip there. The evaluator states it once, as
`Timeline::picture()`: the visible video tracks flattened into one track of
placements, each cut down (with the split arithmetic of #35) to where it is
the one in front. The preview plays that; the export will write it.

```
V1  ········[ clip 4 ]·········          picture:  [1][ 4 ][1][ 2 ]   [ 3 ]
V2  [ clip 1 ][ clip 2 ]   [ 3 ]
```

Moving a video track up or down (`MoveTrack`) changes what is in front. A
video track cannot move below an audio track.

## Mute, solo, lock, collapse

Each is a switch on the track, in the project file, changed by an undoable
edit (`SetTrack`), and applied by the evaluator — so the preview and the
export agree.

| Switch   | Effect                                                                          |
| -------- | ------------------------------------------------------------------------------- |
| Mute     | A video track is not seen; any track is not heard                               |
| Solo     | While any track is soloed, only soloed tracks are heard. Muted wins over soloed |
| Lock     | Every edit to the track's clips is refused, from every entry point (see below)  |
| Collapse | The row is drawn thin. Layout only                                              |

These are the project's mute and solo. The player's monitor volume, mute and
per-track monitor solo (#31) are for listening only and never reach an export.

**The lock is enforced in the edit layer.** Every edit — a drag, a key, the
inspector, a script — compiles to primitive changes, and `Document::apply`
refuses the edit if any change touches a clip of a locked track (inserting,
removing or replacing one, or removing the track). There is no path around
it, because there is no other way to change the graph (ADR-0007). The
timeline also refuses to start a drag on a locked track and says why; that is
a courtesy, not the enforcement.

Removing a track removes its clips, in one undoable edit; the menu item says
how many.

## Detached sound

A clip on a video track plays its file's pictures **and its sound**: every
visible video track's sound is mixed in, as a sound-only track of the preview
with the project's track id.

**Detach audio** gives a video clip's sound a clip of its own: an audio clip
of the **same file** — the sound stream, over the same moments, lasting
exactly as many frames — on the first audio track with room, or a new one.
Nothing is copied or encoded; it is a second node pointing at the same source
(`CLAUDE.md` section 20 rule 1). The speed and the audio chain (gain,
denoise, normalise) go with the sound. The video clip is marked `detached`,
and its own sound is no longer played, so the sound is heard once, from its
new clip.

The two are **linked** (`link`): selecting one selects the other, and moving
or deleting one moves or deletes the other, until **Unlink** separates them.
Linking is explicit and visible in the selection, so a user does not move the
pictures and leave the sound behind without noticing.

Mixing several audio tracks re-encodes the audio at export — expected, and to
be said in the export report (#52); the video stream is still copied.
