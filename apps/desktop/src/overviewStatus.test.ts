import { describe, expect, it } from "vitest";
import type { Status } from "./types";
import { previewAcmeStatus, previewAcmeWeb, previewDemoApi, previewDemoStatus } from "./previewData";
import {
  collectOverviewWarnings,
  overviewStatusLine,
  visibleOverviewWarnings,
} from "./overviewStatus";

function status(partial: Partial<Status> & Pick<Status, "state">): Status {
  return {
    id: "demo-api",
    name: "Demo API",
    running: partial.state === "ready" || partial.state === "port_open",
    elapsed_secs: null,
    timeout_secs: 180,
    failure: null,
    pid: null,
    port: 8080,
    port_open: false,
    login_probe: { url: "http://127.0.0.1:8080/login", http_code: null, ok: false },
    log_path: "~/.tongyun-dev/logs/demo-api.log",
    pid_path: "~/.tongyun-dev/pids/demo-api.pid",
    root: "~/code/demo/demo-api",
    port_conflict: false,
    port_holder: "",
    ...partial,
  };
}

describe("overviewStatusLine", () => {
  it("运行中时把端口号放在状态旁边，四项信息收成一行", () => {
    const line = overviewStatusLine(previewDemoApi, previewDemoStatus);
    expect(line.headline).toBe("运行中");
    expect(line.tone).toBe("ok");
    expect(line.portText).toBe(":8080");
    expect(line.facts.map((fact) => fact.key)).toEqual(["state", "pid", "port", "probe"]);
    expect(line.facts.find((fact) => fact.key === "state")?.value).toBe("就绪");
    expect(line.facts.find((fact) => fact.key === "pid")?.value).toBe("PID 4242");
    expect(line.facts.find((fact) => fact.key === "port")?.value).toBe("已通");
    expect(line.facts.find((fact) => fact.key === "probe")?.value).toBe("正常 · 200");
  });

  it("已停止时不重复状态词，并标出未运行和未通", () => {
    const line = overviewStatusLine(previewAcmeWeb, previewAcmeStatus);
    expect(line.headline).toBe("已停止");
    expect(line.portText).toBe(":5173");
    expect(line.facts.find((fact) => fact.key === "state")).toBeUndefined();
    expect(line.facts.find((fact) => fact.key === "pid")?.value).toBe("未运行");
    expect(line.facts.find((fact) => fact.key === "port")?.value).toBe("未通");
    expect(line.facts.find((fact) => fact.key === "probe")?.value).toBe("未响应");
  });

  it("启动中保留秒数，失败时状态行仍是已停止", () => {
    const starting = overviewStatusLine(
      previewDemoApi,
      status({ state: "starting", elapsed_secs: 8, port_open: false }),
    );
    expect(starting.headline).toBe("启动中");
    expect(starting.facts.find((fact) => fact.key === "state")?.value).toBe("启动中 · 8 秒");

    const failed = overviewStatusLine(
      previewDemoApi,
      status({ state: "failed", failure: "端口没有起来" }),
    );
    expect(failed.headline).toBe("已停止");
    expect(failed.tone).toBe("danger");
    expect(failed.facts.find((fact) => fact.key === "state")?.value).toBe("启动失败");
  });

  it("没有端口时直接标出未配置", () => {
    const line = overviewStatusLine({ ...previewDemoApi, configured: false, port: 0 }, null);
    expect(line.headline).toBe("未配置");
    expect(line.portText).toBe("端口未配置");
    expect(line.facts.find((fact) => fact.key === "port")?.value).toBe("未配置");
    expect(line.facts.find((fact) => fact.key === "probe")?.value).toBe("未配置");
  });
});

describe("collectOverviewWarnings", () => {
  it("同时有端口占用和启动提示时只把端口占用排在第一条，多条提示合成一条", () => {
    const warnings = collectOverviewWarnings({
      project: {
        ...previewDemoApi,
        hints: ["第一条提示", "第二条提示"],
      },
      status: {
        ...previewDemoStatus,
        hints: ["第一条提示", "第二条提示"],
      },
      error: "",
    });
    expect(warnings.map((item) => item.id)).toEqual(["port-conflict", "hints"]);
    expect(warnings[0].title).toBe("端口 8080 被别的进程占着");
    expect(warnings[1].detail).toBe("第一条提示\n第二条提示");
    expect(visibleOverviewWarnings(warnings, false)).toEqual([warnings[0]]);
    expect(visibleOverviewWarnings(warnings, true)).toEqual(warnings);
  });

  it("配置解析失败优先于端口占用", () => {
    const warnings = collectOverviewWarnings({
      project: { ...previewDemoApi, resolve_error: "log_dir 展不开" },
      status: previewDemoStatus,
    });
    expect(warnings[0].id).toBe("resolve");
    expect(warnings[0].action).toBe("edit");
  });

  it("启动失败和操作错误可以收进同一条警告列表", () => {
    const warnings = collectOverviewWarnings({
      project: { ...previewDemoApi, hints: [] },
      status: status({ state: "failed", failure: "探针超时", pid: 50 }),
      error: "停止没有成功",
    });
    expect(warnings.map((item) => item.id)).toEqual(["error", "failed"]);
    expect(warnings[1].action).toBe("logs");
  });

  it("缺依赖的停止态给出一条可安装的提示", () => {
    const warnings = collectOverviewWarnings({
      project: { ...previewDemoApi, hints: [] },
      status: status({ state: "stopped", deps_missing: true, install_command: "pnpm install" }),
    });
    expect(warnings.map((item) => item.id)).toEqual(["deps"]);
    expect(warnings[0].actionLabel).toBe("安装依赖");
  });
});
