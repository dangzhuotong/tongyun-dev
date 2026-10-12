import type { GroupInfo, LifeState, Project, Status } from "./types";

export const UNGROUPED_NAME = "未分组";
export const UNGROUPED_KEY = "";

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
 * 获取项目所属组 key。
 * 未设置 group 或 group 为空字符串时归入 UNGROUPED_KEY ("")。
 */
export function getProjectGroupKey(project: { group?: string | null }): string {
  const g = project.group?.trim();
  return g ? g : UNGROUPED_KEY;
}

/**
 * 兼容旧接口：获取项目所属组名。
 * 未设置 group 或 group 为空字符串时归入「未分组」。
 */
export function getProjectGroup(project: { group?: string | null }): string {
  const g = project.group?.trim();
  return g ? g : UNGROUPED_NAME;
}

/**
 * 自动根据用户输入的显示名生成 group key：
 * ASCII 转小写、非法字符换连字符（1-40 字符，^[a-z0-9_-]+$）；
 * 非 ASCII、空名或 key 已存在时用 g1、g2… 递增避开已有。
 */
export function generateGroupKey(label: string, existingKeys: string[] = []): string {
  const trimmed = label.trim();
  const isAscii = /^[\x00-\x7F]+$/.test(trimmed);
  if (isAscii && trimmed.length > 0) {
    let candidate = trimmed
      .toLowerCase()
      .replace(/[^a-z0-9_-]+/g, "-")
      .replace(/^-+|-+$/g, "");
    if (candidate.length > 40) {
      candidate = candidate.slice(0, 40).replace(/-+$/, "");
    }
    if (candidate.length > 0 && !existingKeys.includes(candidate)) {
      return candidate;
    }
  }
  let idx = 1;
  while (existingKeys.includes(`g${idx}`)) {
    idx++;
  }
  return `g${idx}`;
}

export interface ResolvedFormGroup {
  /** 提交给项目的 group key。若输入为空则为 undefined（清空/未分组）。 */
  key?: string;
  /** 若需要新建组，包含待建组的 key 与 label */
  newGroup?: {
    key: string;
    label: string;
  };
}

/**
 * 获取项目表单初始展示的分组名称（若已有对应组的 label 则展示 label，否则展示 key）。
 */
export function getInitialGroupLabel(
  key: string | undefined | null,
  groups: GroupInfo[] = []
): string {
  const trimmed = (key ?? "").trim();
  if (!trimmed) return "";
  const found = groups.find((g) => g.key === trimmed);
  return found?.label || trimmed;
}

/**
 * 将用户在项目表单中输入或选择的分组名称解析为后端合法的 group key：
 * 1. 输入为空或纯空白：不设组（返回 undefined）
 * 2. 先按 label（以及 key）在已有分组中查找（精确匹配优先，支持大小写不敏感匹配）：
 *    若匹配到已有组，直接使用其 key，不新建组
 * 3. 若已有组中查不到（用户输入了新组名）：
 *    使用 generateGroupKey 生成合法 key，
 *    返回该 key 并标记 newGroup: { key, label: trimmedInput }，
 *    以便保存前调用 create_group(key, label) 创建新组。
 */
export function resolveFormGroup(
  rawInput: string | undefined | null,
  groups: GroupInfo[] = [],
  existingKeys: string[] = []
): ResolvedFormGroup {
  const trimmed = (rawInput ?? "").trim();
  if (!trimmed) {
    return { key: undefined };
  }

  const allExistingKeys = Array.from(
    new Set([
      ...groups.map((g) => g.key).filter(Boolean),
      ...existingKeys.filter(Boolean),
    ])
  );

  // 优先精确匹配 label 或 key
  const exact = groups.find((g) => g.label === trimmed || g.key === trimmed);
  if (exact) {
    return { key: exact.key };
  }

  // 大小写不敏感匹配 label 或 key
  const lower = trimmed.toLowerCase();
  const caseInsensitive = groups.find(
    (g) => g.label.toLowerCase() === lower || g.key.toLowerCase() === lower
  );
  if (caseInsensitive) {
    return { key: caseInsensitive.key };
  }

  // 若直接在 existingKeys 中匹配到
  const directKey = allExistingKeys.find((k) => k.toLowerCase() === lower);
  if (directKey) {
    return { key: directKey };
  }

  // 查不到，生成合法 key
  const generatedKey = generateGroupKey(trimmed, allExistingKeys);
  return {
    key: generatedKey,
    newGroup: {
      key: generatedKey,
      label: trimmed,
    },
  };
}


/**
 * 对分组名列表进行排序（兼容旧接口）：
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
  /** 组 key */
  key: string;
  /** 组显示名 */
  label: string;
  /** 兼容旧代码，同 label */
  name: string;
  /** 排序序号（可选） */
  order?: number | null;
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
  /** 顶层定义的 groups（来自 core list 输出） */
  groups?: GroupInfo[];
  /** 配置文件顶层指定的组排序（兼容旧参数） */
  groupOrder?: string[];
  /** 用户本地临时新建的空分组（兼容旧参数） */
  customGroups?: string[];
  /** 是否处于搜索/筛选激活状态 */
  isFiltering?: boolean;
}

