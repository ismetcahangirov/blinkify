import { invoke } from "@tauri-apps/api/core";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import {
  ENGINE_POLL_MS,
  engineLabel,
  followEngine,
  useShellStore,
} from "./shell.store.js";

vi.mock("@tauri-apps/api/core", () => ({ invoke: vi.fn() }));

const flush = () => vi.advanceTimersByTimeAsync(0);
const status = () => useShellStore.getState().engineStatus;

describe("the engine status (#151)", () => {
  beforeEach(() => {
    vi.useFakeTimers();
    useShellStore.setState({
      engineStatus: "not-connected",
      engineError: null,
    });
  });
  afterEach(() => {
    vi.useRealTimers();
    vi.mocked(invoke).mockReset();
  });

  it("is starting while the encoder probe runs, and ready once it is done", async () => {
    const answers: unknown[] = [null, null, { codecs: [] }];
    vi.mocked(invoke).mockImplementation(() =>
      Promise.resolve(answers.shift()),
    );
    const stop = followEngine();
    expect(status()).toBe("connecting");
    await flush();
    expect(status()).toBe("connecting");
    await vi.advanceTimersByTimeAsync(ENGINE_POLL_MS);
    expect(status()).toBe("connecting");
    await vi.advanceTimersByTimeAsync(ENGINE_POLL_MS);
    expect(status()).toBe("ready");
    expect(invoke).toHaveBeenCalledTimes(3);
    expect(invoke).toHaveBeenCalledWith("encoder_capabilities");
    // Ready: nothing more is asked.
    await vi.advanceTimersByTimeAsync(ENGINE_POLL_MS * 4);
    expect(invoke).toHaveBeenCalledTimes(3);
    stop();
  });

  it("fails, with the engine's reason, when the sidecar is missing", async () => {
    vi.mocked(invoke).mockRejectedValue(
      "the FFmpeg sidecar is missing from the installation",
    );
    followEngine();
    await flush();
    expect(status()).toBe("failed");
    expect(useShellStore.getState().engineError).toBe(
      "the FFmpeg sidecar is missing from the installation",
    );
    await vi.advanceTimersByTimeAsync(ENGINE_POLL_MS * 4);
    expect(invoke).toHaveBeenCalledTimes(1);
  });

  it("stops asking once stopped", async () => {
    vi.mocked(invoke).mockResolvedValue(null);
    const stop = followEngine();
    await flush();
    stop();
    await vi.advanceTimersByTimeAsync(ENGINE_POLL_MS * 4);
    expect(invoke).toHaveBeenCalledTimes(1);
  });

  it("says each state in words, never as the raw value", () => {
    expect(engineLabel("not-connected", null)).toBe("Engine: not started");
    expect(engineLabel("connecting", null)).toBe("Engine: starting…");
    expect(engineLabel("ready", null)).toBe("Engine: ready");
    expect(engineLabel("failed", "the sidecar is missing")).toBe(
      "Engine: unavailable (the sidecar is missing)",
    );
    expect(engineLabel("failed", null)).toBe("Engine: unavailable");
  });
});
