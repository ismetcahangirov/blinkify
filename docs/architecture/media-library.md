# The media library

The library panel (#53) is how media gets into a project: import, browse,
search, and drag onto the timeline. This note covers what import does and does
not do, how a drag crosses from one zone to another, and where the library's
thumbnails come from.

## Import copies nothing

An imported file is a **source reference** in the project (#32): path, size,
modification time and content hash. The file stays where it is. Copying media
into a project folder would be an intermediate file, which `CLAUDE.md` section
20 rule 1 forbids, and a disk the user did not agree to fill.

The shell's `import_media` command (`apps/desktop/src-tauri/src/library.rs`)
does this for each path, one after another, on a worker thread rather than the
window's:

1. **Probe** the file where it is (#23). The engine returns an `AssetInfo`
   (`crates/blinkify-engine/src/project/asset.rs`) with the duration, the
   first real video stream (cover art is skipped), the first audio stream, and
   the sequence settings that would copy the file exactly.
2. **Refuse** what cannot be used, with the reason: a file the probe cannot
   read, a zero-byte file, a file removed between the drop and the probe, or a
   file with neither pictures nor sound. A refused file is never added as a
   broken entry. The refusals come back in the outcome and the panel lists
   them.
3. **Fingerprint** it, opening it read-only (`CLAUDE.md` section 19).
4. **Add** every accepted file to the document as one undoable step. A file
   that is already in the project keeps its source.

An `ImportProgress` event goes out on `library://import` after each file, so a
hundred-file import shows its progress and the window stays responsive.

`crates/blinkify-engine/tests/library.rs` runs these steps against real files.
It asserts that importing and then placing a clip leaves the file tree exactly
as it was. The test covers a non-ASCII folder and file name, and the zero-byte,
not-media and removed-file refusals.

The probe runs once, at import. Everything that reads it later takes the
recorded values from `ProjectView.assets`: the codec badge, the resolution and
the #57 eligibility notice. When a project is opened, the probe of each present
source runs again. That keeps the values true to the file on disk, which may
have been relinked.

## Removing a source

`Edit::RemoveSource` removes a source together with every clip that plays it,
as one entry in the history. Undo brings back the source and its clips. The
panel removes an unused asset immediately and asks first when the timeline
uses it, saying how many clips go with it. The file itself is never touched.

## Missing and changed sources

A source that is not at its path shows as **missing** on its card. The card
also offers a relink, the same `relink_source` from #32: the engine checks
that the chosen file has the same content before pointing the project at it.
A source that has changed in place shows as **changed**. Relinking cannot make
a changed file into the file the edit was made on, so the card offers no
relink for it.

## A drag between zones is data

The library and the timeline are separate React subtrees (#19). A drag from one
to the other must not make the timeline depend on the library's components. So
what travels is data (`apps/desktop/src/library/assetDrag.ts`): the dragged
source ids and the pointer's position. A zone that accepts drops registers a
target with `registerDropTarget` and receives `hover`, `leave` and `drop` calls
as the pointer moves. It learns nothing of the card being dragged.

The drag uses pointer events rather than HTML5 drag-and-drop. The application
window takes the native drag-and-drop for files dropped from the desktop,
because only that gives real file-system paths. On Windows this leaves no HTML5
drag inside the page.

On the timeline, `timeline/assetDrop.ts` works out where the drop would land.
It uses the same ghost, the same snapping (#34: clip edges, the playhead, the
sequence start, 8 px, Alt to suspend) and the same refusals as a clip drag:

- The pointer is not over a track.
- The track is locked.
- A video track receives a file with no pictures, or an audio track receives a
  file with no sound.
- A clip is in the way.

On release, each asset becomes one `AddClip` covering its whole stream, with
the assets placed one after another. Several assets are sent inside one
gesture, so the drop is one undo step. The first clip in a sequence that
matches its first clip sets the sequence's settings (#57, ADR-0008).

A file dropped from the desktop onto the library is imported. A file dropped
onto the timeline is imported and then placed where it was dropped: two steps,
import and place, each undoable.

## Thumbnails come from the filmstrip

A card's picture is a tile from the same filmstrip sprite sheets as the
timeline (#26). `library/libraryMedia.ts` holds a `TimelineMedia` and asks for
a strip at a low density. There is no second thumbnail path. Because the engine
caches strips by content, the timeline finds them already made.

Asking is also what starts the work. When a card appears, it asks for its
thumbnail and its waveform (#25). The engine generates both at background
priority (#22), and the card says _Preparing…_ until both are ready.

## Eligibility at the point of import

A source whose pictures cannot be stream-copied into the sequence (#57) says
so on its card. Otherwise the first the user would hear of the re-encode is a
mark on the timeline after the drop. When the source's shape is one a sequence
can have, the card offers **Match sequence…**. That option first shows the
`preview_settings` impact statement: which clips would gain a lossless copy
and which would lose it. Only then does it apply `SetSettings`, so no clip
loses its copy silently (`CLAUDE.md` section 20 rule 3).

## What is not here yet

- The tabs other than Media (Audio, Text, Stickers, Effects, Transitions,
  Filters) are present but disabled. They are there so the layout does not
  change when they are filled.
- The library orders assets by name. Folders, bins and sorting by date or
  length are not part of v1.
