import { describe, expect, it } from "vitest";
import { cursorUri } from "./cursorUri";

describe("cursorUri", () => {
  it("formats POSIX path correctly", () => {
    expect(cursorUri("/Users/demo/project")).toBe("cursor://file/Users/demo/project");
  });

  it("normalizes Windows backslashes and drive letters", () => {
    expect(cursorUri("C:\\code\\demo")).toBe("cursor://file/C:/code/demo");
    expect(cursorUri("D:/workspace/proj")).toBe("cursor://file/D:/workspace/proj");
  });

  it("encodes whitespace for safe URI", () => {
    expect(cursorUri("/Users/demo/my project")).toBe("cursor://file/Users/demo/my%20project");
    expect(cursorUri("C:\\My Documents\\app")).toBe("cursor://file/C:/My%20Documents/app");
  });

  it("handles empty or null or undefined input", () => {
    expect(cursorUri("")).toBe("");
    expect(cursorUri("   ")).toBe("");
    expect(cursorUri(null)).toBe("");
    expect(cursorUri(undefined)).toBe("");
  });
});
