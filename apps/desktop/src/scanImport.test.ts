import { describe, expect, it } from "vitest";
import { pickedRows, rowProblems, toImportItems, toRows, TOMCAT_REASON } from "./scanImport";
import type { ScanCandidate } from "./types";

const C = (over: Partial<ScanCandidate>): ScanCandidate => ({
  id: "demo-web",
  name: "demo-web",
  root: "~/code/demo-web",
  kind: "vite",
  label: "Vite + Vue",
  confidence: "high",
  start: "pnpm dev",
  stop: null,
  port: 5173,
  port_guessed: true,
  health_url: "http://127.0.0.1:5173/",
  login_url: null,
  java: null,
  node: "20",
  background: true,
  needs_tomcat: false,
  why: [],
  existing_id: null,
  ...over,
});

describe("scan import", () => {
  it("pre-picks only new rows", () => {
    const rows = toRows([C({}), C({ id: "api", existing_id: "api" })]);
    expect(rows.map((r) => r.picked)).toEqual([true, false]);
    rows[1].picked = true;
    expect(pickedRows(rows).map((r) => r.id)).toEqual(["demo-web"]);
  });

  it("flags bad ids, duplicates, empty start and missing port", () => {
    const rows = toRows([
      C({ id: "ok" }),
      C({ id: "ok" }),
      C({ id: "bad id" }),
      C({ id: "x", start: " " }),
      C({ id: "y", port: 0 }),
    ]);
    const p = rowProblems(rows);
    expect(p[0]).toBeUndefined();
    expect(p[1]).toContain("重复");
    expect(p[2]).toContain("ID");
    expect(p[3]).toContain("启动命令");
    expect(p[4]).toContain("端口");
  });

  it("builds add requests and parks WAR rows without Tomcat", () => {
    const rows = toRows([
      C({ id: " web ", name: "Web" }),
      C({ id: "legacy", kind: "tomcat-war", needs_tomcat: true, port: 8080 }),
      C({ id: "legacy2", kind: "tomcat-war", needs_tomcat: true, port: 8081 }),
    ]);
    rows[2].tomcat = "~/opt/tomcat-9";
    const items = toImportItems(rows);
    expect(items[0]).toMatchObject({ id: "web", name: "Web", port: 5173, node: "20", background: true, tomcat: null });
    expect(items[0].enabled).toBeUndefined();
    expect(items[1]).toMatchObject({ enabled: false, reason: TOMCAT_REASON, tomcat: null });
    expect(items[2]).toMatchObject({ tomcat: "~/opt/tomcat-9", reason: null });
    expect(items[2].enabled).toBeUndefined();
  });
});