/**
 * 将项目按分组整理为 GroupSection 结构。
 * 优先按照 list 输出的 groups 顺序和 label 决定分组展示，
 * 成员来自项目 group 字段；
 * 未定义在 groups 中的项目引用的 group 也会作为同名组展示；
 * 空分组正常展示；「未分组」永远排在最后。
 */
export function groupProjects<P extends FilterProject>({
  allProjects,
  visibleProjects,
  statuses,
  groups,
  groupOrder,
  customGroups = [],
  isFiltering = false,
}: GroupProjectsParams<P>): GroupSection<P>[] {
  // 新结构：传入 groups 数组（来自 core list 输出）
  if (groups !== undefined) {
    const groupKeyMap = new Map<
      string,
      { label: string; order: number | null; all: P[]; visible: P[] }
    >();

    for (const g of groups) {
      groupKeyMap.set(g.key, {
        label: g.label || g.key,
        order: g.order ?? null,
        all: [],
        visible: [],
      });
    }

    for (const cg of customGroups) {
      const trimmed = cg.trim();
      if (trimmed && trimmed !== UNGROUPED_NAME && !groupKeyMap.has(trimmed)) {
        groupKeyMap.set(trimmed, { label: trimmed, order: null, all: [], visible: [] });
      }
    }

    const ungroupedAll: P[] = [];
    const ungroupedVisible: P[] = [];

    for (const p of allProjects) {
      const k = p.group?.trim();
      if (!k) {
        ungroupedAll.push(p);
      } else {
        if (!groupKeyMap.has(k)) {
          groupKeyMap.set(k, { label: k, order: null, all: [], visible: [] });
        }
        groupKeyMap.get(k)!.all.push(p);
      }
    }

    for (const p of visibleProjects) {
      const k = p.group?.trim();
      if (!k) {
        ungroupedVisible.push(p);
      } else {
        if (!groupKeyMap.has(k)) {
          groupKeyMap.set(k, { label: k, order: null, all: [], visible: [] });
        }
        groupKeyMap.get(k)!.visible.push(p);
      }
    }

    const sections: GroupSection<P>[] = [];

    for (const [key, data] of groupKeyMap.entries()) {
      if (isFiltering && data.visible.length === 0) {
        continue;
      }
      const runningCount = data.all.filter((p) => isProjectRunning(p, statuses[p.id])).length;
      sections.push({
        key,
        label: data.label,
        name: data.label,
        order: data.order,
        isUngrouped: false,
        visibleProjects: data.visible,
        totalCount: data.all.length,
        runningCount,
      });
    }

    // 「未分组」永远在最后
    if (ungroupedAll.length > 0 || sections.length === 0) {
      if (!isFiltering || ungroupedVisible.length > 0) {
        const runningCount = ungroupedAll.filter((p) => isProjectRunning(p, statuses[p.id])).length;
        sections.push({
          key: UNGROUPED_KEY,
          label: UNGROUPED_NAME,
          name: UNGROUPED_NAME,
          order: null,
          isUngrouped: true,
          visibleProjects: ungroupedVisible,
          totalCount: ungroupedAll.length,
          runningCount,
        });
      }
    }

    return sections;
  }

  // 兼容回退逻辑：按 group 名称归类
  const groupMap = new Map<string, { all: P[]; visible: P[] }>();

  for (const cg of customGroups) {
    const trimmed = cg.trim();
    if (trimmed && trimmed !== UNGROUPED_NAME && !groupMap.has(trimmed)) {
      groupMap.set(trimmed, { all: [], visible: [] });
    }
  }

  for (const p of allProjects) {
    const g = getProjectGroup(p);
    if (!groupMap.has(g)) {
      groupMap.set(g, { all: [], visible: [] });
    }
    groupMap.get(g)!.all.push(p);
  }

  for (const p of visibleProjects) {
    const g = getProjectGroup(p);
    if (!groupMap.has(g)) {
      groupMap.set(g, { all: [], visible: [] });
    }
    groupMap.get(g)!.visible.push(p);
  }

  const sortedNames = sortGroupNames(Array.from(groupMap.keys()), groupOrder);
  const sections: GroupSection<P>[] = [];
  for (const name of sortedNames) {
    const data = groupMap.get(name)!;
    const isUngrouped = name === UNGROUPED_NAME;

    if (isFiltering && data.visible.length === 0) {
      continue;
    }

    const runningCount = data.all.filter((p) => isProjectRunning(p, statuses[p.id])).length;
    sections.push({
      key: isUngrouped ? UNGROUPED_KEY : name,
      label: name,
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
  groupKey: string,
  collapsedMap: Record<string, boolean>,
  isFiltering = false
): boolean {
  if (isFiltering) {
    return false;
  }
  return Boolean(collapsedMap[groupKey]);
}
