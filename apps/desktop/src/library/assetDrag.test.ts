import { describe, expect, it, vi } from "vitest";
import {
  registerDropTarget,
  startAssetDrag,
  type DropTarget,
} from "./assetDrag.js";

const target = (left: number): DropTarget & Record<string, unknown> => ({
  contains: (p) => p.x >= left && p.x < left + 100,
  hover: vi.fn(),
  leave: vi.fn(),
  drop: vi.fn(),
});

const PLAIN = { alt: false };

describe("a drag of library assets", () => {
  it("tells the target under the pointer, and only it", () => {
    const a = target(0);
    const b = target(200);
    const stopA = registerDropTarget(a);
    const stopB = registerDropTarget(b);
    const drag = startAssetDrag({ sources: [3] });
    drag.move({ x: 50, y: 0 }, PLAIN);
    expect(a.hover).toHaveBeenCalledWith(
      { sources: [3] },
      { x: 50, y: 0 },
      PLAIN,
    );
    drag.move({ x: 250, y: 0 }, PLAIN);
    expect(a.leave).toHaveBeenCalledOnce();
    expect(b.hover).toHaveBeenCalledOnce();
    expect(drag.release({ x: 260, y: 0 }, PLAIN)).toBe(true);
    expect(b.drop).toHaveBeenCalledWith(
      { sources: [3] },
      { x: 260, y: 0 },
      PLAIN,
    );
    expect(a.drop).not.toHaveBeenCalled();
    stopA();
    stopB();
  });

  it("drops nothing when released over no target, or abandoned", () => {
    const a = target(0);
    const stop = registerDropTarget(a);
    const drag = startAssetDrag({ sources: [1] });
    expect(drag.release({ x: 500, y: 0 }, PLAIN)).toBe(false);
    const again = startAssetDrag({ sources: [1] });
    again.move({ x: 10, y: 0 }, PLAIN);
    again.cancel();
    expect(a.leave).toHaveBeenCalled();
    expect(a.drop).not.toHaveBeenCalled();
    stop();
    // Unregistered: no longer told anything.
    startAssetDrag({ sources: [1] }).release({ x: 10, y: 0 }, PLAIN);
    expect(a.drop).not.toHaveBeenCalled();
  });
});
