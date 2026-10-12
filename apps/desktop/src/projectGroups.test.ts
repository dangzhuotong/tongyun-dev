import { describe, expect, it } from "vitest";
import {
  getProjectGroup,
  groupProjects,
  isGroupCollapsed,
  isProjectRunning,
  shouldShowGroupHeaders,
  sortGroupNames,
  UNGROUPED_NAME,
  type FilterProject,
} from "./projectGroups";
import type { LifeState } from "./types";

describe("projectGroups (项目分组与排序逻辑)", () => {
  describe("getProjectGroup", () => {
    it("returns trimmed group name when provided", () => {
      expect(getProjectGroup({ group: "前端" })).toBe("前端");
      expect(getProjectGroup({ group: "  后端开发  " })).toBe("后端开发");
    });

    it("returns 未分组 when group is null, undefined, or empty/whitespace", () => {
      expect(getProjectGroup({})).toBe(UNGROUPED_NAME);
      expect(getProjectGroup({ group: null })).toBe(UNGROUPED_NAME);
      expect(getProjectGroup({ group: undefined })).toBe(UNGROUPED_NAME);
      expect(getProjectGroup({ group: "" })).toBe(UNGROUPED_NAME);
      expect(getProjectGroup({ group: "   " })).toBe(UNGROUPED_NAME);
    });
  });

  describe("isProjectRunning", () => {
    it("considers ready and port_open as running when enabled and configured", () => {
      const proj = { enabled: true, configured: true };
      expect(isProjectRunning(proj, { state: "ready" })).toBe(true);
      expect(isProjectRunning(proj, { state: "port_open" })).toBe(true);
    });

    it("considers starting, installing, stopped, failed as not running", () => {
      const proj = { enabled: true, configured: true };
      const nonRunningStates: LifeState[] = ["starting", "installing", "stopped", "failed"];
      for (const st of nonRunningStates) {
        expect(isProjectRunning(proj, { state: st })).toBe(false);
      }
      expect(isProjectRunning(proj, null)).toBe(false);
    });

    it("returns false if project is disabled or not configured", () => {
      expect(isProjectRunning({ enabled: false, configured: true }, { state: "ready" })).toBe(false);
      expect(isProjectRunning({ enabled: true, configured: false }, { state: "ready" })).toBe(false);
    });
  });

  describe("sortGroupNames", () => {
    it("places 未分组 always at the bottom", () => {
      const names = ["服务B", UNGROUPED_NAME, "服务A"];
      expect(sortGroupNames(names)).toEqual(["服务A", "服务B", UNGROUPED_NAME]);
    });

    it("sorts alphabetically when no group_order is specified", () => {
      const names = ["Demo", "Client", "Api"];
      expect(sortGroupNames(names)).toEqual(["Api", "Client", "Demo"]);
    });

    it("respects group_order and places unspecified groups after in alphabetical order", () => {
      const names = ["工具", "后端", "前端", "微服务", UNGROUPED_NAME];
      const order = ["前端", "后端"];
      expect(sortGroupNames(names, order)).toEqual([
        "前端",
        "后端",
        "工具",
        "微服务",
        UNGROUPED_NAME,
      ]);
    });

    it("guarantees 未分组 is at the end even if group_order contains it", () => {
      const names = ["前端", UNGROUPED_NAME, "后端"];
      const order = [UNGROUPED_NAME, "前端"];
      expect(sortGroupNames(names, order)).toEqual(["前端", "后端", UNGROUPED_NAME]);
    });
  });

  describe("groupProjects", () => {
    const allProjects: FilterProject[] = [
      { id: "p1", name: "Web", root: "/web", port: 3000, configured: true, enabled: true, group: "前端" },
      { id: "p2", name: "Admin", root: "/admin", port: 3001, configured: true, enabled: true, group: "前端" },
      { id: "p3", name: "API", root: "/api", port: 8080, configured: true, enabled: true, group: "后端" },
      { id: "p4", name: "Misc", root: "/misc", port: 9000, configured: true, enabled: true, group: null },
    ];

    const statuses = {
      p1: { state: "ready" as LifeState },
      p2: { state: "stopped" as LifeState },
      p3: { state: "port_open" as LifeState },
      p4: { state: "stopped" as LifeState },
    };

    it("groups projects and calculates running and total counts correctly", () => {
      const sections = groupProjects({
        allProjects,
        visibleProjects: allProjects,
        statuses,
        groupOrder: ["前端", "后端"],
      });

      expect(sections).toHaveLength(3);
      expect(sections[0].name).toBe("前端");
      expect(sections[0].totalCount).toBe(2);
      expect(sections[0].runningCount).toBe(1);
      expect(sections[0].visibleProjects).toHaveLength(2);

      expect(sections[1].name).toBe("后端");
      expect(sections[1].totalCount).toBe(1);
      expect(sections[1].runningCount).toBe(1);
      expect(sections[1].visibleProjects).toHaveLength(1);

      expect(sections[2].name).toBe(UNGROUPED_NAME);
      expect(sections[2].totalCount).toBe(1);
      expect(sections[2].runningCount).toBe(0);
      expect(sections[2].visibleProjects).toHaveLength(1);
      expect(sections[2].isUngrouped).toBe(true);
    });

    it("includes custom empty groups when not filtering", () => {
      const sections = groupProjects({
        allProjects,
        visibleProjects: allProjects,
        statuses,
        customGroups: ["新业务"],
      });

      const newGroup = sections.find((s) => s.name === "新业务");
      expect(newGroup).toBeDefined();
      expect(newGroup?.totalCount).toBe(0);
      expect(newGroup?.runningCount).toBe(0);
      expect(newGroup?.visibleProjects).toHaveLength(0);
    });

    it("hides groups with 0 visible projects when isFiltering is true", () => {
      // 只有 p1 (前端) 匹配
      const visible = [allProjects[0]];
      const sections = groupProjects({
        allProjects,
        visibleProjects: visible,
        statuses,
        customGroups: ["新业务"],
        isFiltering: true,
      });

      // 只有「前端」组有匹配项，其余「后端」、「未分组」和空组「新业务」均被隐藏
      expect(sections).toHaveLength(1);
      expect(sections[0].name).toBe("前端");
      expect(sections[0].visibleProjects).toHaveLength(1);
      expect(sections[0].totalCount).toBe(2); // totalCount 仍反映总项目数
    });
  });

  describe("shouldShowGroupHeaders", () => {
    it("returns false when there are no sections or only 1 ungrouped section", () => {
      expect(shouldShowGroupHeaders([])).toBe(false);
      expect(
        shouldShowGroupHeaders([
          {
            name: UNGROUPED_NAME,
            isUngrouped: true,
            visibleProjects: [],
            totalCount: 2,
            runningCount: 0,
          },
        ])
      ).toBe(false);
    });

    it("returns true when there are multiple sections or named sections", () => {
      expect(
        shouldShowGroupHeaders([
          {
            name: "前端",
            isUngrouped: false,
            visibleProjects: [],
            totalCount: 1,
            runningCount: 0,
          },
        ])
      ).toBe(true);

      expect(
        shouldShowGroupHeaders([
          {
            name: "前端",
            isUngrouped: false,
            visibleProjects: [],
            totalCount: 1,
            runningCount: 0,
          },
          {
            name: UNGROUPED_NAME,
            isUngrouped: true,
            visibleProjects: [],
            totalCount: 1,
            runningCount: 0,
          },
        ])
      ).toBe(true);
    });
  });

  describe("isGroupCollapsed", () => {
    it("returns false (expanded) when isFiltering is true even if stored as collapsed", () => {
      const collapsedMap = { 前端: true, 后端: true };
      expect(isGroupCollapsed("前端", collapsedMap, true)).toBe(false);
      expect(isGroupCollapsed("后端", collapsedMap, true)).toBe(false);
    });

    it("respects collapsedMap when not filtering", () => {
      const collapsedMap = { 前端: true, 后端: false };
      expect(isGroupCollapsed("前端", collapsedMap, false)).toBe(true);
      expect(isGroupCollapsed("后端", collapsedMap, false)).toBe(false);
      expect(isGroupCollapsed("其他", collapsedMap, false)).toBe(false);
    });
  });
});
