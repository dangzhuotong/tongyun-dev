import { describe, expect, it } from "vitest";
import { effectiveStartCommand } from "./startCommand";

describe("effectiveStartCommand", () => {
  const project = {
    start_command: "pnpm dev --host 127.0.0.1 --port 5173",
    profile_start_commands: {
      dev: "pnpm dev --host 127.0.0.1 --port 5173",
      mock: "pnpm dev:mock --port 5174",
    },
  };

  it("uses the assembled line for the selected profile", () => {
    expect(effectiveStartCommand(project, "mock")).toBe("pnpm dev:mock --port 5174");
  });

  it("falls back to the default assembled command when no profile is selected", () => {
    expect(effectiveStartCommand(project, "")).toBe(project.start_command);
    expect(effectiveStartCommand(project, "   ")).toBe(project.start_command);
    expect(effectiveStartCommand(project, null)).toBe(project.start_command);
  });

  it("falls back when the profile has no assembled line", () => {
    expect(effectiveStartCommand(project, "prod")).toBe(project.start_command);
  });

  it("returns empty when there is no project", () => {
    expect(effectiveStartCommand(null, "mock")).toBe("");
  });
});
