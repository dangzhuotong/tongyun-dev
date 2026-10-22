import type { Status } from "./types";

export type OverviewTone = "ok" | "warn" | "danger" | "info" | "muted";

type ProjectBits = {
  enabled: boolean;
  configured: boolean;
  port: number;
  resolve_error?: string | null;
  reason?: string;
  hints?: string[];
};

export type StatusFact = {
  key: "state" | "pid" | "port" | "probe";
  label: string;
  value: string;
  tone: OverviewTone;
  title?: string;
};

export type OverviewProgress = {
  mode: "determinate" | "indeterminate";
  /** 0–100。安装依赖没有总量，用 indeterminate。 */
  percent: number;
  /** 启动等待上限，例如 `最长 180 秒`。安装中为空。 */
  timeoutLabel: string;
};

export type OverviewStatusModel = {
  /** 运行中 / 已停止，以及启动中、安装中、未配置这类仍需要一眼看到的词。 */
  headline: string;
  tone: OverviewTone;
  /** 紧挨状态的端口，例如 `:8080` 或 `端口未配置`。 */
  portText: string;
  /** 原来四张统计卡的内容，紧凑排在同一行。 */
  facts: StatusFact[];
  /** 启动或安装时贴在状态行底部的细进度，没有则为 null。 */
  progress: OverviewProgress | null;
};

export type OverviewWarningAction = "edit" | "dismiss" | "install" | "logs";

export type OverviewWarning = {
  id: string;
  tone: "warn" | "danger" | "muted";
  title: string;
  detail?: string;
  action?: OverviewWarningAction;
  actionLabel?: string;
};

export function runLabel(status: Status | null, project: { enabled: boolean; configured: boolean }): string {
  if (!project.configured) return "未配置";
  if (!project.enabled) return "已停用";
  if (!status) return "探测中…";
  switch (status.state) {
    case "ready":
      return "就绪";
    case "port_open":
      return "端口已通";
    case "installing":
      return `安装依赖中 · ${status.elapsed_secs ?? 0} 秒`;
    case "starting":
      return `启动中 · ${status.elapsed_secs ?? 0} 秒`;
    case "failed":
      return status.deps_missing ? "依赖未安装" : "启动失败";
    default:
      return "已停止";
  }
}

export function runTone(status: Status | null, project: { enabled: boolean; configured: boolean }): OverviewTone {
  if (!project.configured) return "warn";
  if (!project.enabled || !status) return "muted";
  switch (status.state) {
    case "ready":
    case "port_open":
      return "ok";
    case "installing":
    case "starting":
      return "info";
    case "failed":
      return "danger";
    default:
      return "muted";
  }
}

export function overviewHeadline(
  status: Status | null,
  project: { enabled: boolean; configured: boolean },
): { headline: string; tone: OverviewTone } {
  if (!project.configured) return { headline: "未配置", tone: "warn" };
  if (!project.enabled) return { headline: "已停用", tone: "muted" };
  if (!status) return { headline: "探测中", tone: "info" };
  switch (status.state) {
    case "ready":
    case "port_open":
      return { headline: "运行中", tone: "ok" };
    case "starting":
      return { headline: "启动中", tone: "info" };
    case "installing":
      return { headline: "安装中", tone: "info" };
    case "failed":
      return { headline: "已停止", tone: "danger" };
    default:
      return { headline: "已停止", tone: "muted" };
  }
}

function portNumber(status: Status | null, project: { port: number }): number {
  if (status && status.port > 0) return status.port;
  return project.port > 0 ? project.port : 0;
}

function probeFact(project: { configured: boolean; port: number }, status: Status | null): StatusFact {
  if (!project.configured || !project.port) {
    return { key: "probe", label: "登录页", value: "未配置", tone: "muted" };
  }
  if (!status) return { key: "probe", label: "登录页", value: "—", tone: "muted" };
  const url = status.login_probe.url.trim();
  if (status.login_probe.ok) {
    const code = status.login_probe.http_code ?? "";
    return {
      key: "probe",
      label: "登录页",
      value: `正常 · ${code}`.replace(/\s·\s$/, "").trim(),
      tone: "ok",
      title: url || undefined,
    };
  }
  if (status.login_probe.http_code) {
    return {
      key: "probe",
      label: "登录页",
      value: `异常 · ${status.login_probe.http_code}`,
      tone: "danger",
      title: url || undefined,
    };
  }
  return { key: "probe", label: "登录页", value: "未响应", tone: "muted", title: url || undefined };
}

/** 状态条 + 四张统计卡合成一行。端口号跟在运行中/已停止后面，通断、进程、登录页作为行内信息。 */
export function overviewStatusLine(project: ProjectBits, status: Status | null): OverviewStatusModel {
  const head = overviewHeadline(status, project);
  const port = portNumber(status, project);
  const stateLabel = runLabel(status, project);
  const facts: StatusFact[] = [];
  if (stateLabel !== head.headline) {
    facts.push({
      key: "state",
      label: "状态",
      value: stateLabel,
      tone: runTone(status, project),
    });
  }
  facts.push({
    key: "pid",
    label: "进程",
    value: status?.pid ? `PID ${status.pid}` : status ? "未运行" : "—",
    tone: status?.pid ? "ok" : "muted",
  });
  if (!port) {
    facts.push({ key: "port", label: "端口", value: "未配置", tone: "warn" });
  } else if (!status) {
    facts.push({ key: "port", label: "端口", value: "探测中", tone: "info", title: `端口 ${port}` });
  } else {
    facts.push({
      key: "port",
      label: "端口",
      value: status.port_open ? "已通" : "未通",
      tone: status.port_open ? "ok" : "muted",
      title: `端口 ${status.port}`,
    });
  }
  facts.push(probeFact(project, status));
  const warnings = collectOverviewWarnings({ project, status });
  return {
    headline: head.headline,
    tone: toneAtLeast(head.tone, warnings),
    portText: port > 0 ? `:${port}` : "端口未配置",
    facts,
    progress: startProgress(status),
  };
}

