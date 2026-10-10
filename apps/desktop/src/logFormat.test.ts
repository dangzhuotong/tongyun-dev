import { describe, expect, it } from "vitest";
import {
  countMatches,
  escapeHtml,
  findMatches,
  getLogLevel,
  stateLabel,
  stateShortLabel,
} from "./logFormat";

describe("getLogLevel (错误行与警告行识别)", () => {
  it("identifies ERROR as error", () => {
    expect(getLogLevel("2026-10-10 12:00:00 [main] ERROR com.example.App - Failed to start")).toBe("error");
  });

  it("identifies Exception as error", () => {
    expect(getLogLevel("java.lang.NullPointerException: null")).toBe("error");
  });

  it("identifies Caused by as error", () => {
    expect(getLogLevel("Caused by: org.springframework.beans.factory.BeanCreationException")).toBe("error");
  });

  it("identifies FATAL as error", () => {
    expect(getLogLevel("FATAL server encountered unrecoverable issue")).toBe("error");
  });

  it("identifies WARN as warn", () => {
    expect(getLogLevel("2026-10-10 12:00:00 [main] WARN com.example.App - Deprecated config")).toBe("warn");
  });

  it("prioritizes error when both ERROR and WARN are present", () => {
    expect(getLogLevel("WARN: suppressed ERROR occurred")).toBe("error");
  });

  it("returns normal for normal log lines", () => {
    expect(getLogLevel("2026-10-10 12:00:00 [main] INFO com.example.App - Server started")).toBe("normal");
    expect(getLogLevel("")).toBe("normal");
  });
});

describe("search matches and counting (搜索匹配计数)", () => {
  const lines = [
    "2026-10-10 10:00:00 [main] INFO server started on port 8080",
    "2026-10-10 10:00:01 [worker-1] DEBUG port check: port 8080 is open",
    "2026-10-10 10:00:02 [main] INFO ready",
  ];

  it("returns 0 matches for empty or whitespace query", () => {
    expect(countMatches(lines, "")).toBe(0);
    expect(countMatches(lines, "   ")).toBe(0);
    expect(findMatches(lines, "")).toEqual([]);
  });

  it("counts case-insensitive matches across multiple lines", () => {
    // "port" appears once in line 0, twice in line 1 ("port check", "port 8080")
    expect(countMatches(lines, "port")).toBe(3);
    expect(countMatches(lines, "PORT")).toBe(3);
  });

  it("locates line indices and match offsets accurately", () => {
    const matches = findMatches(lines, "ready");
    expect(matches).toHaveLength(1);
    expect(matches[0]).toEqual({
      lineIndex: 2,
      start: 32,
      end: 37,
    });
  });

  it("returns 0 matches when keyword is not found", () => {
    expect(countMatches(lines, "notfound")).toBe(0);
  });
});

describe("escapeHtml (HTML 转义防注入)", () => {
  it("escapes special HTML characters to prevent XSS injection", () => {
    const malicious = '<script>alert("XSS & danger")</script>';
    const safe = escapeHtml(malicious);
    expect(safe).toBe("&lt;script&gt;alert(&quot;XSS &amp; danger&quot;)&lt;/script&gt;");
    expect(safe).not.toContain("<");
    expect(safe).not.toContain(">");
    expect(safe).not.toContain('"');
  });

  it("escapes single quotes correctly", () => {
    expect(escapeHtml("onclick='evil()'")).toBe("onclick=&#039;evil()&#039;");
  });

  it("leaves strings without special characters untouched", () => {
    expect(escapeHtml("simple text 123")).toBe("simple text 123");
  });
});

describe("stateLabel (各状态文字)", () => {
  it("formats stopped status correctly", () => {
    expect(stateLabel({ state: "stopped" })).toBe("已停止");
  });

  it("formats starting status with elapsed seconds", () => {
    expect(stateLabel({ state: "starting", elapsed_secs: 8 })).toBe("启动中 · 已 8 秒");
    expect(stateLabel({ state: "starting", elapsed_secs: null })).toBe("启动中 · 已 0 秒");
  });

  it("formats port_open status correctly", () => {
    expect(stateLabel({ state: "port_open" })).toBe("端口已通 · 等登录页");
  });

  it("formats ready status with elapsed seconds", () => {
    expect(stateLabel({ state: "ready", elapsed_secs: 5 })).toBe("就绪 · 用时 5 秒");
    expect(stateLabel({ state: "ready", elapsed_secs: null })).toBe("就绪");
  });

  it("formats failed status with failure reason", () => {
    expect(stateLabel({ state: "failed", failure: "进程已退出" })).toBe("失败 · 进程已退出");
    expect(stateLabel({ state: "failed", failure: null })).toBe("失败 · 未知原因");
  });

  it("handles null or undefined safely", () => {
    expect(stateLabel(null)).toBe("—");
    expect(stateLabel(undefined)).toBe("—");
  });

  it("formats short label for sidebar list", () => {
    expect(stateShortLabel({ state: "stopped" })).toBe("已停止");
    expect(stateShortLabel({ state: "starting", elapsed_secs: 14 })).toBe("启动中 · 14s");
    expect(stateShortLabel({ state: "port_open" })).toBe("端口已通");
    expect(stateShortLabel({ state: "ready" })).toBe("就绪");
    expect(stateShortLabel({ state: "failed" })).toBe("失败");
  });
});
