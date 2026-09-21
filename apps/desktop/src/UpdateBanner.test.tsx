import { render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { invoke } from "@tauri-apps/api/core";
import { beforeEach, describe, expect, it, vi } from "vitest";

import { UpdateBanner } from "./UpdateBanner.js";
import { useUpdateStore } from "./update.store.js";

vi.mock("@tauri-apps/api/core", () => ({ invoke: vi.fn() }));

const invoked = vi.mocked(invoke);

const OFFER = {
  version: "0.2.0",
  currentVersion: "0.1.0",
  notes: null,
  publishedAt: null,
};

beforeEach(() => {
  invoked.mockReset();
  useUpdateStore.setState({ status: "idle", offer: null, error: null });
});

describe("UpdateBanner", () => {
  it("renders nothing when there is no update", async () => {
    invoked.mockResolvedValue(null);
    render(<UpdateBanner />);
    expect(
      await screen.findByTestId("update-banner").catch(() => null),
    ).toBeNull();
    expect(screen.queryByTestId("update-banner")).toBeNull();
  });

  it("renders nothing when the check failed", async () => {
    // No network, no release yet, or a manifest that failed verification. None
    // of those is the user's problem at launch, and none of them should put an
    // error in front of somebody who only wanted to open the application.
    invoked.mockRejectedValue(new Error("network unreachable"));
    render(<UpdateBanner />);
    await vi.waitFor(() => {
      expect(invoked).toHaveBeenCalledWith("pending_update");
    });
    expect(screen.queryByTestId("update-banner")).toBeNull();
  });

  it("names both versions rather than saying an update is available", async () => {
    invoked.mockResolvedValue(OFFER);
    render(<UpdateBanner />);

    const banner = await screen.findByTestId("update-banner");
    expect(banner).toHaveTextContent("0.2.0");
    expect(banner).toHaveTextContent("0.1.0");
  });

  it("does not install anything without a deliberate action", async () => {
    // Forbidden behaviour 3, as an assertion. Mounting the banner asks what the
    // launch check found and stops there; an install that starts on its own
    // would replace a working build mid-edit.
    invoked.mockResolvedValue(OFFER);
    render(<UpdateBanner />);
    await screen.findByTestId("update-banner");

    expect(invoked).toHaveBeenCalledTimes(1);
    expect(invoked).toHaveBeenCalledWith("pending_update");
    expect(invoked).not.toHaveBeenCalledWith("install_update");
  });

  it("installs when the user asks for it", async () => {
    invoked.mockResolvedValue(OFFER);
    render(<UpdateBanner />);
    await screen.findByTestId("update-banner");

    invoked.mockImplementation(() => new Promise(() => {}));
    await userEvent.click(
      screen.getByRole("button", { name: "Install and restart" }),
    );

    expect(invoked).toHaveBeenCalledWith("install_update");
  });

  it("says so, and says Blinkify is unchanged, when the install fails", async () => {
    invoked.mockResolvedValue(OFFER);
    render(<UpdateBanner />);
    await screen.findByTestId("update-banner");

    invoked.mockRejectedValue(
      new Error("the update could not be installed: signature"),
    );
    await userEvent.click(
      screen.getByRole("button", { name: "Install and restart" }),
    );

    const error = await screen.findByTestId("update-error");
    expect(error).toHaveTextContent("signature");
    expect(error).toHaveTextContent("Blinkify is unchanged");
  });

  it("stays dismissed once the user says no", async () => {
    invoked.mockResolvedValue(OFFER);
    const { unmount } = render(<UpdateBanner />);
    await screen.findByTestId("update-banner");

    await userEvent.click(screen.getByRole("button", { name: "Not now" }));
    expect(screen.queryByTestId("update-banner")).toBeNull();

    // Remounting must not resurrect it. "Not now" that becomes "again in ten
    // seconds" is how a user learns to stop reading the banner.
    unmount();
    render(<UpdateBanner />);
    await vi.waitFor(() => {
      expect(useUpdateStore.getState().status).toBe("dismissed");
    });
    expect(screen.queryByTestId("update-banner")).toBeNull();
  });
});