const TONE_RANK: Record<OverviewTone, number> = {
  muted: 0,
  ok: 1,
  info: 2,
  warn: 3,
  danger: 4,
};

/** 状态行颜色取自身和警告里更高的一级，避免启动失败被端口警告的颜色盖住。启动提示不改变状态行颜色。 */
export function toneAtLeast(
  base: OverviewTone,
  warnings: { id?: string; tone: OverviewTone }[],
): OverviewTone {
  return warnings
    .filter((w) => w.id !== "hints")
    .reduce(
      (best, warning) => (TONE_RANK[warning.tone] > TONE_RANK[best] ? warning.tone : best),
      base,
    );
}

function startProgress(status: Status | null): OverviewProgress | null {
  if (!status) return null;
  if (status.state === "installing") {
    return { mode: "indeterminate", percent: 35, timeoutLabel: "" };
  }
  if (status.state === "starting") {
    const elapsed = status.elapsed_secs ?? 0;
    const timeout = status.timeout_secs || 180;
    const pct = Math.min(100, Math.round((elapsed / Math.max(timeout, 1)) * 100));
    return {
      mode: "determinate",
      percent: Math.max(4, pct),
      timeoutLabel: `最长 ${timeout} 秒`,
    };
  }
  return null;
}

function failedDetail(status: Status): string {
  const parts: string[] = [];
  if (status.failure) parts.push(status.failure);
  if (status.pid != null) {
    parts.push(`进程 PID ${status.pid} 仍在，停止可清理，重启可重新尝试`);
  }
  return parts.join("\n") || "服务启动失败";
}

function hintWarning(hints: string[]): OverviewWarning | null {
  const lines = hints.map((hint) => hint.trim()).filter(Boolean);
  if (lines.length === 0) return null;
  return {
    id: "hints",
    tone: "warn",
    title: "启动提示",
    detail: lines.join("\n"),
  };
}

/**
 * 详情页同时只突出一条警告。同级里仍按「配置错误 > 未配置 > 操作失败 > 已停用 >
 * 端口占用 > 启动失败 > 启动提示 > 缺依赖」稳定排序，但 danger 一定排在 warn / muted 前面，
 * 这样启动失败不会被折进端口占用的「还有 N 条」。
 */
export function collectOverviewWarnings(input: {
  project: ProjectBits;
  status: Status | null;
  error?: string;
}): OverviewWarning[] {
  const { project, status } = input;
  const warnings: OverviewWarning[] = [];
  const error = input.error?.trim() ?? "";

  if (project.resolve_error) {
    warnings.push({
      id: "resolve",
      tone: "warn",
      title: "配置解析失败",
      detail: project.resolve_error,
      action: "edit",
      actionLabel: "去配置",
    });
  } else if (!project.configured) {
    warnings.push({
      id: "unconfigured",
      tone: "warn",
      title: "这个项目还没配置好",
      detail: "缺端口或启动命令，所以先不能启动。填好就能用。",
      action: "edit",
      actionLabel: "去配置",
    });
  }

  if (error) {
    warnings.push({
      id: "error",
      tone: "danger",
      title: "操作没有完成",
      detail: error,
      action: "dismiss",
      actionLabel: "知道了",
    });
  }

  if (!project.enabled) {
    warnings.push({
      id: "disabled",
      tone: "muted",
      title: "已停用",
      detail: project.reason || "这个项目在配置里被停用了。",
      action: "edit",
      actionLabel: "项目设置",
    });
  }

  if (status?.port_conflict) {
    warnings.push({
      id: "port-conflict",
      tone: "warn",
      title: `端口 ${status.port} 被别的进程占着`,
      detail: status.port_holder || "不是本项目启动的进程。",
    });
  }

  if (status?.state === "failed") {
    const deps = Boolean(status.deps_missing || (status.failure ?? "").includes("依赖"));
    warnings.push({
      id: "failed",
      tone: "danger",
      title: "启动失败",
      detail: failedDetail(status),
      action: deps && status.install_command ? "install" : "logs",
      actionLabel: deps && status.install_command ? "安装依赖" : "看日志",
    });
  }

  const hints = status?.hints && status.hints.length > 0 ? status.hints : (project.hints ?? []);
  const hint = hintWarning(hints);
  if (hint) warnings.push(hint);

  if (status?.deps_missing && status.state === "stopped") {
    warnings.push({
      id: "deps",
      tone: "warn",
      title: "还没装依赖",
      detail: `缺 node_modules。点「启动」会先自动运行 ${status.install_command || "install"}。`,
      action: status.install_command ? "install" : undefined,
      actionLabel: status.install_command ? "安装依赖" : undefined,
    });
  }

  return warnings
    .map((warning, index) => ({ warning, index }))
    .sort((a, b) => {
      const byTone = TONE_RANK[b.warning.tone] - TONE_RANK[a.warning.tone];
      return byTone !== 0 ? byTone : a.index - b.index;
    })
    .map((item) => item.warning);
}

/** 折叠时只留第一条；展开后其余警告仍可看到。 */
export function visibleOverviewWarnings(warnings: OverviewWarning[], expanded: boolean): OverviewWarning[] {
  if (warnings.length === 0) return [];
  return expanded ? warnings : warnings.slice(0, 1);
}
