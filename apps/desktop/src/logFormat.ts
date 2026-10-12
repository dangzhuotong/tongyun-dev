import type { LifeState, Status } from "./types";

export type LogLevel = "error" | "warn" | "normal";

export const ANSI_OSC_REGEX = /\x1b\][^\x07\x1b]*(?:\x07|\x1b\\)/g;
export const ANSI_CSI_REGEX = /\x1b\[[0-9:;<=>?]*[ -/]*[@-~]/g;

/**
 * 去掉 ANSI CSI 与 OSC 转义序列（颜色代码、光标移动、窗口标题等）。
 */
export function stripAnsi(text: string): string {
  return text.replace(ANSI_OSC_REGEX, "").replace(ANSI_CSI_REGEX, "");
}

/**
 * 处理单行内的 \r（回车符覆盖）。
 * 模拟终端在同一行输出时的光标归零与字符覆盖效果。
 */
export function resolveCarriageReturnLine(line: string): string {
  if (!line.includes("\r")) return line;
  const segments = line.split("\r");
  const buffer: string[] = [];
  for (const seg of segments) {
    for (let i = 0; i < seg.length; i++) {
      buffer[i] = seg[i];
    }
  }
  return buffer.join("");
}

/**
 * 先将 Windows CRLF (\r\n) 规整为 \n，再对每行内部的 \r 覆盖进行解析处理。
 */
export function resolveCarriageReturns(text: string): string {
  const normalized = text.replace(/\r\n/g, "\n");
  const lines = normalized.split("\n");
  const processed = lines.map(resolveCarriageReturnLine);
  return processed.join("\n");
}

/**
 * 格式化日志文本用于界面展示：
 * 去掉 ANSI CSI/OSC 控制序列，并处理 \r 覆盖行。
 */
export function formatLogText(text: string): string {
  return resolveCarriageReturns(stripAnsi(text));
}

export type SearchMatch = {
  lineIndex: number;
  start: number;
  end: number;
};

/**
 * Escapes special HTML characters to prevent XSS / HTML injection.
 */
export function escapeHtml(str: string): string {
  return str
    .replace(/&/g, "&amp;")
    .replace(/</g, "&lt;")
    .replace(/>/g, "&gt;")
    .replace(/"/g, "&quot;")
    .replace(/'/g, "&#039;");
}

/**
 * Classifies log lines based on keywords:
 * Lines containing ERROR, Exception, Caused by, FATAL are error lines.
 * Lines containing WARN are warning lines.
 */
export function getLogLevel(line: string): LogLevel {
  if (/(ERROR|Exception|Caused by|FATAL)/.test(line)) {
    return "error";
  }
  if (/WARN/.test(line)) {
    return "warn";
  }
  return "normal";
}

/**
 * Finds all case-insensitive occurrences of a query string across multiple log lines.
 */
export function findMatches(lines: string[], query: string): SearchMatch[] {
  if (!query || query.trim() === "") return [];
  const matches: SearchMatch[] = [];
  const qLower = query.toLowerCase();

  for (let lineIndex = 0; lineIndex < lines.length; lineIndex++) {
    const line = lines[lineIndex];
    const lineLower = line.toLowerCase();
    let start = 0;
    while (start <= lineLower.length - qLower.length) {
      const idx = lineLower.indexOf(qLower, start);
      if (idx === -1) break;
      matches.push({
        lineIndex,
        start: idx,
        end: idx + query.length,
      });
      start = idx + Math.max(1, query.length);
    }
  }

  return matches;
}

/**
 * Counts the total number of matches for a query across log lines.
 */
export function countMatches(lines: string[], query: string): number {
  return findMatches(lines, query).length;
}

/**
 * Formats user-facing status label for full detail banner.
 */
export function stateLabel(
  status:
    | Pick<Status, "state" | "elapsed_secs" | "failure">
    | { state: LifeState; elapsed_secs?: number | null; failure?: string | null }
    | null
    | undefined
): string {
  if (!status) return "—";
  switch (status.state) {
    case "stopped":
      return "已停止";
    case "installing": {
      const secs = status.elapsed_secs ?? 0;
      return `安装依赖中 · 已 ${secs} 秒`;
    }
    case "starting": {
      const secs = status.elapsed_secs ?? 0;
      return `启动中 · 已 ${secs} 秒`;
    }
    case "port_open":
      return "端口已通 · 等登录页";
    case "ready": {
      const secs = status.elapsed_secs;
      return secs != null ? `就绪 · 用时 ${secs} 秒` : "就绪";
    }
    case "failed":
      return `失败 · ${status.failure || "未知原因"}`;
    default:
      return "—";
  }
}

/**
 * Formats concise status label for the sidebar project list.
 */
export function stateShortLabel(
  status:
    | Pick<Status, "state" | "elapsed_secs">
    | { state: LifeState; elapsed_secs?: number | null }
    | null
    | undefined
): string {
  if (!status) return "—";
  switch (status.state) {
    case "stopped":
      return "已停止";
    case "installing": {
      const secs = status.elapsed_secs ?? 0;
      return `装依赖 · ${secs}s`;
    }
    case "starting": {
      const secs = status.elapsed_secs ?? 0;
      return `启动中 · ${secs}s`;
    }
    case "port_open":
      return "端口已通";
    case "ready":
      return "就绪";
    case "failed":
      return "失败";
    default:
      return "—";
  }
}
