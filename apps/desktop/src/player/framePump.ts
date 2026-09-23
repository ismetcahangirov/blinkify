import { parseWireFrame, type WireFrame } from "./frameWire.js";

/**
 * Pull preview frames from the engine, one request at a time.
 *
 * The engine decides which frame is due; this only asks "anything newer than
 * the last one I drew?" and draws the answer. At most one request is ever in
 * flight, and the next is sent only once the previous frame has been handed to
 * `onFrame`, so a renderer that falls behind — a throttled window, a busy main
 * thread — is simply shown fewer, newer frames. Nothing queues for it, in
 * either process (#27).
 *
 * The pacing is the engine's: it holds a request open until a newer frame
 * falls due. So the next request goes out as soon as a frame is drawn, not on
 * the next animation frame — waiting for a paint as well would add up to a
 * display interval to every cycle, and at 30 fps that is what turns a steady
 * picture into a dropped frame every second or two.
 */
export interface FramePumpOptions {
  /** The URL of the frame after `after`: see `frameUrl`. */
  url: (after: number) => string;
  fetch: (url: string, init: { signal: AbortSignal }) => Promise<Response>;
  onFrame: (frame: WireFrame) => void;
  /** Called when the session is gone or a response is unusable. */
  onError: (error: Error) => void;
  /** How long to back off after an error before asking again. */
  retryMs?: number;
}

/** No newer frame became due while the engine held the request open. */
const NO_CONTENT = 204;
/** The session was closed, or never existed. */
const NOT_FOUND = 404;

/** Start pulling frames. Returns the function that stops it. */
export function startFramePump(options: FramePumpOptions): () => void {
  const controller = new AbortController();
  const { signal } = controller;
  const retryMs = options.retryMs ?? 250;

  const run = async () => {
    let after = 0;
    while (!signal.aborted) {
      try {
        const response = await options.fetch(options.url(after), { signal });
        if (response.status === NO_CONTENT) continue;
        if (response.status === NOT_FOUND) {
          options.onError(new Error("the preview session has closed"));
          return;
        }
        if (!response.ok) {
          throw new Error(
            `frame request failed with ${String(response.status)}`,
          );
        }
        const frame = parseWireFrame(await response.arrayBuffer());
        after = frame.seq;
        options.onFrame(frame);
      } catch (error) {
        if (signal.aborted) return;
        options.onError(
          error instanceof Error ? error : new Error(String(error)),
        );
        await new Promise((resolve) => setTimeout(resolve, retryMs));
      }
    }
  };
  void run();
  return () => {
    controller.abort();
  };
}

/**
 * The URL of the frame after `after` in `session`, given the base URL of the
 * engine's `frame` scheme (`convertFileSrc("", "frame")`).
 */
export function frameUrl(base: string, session: number, after: number): string {
  const root = base.endsWith("/") ? base : `${base}/`;
  return `${root}${String(session)}/${String(after)}`;
}
