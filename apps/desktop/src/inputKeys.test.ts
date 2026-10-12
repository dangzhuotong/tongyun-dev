import { describe, expect, it } from "vitest";
import {
  mapEmptyInputKey,
  QUICK_INPUT_BUTTONS,
  resolveKeyboardInput,
} from "./inputKeys";

describe("mapEmptyInputKey (空输入框按键映射)", () => {
  it("maps arrow keys to terminal key names", () => {
    expect(mapEmptyInputKey("ArrowUp")).toBe("up");
    expect(mapEmptyInputKey("ArrowDown")).toBe("down");
    expect(mapEmptyInputKey("ArrowLeft")).toBe("left");
    expect(mapEmptyInputKey("ArrowRight")).toBe("right");
  });

  it("maps Tab, Escape, and Enter", () => {
    expect(mapEmptyInputKey("Tab")).toBe("tab");
    expect(mapEmptyInputKey("Escape")).toBe("esc");
    expect(mapEmptyInputKey("Esc")).toBe("esc");
    expect(mapEmptyInputKey("Enter")).toBe("enter");
  });

  it("maps Ctrl-C or Cmd-C", () => {
    expect(mapEmptyInputKey("c", { ctrlKey: true })).toBe("ctrl-c");
    expect(mapEmptyInputKey("C", { ctrlKey: true })).toBe("ctrl-c");
    expect(mapEmptyInputKey("c", { metaKey: true })).toBe("ctrl-c");
  });

  it("returns null for non-mapped keys", () => {
    expect(mapEmptyInputKey("a")).toBeNull();
    expect(mapEmptyInputKey(" ")).toBeNull();
    expect(mapEmptyInputKey("Backspace")).toBeNull();
  });
});

describe("resolveKeyboardInput (按键与输入框文本解析)", () => {
  it("when input is empty, special keys send key payload", () => {
    expect(resolveKeyboardInput({ key: "ArrowUp" }, "")).toEqual({ key: "up" });
    expect(resolveKeyboardInput({ key: "ArrowDown" }, "")).toEqual({ key: "down" });
    expect(resolveKeyboardInput({ key: "ArrowLeft" }, "")).toEqual({ key: "left" });
    expect(resolveKeyboardInput({ key: "ArrowRight" }, "")).toEqual({ key: "right" });
    expect(resolveKeyboardInput({ key: "Tab" }, "")).toEqual({ key: "tab" });
    expect(resolveKeyboardInput({ key: "Escape" }, "")).toEqual({ key: "esc" });
    expect(resolveKeyboardInput({ key: "Enter" }, "")).toEqual({ key: "enter" });
  });

  it("when input has text, Enter sends text with enter = true", () => {
    expect(resolveKeyboardInput({ key: "Enter" }, "pnpm run build")).toEqual({
      text: "pnpm run build",
      enter: true,
    });
  });

  it("when input has text, arrow keys return null so user can navigate cursor", () => {
    expect(resolveKeyboardInput({ key: "ArrowLeft" }, "hello")).toBeNull();
    expect(resolveKeyboardInput({ key: "ArrowRight" }, "hello")).toBeNull();
    expect(resolveKeyboardInput({ key: "ArrowUp" }, "hello")).toBeNull();
  });

  it("Ctrl-C always sends interrupt payload regardless of text", () => {
    expect(resolveKeyboardInput({ key: "c", ctrlKey: true }, "")).toEqual({
      key: "ctrl-c",
    });
    expect(resolveKeyboardInput({ key: "c", ctrlKey: true }, "running script")).toEqual({
      key: "ctrl-c",
    });
  });
});

describe("QUICK_INPUT_BUTTONS (快捷按钮定义)", () => {
  it("contains all required buttons with correct payloads", () => {
    const labels = QUICK_INPUT_BUTTONS.map((b) => b.label);
    expect(labels).toEqual(["↑", "↓", "回车", "y", "n", "Ctrl-C"]);

    const upBtn = QUICK_INPUT_BUTTONS.find((b) => b.label === "↑");
    expect(upBtn?.payload).toEqual({ key: "up" });

    const downBtn = QUICK_INPUT_BUTTONS.find((b) => b.label === "↓");
    expect(downBtn?.payload).toEqual({ key: "down" });

    const enterBtn = QUICK_INPUT_BUTTONS.find((b) => b.label === "回车");
    expect(enterBtn?.payload).toEqual({ key: "enter" });

    const yBtn = QUICK_INPUT_BUTTONS.find((b) => b.label === "y");
    expect(yBtn?.payload).toEqual({ text: "y", enter: true });

    const nBtn = QUICK_INPUT_BUTTONS.find((b) => b.label === "n");
    expect(nBtn?.payload).toEqual({ text: "n", enter: true });

    const ctrlCBtn = QUICK_INPUT_BUTTONS.find((b) => b.label === "Ctrl-C");
    expect(ctrlCBtn?.payload).toEqual({ key: "ctrl-c" });
  });
});
