import type { NewProjectReq, ScanCandidate } from "./types";

/** A scan result row as edited in the dialog. */
export type ScanRow = ScanCandidate & {
  picked: boolean;
  /** Tomcat install dir typed by the user (WAR projects only). */
  tomcat: string;
};

export const TOMCAT_REASON = "Tomcat WAR：先填 Tomcat 安装目录（tomcat 字段）再启用";

export function toRows(candidates: ScanCandidate[]): ScanRow[] {
  return candidates.map((c) => ({ ...c, picked: !c.existing_id, tomcat: "" }));
}

/** Rows that will be written by「添加选中」. */
export function pickedRows(rows: ScanRow[]): ScanRow[] {
  return rows.filter((r) => r.picked && !r.existing_id);
}

/** Problems that block adding a picked row (keyed by row index). */
export function rowProblems(rows: ScanRow[]): Record<number, string> {
  const out: Record<number, string> = {};
  const seen = new Map<string, number>();
  rows.forEach((r, i) => {
    if (!r.picked || r.existing_id) return;
    const id = r.id.trim();
    if (!/^[A-Za-z0-9_-]{1,64}$/.test(id)) out[i] = "ID 只能用字母、数字、_、-";
    else if (seen.has(id)) out[i] = `ID 和第 ${(seen.get(id) ?? 0) + 1} 行重复`;
    else if (!r.start.trim()) out[i] = "启动命令不能为空";
    else if (!r.port || r.port < 1 || r.port > 65535) out[i] = "填一个端口";
    seen.set(id, i);
  });
  return out;
}

/** Turn picked rows into add requests. WAR rows stay disabled until a Tomcat dir is given. */
export function toImportItems(rows: ScanRow[]): NewProjectReq[] {
  return pickedRows(rows).map((r) => {
    const tomcat = r.tomcat.trim();
    const parked = r.needs_tomcat && !tomcat;
    return {
      id: r.id.trim(),
      name: r.name.trim() || null,
      root: r.root,
      start: r.start.trim(),
      stop: r.stop,
      port: r.port || null,
      health_url: r.health_url,
      login_url: r.login_url,
      node: r.node,
      java: r.java,
      background: r.background,
      tomcat: tomcat || null,
      env: r.env,
      profiles: r.profiles,
      enabled: parked ? false : undefined,
      reason: parked ? TOMCAT_REASON : null,
    };
  });
}

export const CONFIDENCE_LABEL: Record<string, string> = {
  high: "把握大",
  medium: "大概率",
  low: "猜的",
};
