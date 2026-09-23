import { describe, expect, it, vi } from "vitest";

import { frameUrl, startFramePump } from "./framePump.js";
import { WIRE_HEADER_BYTES } from "./frameWire.js";

function frameResponse(seq: number): Response {
  const buffer = new ArrayBuffer(WIRE_HEADER_BYTES + 4);
  const view = new DataView(buffer);
  new Uint8Array(buffer, 0, 4).set([0x42, 0x4b, 0x46, 0x32]);
  view.setUint32(4, 1, true);
  view.setUint32(8, 1, true);
  view.setBigUint64(16, BigInt(seq), true);
  return new Response(buffer, { status: 200 });
}

const settle = () => new Promise((resolve) => setTimeout(resolve, 0));

describe("frameUrl", () => {
  it("names the session and the last frame drawn", () => {
    expect(frameUrl("http://frame.localhost/", 3, 41)).toBe(
      "http://frame.localhost/3/41",
    );
    expect(frameUrl("frame://localhost", 3, 0)).toBe("frame://localhost/3/0");
  });
});

describe("startFramePump", () => {
  it("never has more than one request in flight", async () => {
    const pending: Array<{ url: string; respond: (r: Response) => void }> = [];
    const fetch = vi.fn(
      (url: string) =>
        new Promise<Response>((respond) => pending.push({ url, respond })),
    );
    const frames: number[] = [];
    const stop = startFramePump({
      url: (after) => `f/${String(after)}`,
      fetch,
      onFrame: (frame) => frames.push(frame.seq),
      onError: () => undefined,
    });

    await settle();
    await settle();
    // The engine has not answered: still exactly one request.
    expect(pending.map((p) => p.url)).toEqual(["f/0"]);
    pending[0]?.respond(frameResponse(1));
    await settle();
    expect(frames).toEqual([1]);
    // Drawn, so the next request asks for anything after it — and only then.
    expect(pending.map((p) => p.url)).toEqual(["f/0", "f/1"]);
    stop();
  });

  it("asks again at once when no newer frame was due", async () => {
    const urls: string[] = [];
    let calls = 0;
    const stop = startFramePump({
      url: (after) => `f/${String(after)}`,
      fetch: (url) => {
        urls.push(url);
        calls += 1;
        return Promise.resolve(
          calls === 1 ? new Response(null, { status: 204 }) : frameResponse(9),
        );
      },
      onFrame: () => {
        stop();
      },
      onError: () => undefined,
    });
    await settle();
    await settle();
    expect(urls.slice(0, 2)).toEqual(["f/0", "f/0"]);
  });

  it("stops and reports when the session has closed", async () => {
    const onError = vi.fn();
    const fetch = vi.fn(() =>
      Promise.resolve(new Response(null, { status: 404 })),
    );
    startFramePump({
      url: () => "f",
      fetch,
      onFrame: () => undefined,
      onError,
    });
    await settle();
    await settle();
    expect(onError).toHaveBeenCalledWith(
      expect.objectContaining({ message: "the preview session has closed" }),
    );
    expect(fetch).toHaveBeenCalledTimes(1);
  });

  it("aborts the request in flight when stopped", async () => {
    let aborted = false;
    const stop = startFramePump({
      url: () => "f",
      fetch: (_url, { signal }) =>
        new Promise<Response>(() => {
          signal.addEventListener("abort", () => {
            aborted = true;
          });
        }),
      onFrame: () => undefined,
      onError: () => undefined,
    });
    await settle();
    stop();
    expect(aborted).toBe(true);
  });
});
