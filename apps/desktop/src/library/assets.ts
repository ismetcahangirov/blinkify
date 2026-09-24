import type { AssetInfo, ProjectView, SourceStatus } from "@blinkify/types";
import { fileName, type DeepReadonly } from "../project/project.store.js";

/**
 * The media library's view of the project's sources (#53). Pure: the library
 * shows what the engine reported at import — the probe's values, verbatim —
 * and nothing here re-reads a file.
 */

export type AssetKind = "video" | "audio";

export interface Asset {
  readonly id: number;
  readonly path: string;
  readonly name: string;
  readonly info: DeepReadonly<AssetInfo> | null;
  readonly kind: AssetKind;
  /** Offline or changed since import (#32): shown, with a relink. */
  readonly status: DeepReadonly<SourceStatus> | null;
  /** Whether its pictures can be copied into the sequence (#57). */
  readonly eligible: boolean | null;
  /** How many clips play it: what removing it takes with it. */
  readonly uses: number;
}

type View = DeepReadonly<ProjectView>;

export function assetsOf(view: View | null): Asset[] {
  if (!view) return [];
  const uses = new Map<number, number>();
  for (const track of view.project.sequence.tracks)
    for (const clip of track.clips)
      uses.set(clip.source, (uses.get(clip.source) ?? 0) + 1);
  return Object.entries(view.project.sources)
    .flatMap(([key, source]): Asset[] => {
      if (!source) return [];
      const id = Number(key);
      const info = view.assets[id] ?? null;
      return [
        {
          id,
          path: source.path,
          name: fileName(source.path),
          info,
          kind: info?.video ? "video" : "audio",
          status: view.unavailable[id] ?? null,
          eligible: view.eligibility[id]?.eligible ?? null,
          uses: uses.get(id) ?? 0,
        },
      ];
    })
    .sort((a, b) => a.name.localeCompare(b.name));
}

/**
 * A name as a search compares it: case and accents folded, so "baki" finds
 * "Bakı" and "cekilis" finds "Çəkiliş" — the dotless ı has no decomposition,
 * so it is folded by hand.
 */
export function searchable(text: string): string {
  return text
    .normalize("NFD")
    .replace(/\p{M}/gu, "")
    .toLowerCase()
    .replace(/ı/g, "i")
    .replace(/ə/g, "e");
}

/** The assets matching a search and a kind. */
export function filterAssets(
  assets: readonly Asset[],
  query: string,
  kind: AssetKind | "all",
): Asset[] {
  const needle = searchable(query.trim());
  return assets.filter(
    (asset) =>
      (kind === "all" || asset.kind === kind) &&
      (needle === "" || searchable(asset.name).includes(needle)),
  );
}

/** `1:42`, `1:02:03`, or a dash for no duration. */
export function duration(seconds: number | null | undefined): string {
  if (seconds === null || seconds === undefined || !Number.isFinite(seconds))
    return "—";
  const whole = Math.round(seconds);
  const h = Math.floor(whole / 3600);
  const m = Math.floor((whole % 3600) / 60);
  const s = whole % 60;
  const pad = (n: number) => String(n).padStart(2, "0");
  return h > 0 ? `${h}:${pad(m)}:${pad(s)}` : `${m}:${pad(s)}`;
}

const CODEC_NAMES: Record<string, string> = {
  h264: "H.264",
  hevc: "HEVC",
  av1: "AV1",
  vp9: "VP9",
  prores: "ProRes",
  mjpeg: "MJPEG",
  aac: "AAC",
  mp3: "MP3",
  opus: "Opus",
  flac: "FLAC",
  pcm_s16le: "PCM",
  pcm_s24le: "PCM",
};

/** The codec badge: the video codec's name, or the sound's for audio. */
export function codecBadge(info: DeepReadonly<AssetInfo> | null): string {
  const codec = info?.video?.codec ?? info?.audio?.codec ?? null;
  if (!codec) return "?";
  return CODEC_NAMES[codec] ?? codec.toUpperCase();
}

/** `3840×2160` as displayed, or nothing for sound. */
export function resolution(info: DeepReadonly<AssetInfo> | null): string {
  const video = info?.video;
  return video ? `${video.displayWidth}×${video.displayHeight}` : "";
}
