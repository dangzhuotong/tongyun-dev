import type { LifeState, Project, Status } from "./types";

/** 侧栏状态筛选。运行中 = 端口已通或就绪；未配置 = 缺端口/启动命令。 */
export type StatusFilter = "all" | "running" | "starting" | "failed" | "unconfigured";

export const STATUS_FILTERS: { key: StatusFilter; label: string }[] = [
  { key: "all", label: "全部" },
  { key: "running", label: "运行中" },
  { key: "starting", label: "启动中" },
  { key: "failed", label: "失败" },
  { key: "unconfigured", label: "未配置" },
];

type FilterProject = Pick<Project, "id" | "name" | "root" | "port" | "configured" | "enabled">;
type FilterStatus = Pick<Status, "state"> | { state: LifeState } | undefined | null;

/** 把输入拆成关键词：忽略大小写、去掉多余空白。 */
export function queryTokens(query: string): string[] {
  return query
    .trim()
    .toLowerCase()
    .split(/\s+/)
    .filter(Boolean);
}

/** 每个关键词都要命中名称、id、端口或目录之一（AND）。空查询全部命中。 */
export function matchesQuery(project: FilterProject, query: string): boolean {
  const tokens = queryTokens(query);
  if (tokens.length === 0) return true;
  const hay = [project.name, project.id, project.port > 0 ? String(project.port) : "", project.root]
    .join("\n")
    .toLowerCase();
  return tokens.every((t) => hay.includes(t));
}

export function matchesStatus(project: FilterProject, status: FilterStatus, filter: StatusFilter): boolean {
  if (filter === "all") return true;
  if (filter === "unconfigured") return !project.configured;
  if (!project.configured || !project.enabled) return false;
  const state = status?.state ?? "stopped";
  switch (filter) {
    case "running":
      return state === "ready" || state === "port_open";
    case "starting":
      return state === "starting" || state === "installing";
    case "failed":
      return state === "failed";
    default:
      return true;
  }
}

export function filterProjects<P extends FilterProject>(
  projects: P[],
  statuses: Record<string, FilterStatus>,
  query: string,
  filter: StatusFilter,
): P[] {
  return projects.filter((p) => matchesQuery(p, query) && matchesStatus(p, statuses[p.id], filter));
}

/** 各状态筛选的数量（在当前搜索词下）。 */
export function countByFilter<P extends FilterProject>(
  projects: P[],
  statuses: Record<string, FilterStatus>,
  query: string,
): Record<StatusFilter, number> {
  const out: Record<StatusFilter, number> = { all: 0, running: 0, starting: 0, failed: 0, unconfigured: 0 };
  for (const p of projects) {
    if (!matchesQuery(p, query)) continue;
    for (const { key } of STATUS_FILTERS) {
      if (matchesStatus(p, statuses[p.id], key)) out[key] += 1;
    }
  }
  return out;
}
