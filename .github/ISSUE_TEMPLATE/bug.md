---
name: Bug
about: Something is broken. Media bugs need the source file's characteristics.
title: ""
labels: "type:bug, status:ready"
assignees: "ismetcahangirov"
---

## What happened

<!-- One or two sentences. -->

## Steps to reproduce

1.
2.
3.

## Expected

## Actual

## Source file characteristics

<!-- Fill this in for ANY bug involving a media file — import, preview,
     timeline, export. Most video-editor bugs are unreproducible without it,
     and "an mp4 from my phone" is four unknowns, not one.
     `ffprobe -hide_banner <file>` prints most of this. Delete the section
     only if no media file is involved at all. -->

- **Container**: <!-- mp4, mov, mkv, … -->
- **Video codec and profile**: <!-- h264 High 4.2, hevc Main 10, … -->
- **Audio codec**: <!-- aac, pcm_s16le, … -->
- **Resolution and frame rate**:
- **Variable or constant frame rate**: <!-- VFR is the usual cause. If unsure, say unsure. -->
- **Came off a phone?**: <!-- Which phone, if so. Phone captures are VFR and often rotated. -->
- **Rotation metadata**:

## Export tier, if this is an export bug

<!-- What the export dialog said before you started: copied, smart-cut, or
     re-encoded — and what the report said afterwards. A mismatch between the
     two is itself the bug. -->

## Environment

- **Blinkify version**:
- **Windows version**:
- **GPU and driver version**: <!-- Decides which encoders exist. See ADR-0003. -->

## Logs or screenshots
