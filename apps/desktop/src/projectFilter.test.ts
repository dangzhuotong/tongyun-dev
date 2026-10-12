import { describe, expect, it } from "vitest";
import { countByFilter, filterProjects, matchesQuery, matchesStatus, queryTokens } from "./projectFilter";

const P = (id: string, name: string, port: number, root = `~/code/demo/${id}`, configured = true, enabled = true) => ({
  id, name, port, root, configured, enabled,
});
const projects = [
  P("acme-web", "ACME 官网前端", 3000),
  P("acme-admin", "ACME 管理后台", 8080),
  P("acme-api", "ACME 主服务", 8081),
  P("acme-ap", "ACME 接入平台", 8090, "~/code/demo/work/ap"),
  P("acme-ops", "运维看板", 0, "~/code/demo/ops", false),
  P("acme-legacy", "旧版报表", 9000, "~/code/demo/legacy", true, false),
];
const statuses = {
  "acme-admin": { state: "ready" as const },
  "acme-api": { state: "starting" as const },
  "acme-ap": { state: "failed" as const },
  "acme-web": { state: "port_open" as const },
  "acme-legacy": { state: "stopped" as const },
};

describe("queryTokens", () => {
  it("忽略大小写和多余空白", () => {
    expect(queryTokens("  ACME   Admin ")).toEqual(["acme", "admin"]);
    expect(queryTokens("   ")).toEqual([]);
  });
});

describe("matchesQuery", () => {
  it("空查询全部命中", () => {
    expect(projects.every((p) => matchesQuery(p, ""))).toBe(true);
  });
  it("按名称（中文）", () => {
    expect(filterProjects(projects, statuses, "管理", "all").map((p) => p.id)).toEqual(["acme-admin"]);
  });
  it("按 id，忽略大小写", () => {
    expect(filterProjects(projects, statuses, "ACME-AP", "all").map((p) => p.id)).toEqual(["acme-api", "acme-ap"]);
  });
  it("按端口", () => {
    expect(filterProjects(projects, statuses, "8081", "all").map((p) => p.id)).toEqual(["acme-api"]);
    expect(filterProjects(projects, statuses, "808", "all").map((p) => p.id)).toEqual(["acme-admin", "acme-api"]);
  });
  it("端口 0 不参与匹配", () => {
    expect(filterProjects(projects, statuses, "0", "all").map((p) => p.id)).not.toContain("acme-ops");
  });
  it("按目录", () => {
    expect(filterProjects(projects, statuses, "/work/", "all").map((p) => p.id)).toEqual(["acme-ap"]);
  });
  it("多个关键词需同时命中", () => {
    expect(filterProjects(projects, statuses, "acme 后台", "all").map((p) => p.id)).toEqual(["acme-admin"]);
    expect(filterProjects(projects, statuses, "acme 不存在", "all")).toEqual([]);
  });
});

describe("matchesStatus", () => {
  it("运行中 = 就绪或端口已通", () => {
    expect(filterProjects(projects, statuses, "", "running").map((p) => p.id)).toEqual(["acme-web", "acme-admin"]);
  });
  it("启动中 / 失败", () => {
    expect(filterProjects(projects, statuses, "", "starting").map((p) => p.id)).toEqual(["acme-api"]);
    expect(filterProjects(projects, statuses, "", "failed").map((p) => p.id)).toEqual(["acme-ap"]);
  });
  it("未配置只看 configured", () => {
    expect(filterProjects(projects, statuses, "", "unconfigured").map((p) => p.id)).toEqual(["acme-ops"]);
  });
  it("没有状态时按已停止算；禁用项目不算运行中", () => {
    expect(matchesStatus(P("x", "x", 1), undefined, "running")).toBe(false);
    expect(matchesStatus(P("x", "x", 1, "/x", true, false), { state: "ready" }, "running")).toBe(false);
  });
});

describe("countByFilter", () => {
  it("统计各状态数量", () => {
    expect(countByFilter(projects, statuses, "")).toEqual({ all: 6, running: 2, starting: 1, failed: 1, unconfigured: 1 });
  });
  it("跟随搜索词", () => {
    expect(countByFilter(projects, statuses, "acme 主")).toEqual({ all: 1, running: 0, starting: 1, failed: 0, unconfigured: 0 });
  });
});

describe("installing 算启动中", () => {
  it("装依赖的项目出现在「启动中」筛选里", () => {
    const p = { id: "web", name: "web", port: 5173, root: "/w", configured: true, enabled: true };
    expect(matchesStatus(p, { state: "installing" }, "starting")).toBe(true);
    expect(matchesStatus(p, { state: "installing" }, "running")).toBe(false);
  });
});
