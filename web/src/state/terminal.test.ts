import { describe, it, expect, vi, beforeEach, afterEach } from "vitest";
import {
  getScreen,
  getScreenSignal,
  setFullScreen,
  updateScreen,
  updateLine,
  removeScreen,
  batchUpdateLine,
  batchUpdateScreen,
  flushBatchedUpdates,
} from "./terminal";
import type { ScreenState } from "./terminal";

function makeScreen(overrides: Partial<ScreenState> = {}): ScreenState {
  return {
    lines: ["line0", "line1", "line2"],
    cursor: { row: 0, col: 0, visible: true },
    alternateActive: false,
    cols: 80,
    rows: 3,
    firstLineIndex: 0,
    totalLines: 3,
    scrollbackLines: [],
    scrollbackOffset: 0,
    scrollbackComplete: false,
    scrollbackLoading: false,
    overlays: [],
    panels: [],
    ...overrides,
  };
}

beforeEach(() => {
  // Mock requestAnimationFrame to capture scheduled callbacks
  vi.stubGlobal("requestAnimationFrame", (cb: FrameRequestCallback) => {
    return setTimeout(() => cb(0), 0) as unknown as number;
  });
  vi.stubGlobal("cancelAnimationFrame", (id: number) => clearTimeout(id));
});

afterEach(() => {
  // Clean up session signals between tests
  removeScreen("test");
  removeScreen("test2");
  vi.restoreAllMocks();
});

// ---------------------------------------------------------------------------
// Synchronous updateLine / updateScreen (unchanged behavior, regression tests)
// ---------------------------------------------------------------------------

describe("updateLine (synchronous)", () => {
  it("updates a single line immediately", () => {
    setFullScreen("test", makeScreen());
    updateLine("test", 1, "updated");
    const s = getScreen("test");
    expect(s.lines[1]).toBe("updated");
    // Other lines unchanged
    expect(s.lines[0]).toBe("line0");
    expect(s.lines[2]).toBe("line2");
  });

  it("ignores out-of-range index", () => {
    setFullScreen("test", makeScreen());
    updateLine("test", 5, "nope");
    const s = getScreen("test");
    expect(s.lines).toEqual(["line0", "line1", "line2"]);
  });

  it("pads with empty lines when index exceeds current length", () => {
    setFullScreen("test", makeScreen({ lines: ["a"], rows: 4 }));
    updateLine("test", 3, "at-3");
    const s = getScreen("test");
    expect(s.lines).toEqual(["a", "", "", "at-3"]);
  });
});

describe("updateScreen (synchronous)", () => {
  it("merges partial updates into screen state", () => {
    setFullScreen("test", makeScreen());
    updateScreen("test", { totalLines: 100 });
    expect(getScreen("test").totalLines).toBe(100);
    // lines untouched
    expect(getScreen("test").lines).toEqual(["line0", "line1", "line2"]);
  });
});

// ---------------------------------------------------------------------------
// Batched updates
// ---------------------------------------------------------------------------

describe("batchUpdateLine", () => {
  it("does not apply immediately", () => {
    setFullScreen("test", makeScreen());
    batchUpdateLine("test", 0, "batched");
    // Should still see old value before flush
    expect(getScreen("test").lines[0]).toBe("line0");
  });

  it("applies on flushBatchedUpdates", () => {
    setFullScreen("test", makeScreen());
    batchUpdateLine("test", 0, "batched0");
    batchUpdateLine("test", 2, "batched2");
    flushBatchedUpdates();
    const s = getScreen("test");
    expect(s.lines[0]).toBe("batched0");
    expect(s.lines[1]).toBe("line1"); // untouched
    expect(s.lines[2]).toBe("batched2");
  });

  it("last write wins for same index", () => {
    setFullScreen("test", makeScreen());
    batchUpdateLine("test", 0, "first");
    batchUpdateLine("test", 0, "second");
    flushBatchedUpdates();
    expect(getScreen("test").lines[0]).toBe("second");
  });
});

describe("batchUpdateScreen", () => {
  it("does not apply immediately", () => {
    setFullScreen("test", makeScreen());
    batchUpdateScreen("test", { totalLines: 999 });
    expect(getScreen("test").totalLines).toBe(3);
  });

  it("applies on flush", () => {
    setFullScreen("test", makeScreen());
    batchUpdateScreen("test", { totalLines: 999, scrollbackComplete: true });
    flushBatchedUpdates();
    expect(getScreen("test").totalLines).toBe(999);
    expect(getScreen("test").scrollbackComplete).toBe(true);
  });

  it("merges multiple partial updates", () => {
    setFullScreen("test", makeScreen());
    batchUpdateScreen("test", { totalLines: 50 });
    batchUpdateScreen("test", { scrollbackComplete: true });
    flushBatchedUpdates();
    expect(getScreen("test").totalLines).toBe(50);
    expect(getScreen("test").scrollbackComplete).toBe(true);
  });
});

describe("combined batch line + screen updates", () => {
  it("applies both in a single signal write", () => {
    setFullScreen("test", makeScreen());
    const sig = getScreenSignal("test");
    let writeCount = 0;
    // Track how many times the signal value changes
    const original = sig.value;
    Object.defineProperty(sig, "value", {
      get() {
        return this._v ?? original;
      },
      set(v) {
        writeCount++;
        this._v = v;
      },
      configurable: true,
    });
    // Reset after our defineProperty set it once
    writeCount = 0;

    batchUpdateLine("test", 0, "new-line");
    batchUpdateScreen("test", { totalLines: 42 });
    flushBatchedUpdates();

    // Should have written signal exactly once
    expect(writeCount).toBe(1);
  });
});

describe("multi-session batching", () => {
  it("flushes updates for multiple sessions", () => {
    setFullScreen("test", makeScreen());
    setFullScreen("test2", makeScreen());
    batchUpdateLine("test", 0, "s1-line");
    batchUpdateScreen("test2", { totalLines: 77 });
    flushBatchedUpdates();
    expect(getScreen("test").lines[0]).toBe("s1-line");
    expect(getScreen("test2").totalLines).toBe(77);
  });
});

describe("flushBatchedUpdates when nothing pending", () => {
  it("does not throw and does not mutate state", () => {
    setFullScreen("test", makeScreen());
    const before = getScreen("test");
    flushBatchedUpdates();
    const after = getScreen("test");
    // Same reference — no write occurred
    expect(after).toBe(before);
  });
});
