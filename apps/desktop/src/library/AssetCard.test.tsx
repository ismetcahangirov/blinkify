import { render, screen } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";
import { AssetCard } from "./AssetCard.js";
import type { Asset } from "./assets.js";
import { waveformFailure, waveformReady } from "./libraryMedia.js";

vi.mock("./libraryMedia.js", () => ({
  cardTile: () => null,
  waveformReady: vi.fn(() => false),
  waveformFailure: vi.fn(() => null),
  onLibraryMedia: () => () => undefined,
}));

const SONG: Asset = {
  id: 1,
  path: "C:\\media\\song.mp3",
  name: "song.mp3",
  info: {
    durationSeconds: 180,
    video: null,
    audio: { stream: 0, codec: "mp3", sampleRate: 44100, channels: 2 },
    matching: null,
  },
  kind: "audio",
  status: null,
  eligible: null,
  uses: 0,
};

function card() {
  return render(
    <AssetCard
      asset={SONG}
      selected={false}
      onPointerDown={() => undefined}
      onRelink={() => undefined}
      onRemove={() => undefined}
      onMatch={() => undefined}
    />,
  );
}

describe("an asset card's waveform (#152)", () => {
  beforeEach(() => {
    vi.mocked(waveformReady).mockReturnValue(false);
    vi.mocked(waveformFailure).mockReturnValue(null);
  });

  it("says it is preparing while the waveform is being made", () => {
    card();
    expect(screen.getByTestId("asset-preparing")).toBeTruthy();
    expect(screen.queryByTestId("asset-no-waveform")).toBeNull();
  });

  it("stops saying it is preparing once the engine gives up, and says why", () => {
    vi.mocked(waveformFailure).mockReturnValue(
      "could not decode the audio: the sidecar exited with an error",
    );
    card();
    expect(screen.queryByTestId("asset-preparing")).toBeNull();
    const notice = screen.getByTestId("asset-no-waveform");
    expect(notice.textContent).toContain("No waveform");
    expect(notice.getAttribute("title")).toContain(
      "could not decode the audio",
    );
  });
});
