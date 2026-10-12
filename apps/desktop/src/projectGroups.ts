import type { LifeState, Project, Status } from "./types";

export const UNGROUPED_NAME = "未分组";

export type FilterProject = Pick<Project, "id" | "name" | "root" | "port" | "configured" | "enabled" | "group">;
export type FilterStatus = Pick<Status, "state"> | { state: LifeState } | undefined | null;

/**
 * 判断项目是否处于「运行中」状态。
 *
 * 运行中定义为：项目已配置且已启用，且当前状态为 ready 或 port_open。
 * 此定义与侧栏顶部的运行中计数（`ready` | `port_open`）、状态筛选标签栏（`running`）
 * 以及状态药丸灯的绿色/就绪展示保持严格一致。
 */
export function isProjectRunning(
  project: Pick<Project, "enabled" | "configured">,
  status?: FilterStatus
): boolean {
  if (!project.enabled || project.configured === false) return false;
  const state = status?.state;
  return state === "ready" || state === "port_open";
}

/**
 * 获取项目所属组名。
 * 未设置 group 或 group 为空字符串时归入「未分组」。
 */
export function getProjectGroup(project: { group?: string | null }): string {
  const g = project.group?.trim();
  return g ? g : UNGROUPED_NAME;
}

/**
 * 对分组名列表进行排序：
 * 1. 若配置了 group_order，则在 group_order 中的分组按照其在 group_order 中的顺序排在最前；
 * 2. 未在 group_order 中（或未配置 group_order）的分组按中文/名称本地化字典序排序；
 * 3. 「未分组」永远排在最后（无论 group_order 是否显式包含）。
 */
export function sortGroupNames(names: string[], groupOrder?: string[]): string[] {
  const order = (groupOrder ?? []).map((s) => s.trim()).filter((s) => s && s !== UNGROUPED_NAME);

  const distinct = Array.from(new Set(names));
  const hasUngrouped = distinct.includes(UNGROUPED_NAME);
  const otherNames = distinct.filter((n) => n !== UNGROUPED_NAME);

  otherNames.sort((a, b) => {
    const idxA = order.indexOf(a);
    const idxB = order.indexOf(b);

    if (idxA !== -1 && idxB !== -1) {
      return idxA - idxB;
    }
    if (idxA !== -1) return -1;
    if (idxB !== -1) return 1;

    return a.localeCompare(b, "zh-CN");
  });

  if (hasUngrouped) {
    otherNames.push(UNGROUPED_NAME);
  }
  return otherNames;
}

export interface GroupSection<P> {
  /** 组名 */
  name: string;
  /** 是否为未分组 */
  isUngrouped: boolean;
  /** 当前可见的项目列表（经过过滤） */
  visibleProjects: P[];
  /** 该组在总项目列表中的总数 */
  totalCount: number;
  /** 该组中当前处于「运行中」的项目数 */
  runningCount: number;
}

export interface GroupProjectsParams<P> {
  /** 全部项目列表（用于统计总数和运行数） */
  allProjects: P[];
  /** 当前可见的项目列表（经过关键词与状态过滤） */
  visibleProjects: P[];
  /** 状态映射表 */
  statuses: Record<string, FilterStatus>;
  /** 配置文件顶层指定的组排序 */
  groupOrder?: string[];
  /** 用户本地临时新建的空分组 */
  customGroups?: string[];
  /** 是否处于搜索/筛选激活状态 */
  isFiltering?: boolean;
}

/**
 * 将项目按分组整理为 GroupSection 结构。
 */
export function groupProjects<P extends FilterProject>({
  allProjects,
  visibleProjects,
  statuses,
  groupOrder,
  customGroups = [],
  isFiltering = false,
}: GroupProjectsParams<P>): GroupSection<P>[] {
  const groupMap = new Map<string, { all: P[]; visible: P[] }>();

  // 1. 初始化用户新建的分组
  for (const cg of customGroups) {
    const trimmed = cg.trim();
    if (trimmed && trimmed !== UNGROUPED_NAME && !groupMap.has(trimmed)) {
      groupMap.set(trimmed, { all: [], visible: [] });
    }
  }

  // 2. 归类全部项目
  for (const p of allProjects) {
    const g = getProjectGroup(p);
    if (!groupMap.has(g)) {
      groupMap.set(g, { all: [], visible: [] });
    }
    groupMap.get(g)!.all.push(p);
  }

  // 3. 归类当前可见项目
  for (const p of visibleProjects) {
    const g = getProjectGroup(p);
    if (!groupMap.has(g)) {
      groupMap.set(g, { all: [], visible: [] });
    }
    groupMap.get(g)!.visible.push(p);
  }

  // 4. 对组名排序
  const sortedNames = sortGroupNames(Array.from(groupMap.keys()), groupOrder);

  // 5. 组装 sections
  const sections: GroupSection<P>[] = [];
  for (const name of sortedNames) {
    const data = groupMap.get(name)!;
    const isUngrouped = name === UNGROUPED_NAME;

    // 搜索过滤时：没匹配的组隐藏
    if (isFiltering && data.visible.length === 0) {
      continue;
    }

    const runningCount = data.all.filter((p) => isProjectRunning(p, statuses[p.id])).length;

    sections.push({
      name,
      isUngrouped,
      visibleProjects: data.visible,
      totalCount: data.all.length,
      runningCount,
    });
  }

  return sections;
}

/**
 * 判断是否应在界面上展示组头：
 * 只有 1 个组（且是「未分组」）时返回 false，保持原样不显示组头；
 * 当存在多个组或有自定义/命名的分组时返回 true。
 */
export function shouldShowGroupHeaders<P>(sections: GroupSection<P>[]): boolean {
  if (sections.length === 0) return false;
  if (sections.length === 1 && sections[0].isUngrouped) return false;
  return true;
}

/**
 * 判断分组是否折叠：
 * 搜索过滤时：有匹配的组自动展开（强制返回 false）；
 * 未搜索过滤时：根据 localStorage 记录判断（缺省为 false / 展开）。
 */
export function isGroupCollapsed(
  groupName: string,
  collapsedMap: Record<string, boolean>,
  isFiltering = false
): boolean {
  if (isFiltering) {
    return false;
  }
  return Boolean(collapsedMap[groupName]);
}
