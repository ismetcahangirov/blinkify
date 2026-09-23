# Audio monitoring

From issue [#31](https://github.com/ismetcahangirov/blinkify/issues/31), Epic
[#4](https://github.com/ismetcahangirov/blinkify/issues/4). What the editor
hears, how it is measured, and why none of it reaches an export. The clock the
audio drives is [`playback.md`](./playback.md).

```
 track 0 (the pictures' own sound)   track n (sound only: detached audio, music)
        │ decoder per segment                 │ decoder per segment
        ▼                                     ▼
   insertion point (AudioInsert — Epic #7's filter chain attaches here)
        │                                     │
        └──── mixed, if the monitor hears the track (solo, track mute) ────┘
                                   │
                         OutputBuffer (the clock's source)
                                   │ sink takes each sample
                        meter ◄────┤  (peak, short-term loudness, clip)
                                   │ × monitor volume, or 0 when muted
                                   ▼
                        the device, or the silent sink
```

## Two gains that must never be confused

The **monitor volume** is how loud the editor listens. **Clip gain** is an
operation in the edit graph and is written to the file (Epic #7). An editor
that confuses them exports at its monitoring level — a quiet failure nobody
notices until the file is out. So they are different types, in different
places:

- `MonitorVolume` has no conversion to a number. The only thing it does is set
  the gain the output buffer applies as the sink takes each sample, after the
  meter. A `compile_fail` doc test proves it cannot be read back as a gain.
- Track solo and mute decide what the feeder mixes; they are monitor settings
  and not part of the plan.
- The test plays the same programme at full volume, a quarter and muted: what
  is heard scales exactly, and what the meter reads — the programme — is the
  same to a twentieth of a LU in all three.

## The meter

`OutputBuffer` measures every sample the sink takes: what is being _played_,
now, after the insertion point and before the monitor volume.

- **Peak**: sample peak per channel over the last 300 ms, in dBFS. True peak is
  Epic #7's (#46).
- **Short-term loudness**: ITU-R BS.1770-4 — K-weighting (the head's high shelf
  and the RLB high-pass, coefficients computed for the device's actual rate),
  mean square per 100 ms block, the mean of the last 30 blocks, ungated, as EBU
  Tech 3341 defines short-term. The test compares it with FFmpeg's own
  `ebur128` filter on the same file: −23.01 against −23.00 LUFS.
- **Clip**: any sample at or above −0.0009 dBFS — which a full-scale 16-bit
  sample, 32767/32768, reaches — lights the indication, and it stays lit
  through pauses and seeks until the user clicks it.

The renderer reads the meter about fifteen times a second (`monitor_levels`).
A pause or a seek clears the loudness history but not the clip.

## Tracks, gaps and the insertion point

A plan now carries further **sound-only tracks** beside the main segments
(`PlaybackPlan::with_audio_track`). The feeder walks each track as it walked the
main one — its own decoders, started ahead of its own boundaries — and mixes
the tracks the monitor hears. A gap on every track is silence, exactly, and the
clock runs through it.

Every segment's samples pass through `AudioInsert::process` after decoding and
before the mix. It is `PassThrough` until Epic #7 attaches gain, RNNoise and
normalisation there, and nothing assumes a chain is present. Because the meter
sits after it, the user will see what the chain produced.

## Following the default device

Every second the player asks which output device is the default. When it
changes — headphones plugged in, a Bluetooth device connecting — the player
reopens its output on the new default and resumes from the position it had
reached, with a new buffer if the rate differs; playback does not stop. A
device that fails outright is handled the same way (#28), and when there is no
device at all the player plays in silence and says so. The test drives the
change through `DefaultDevice::Given`, because a test cannot change the
operating system's default; `DefaultDevice::System` asks WASAPI through `cpal`.

## What the interface does not have yet

Per-track solo and mute are in the engine and tested with two tracks; the
controls for them belong with the track headers of the timeline (#33, #36). The
player shows the master mute, the monitor volume, the meter and the clip.
