import { describe, expect, it } from "vitest";
import { renderToStaticMarkup } from "react-dom/server";
import { ProjectOverview, type ProjectOverviewProps } from "./ProjectOverview";
import { previewDemoApi, previewDemoStatus, previewLog } from "./previewData";

function renderOverview(overrides: Partial<ProjectOverviewProps> = {}) {
  const props: ProjectOverviewProps = {
    project: previewDemoApi,
    status: previewDemoStatus,
    busy: "",
    error: "",
    log: previewLog,
    logPath: previewLog.path,
    canOpenLog: true,
    canStart: false,
    canStop: true,
    canRestart: true,
    canInstall: true,
    canOpenPage: true,
    selectedProfile: "local",
    onProfileChange: () => {},
    copiedCmd: false,
    pagesOpen: false,
    onTogglePages: () => {},
    onClosePages: () => {},
    moreOpen: false,
    onToggleMore: () => {},
    onCloseMore: () => {},
    onStart: () => {},
    onStop: () => {},
    onRestart: () => {},
    onOpenPage: () => {},
    onOpenAllPages: () => {},
    onEdit: () => {},
    onRefresh: () => {},
    onOpenCursor: () => {},
    onOpenFolder: () => {},
    onOpenTerminal: () => {},
    onOpenLog: () => {},
    onCopyCommand: () => {},
    onInstall: () => {},
    onDelete: () => {},
    onDismissError: () => {},
    onShowLogs: () => {},
    openPageTitle: "在浏览器中打开",
    toolchain: ["Node 20"],
    rootPath: previewDemoApi.root,
    ...overrides,
  };
  return renderToStaticMarkup(<ProjectOverview {...props} />);
}

describe("ProjectOverview", () => {
  it("只露出启动、停止、打开网页和更多，状态收成一行", () => {
    const html = renderOverview();
    expect(html).toContain("运行中");
    expect(html).toContain(":8080");
    expect(html).toContain("PID 4242");
    expect(html).toContain("已通");
    expect(html).toContain("正常 · 200");
    expect(html).toContain("启动");
    expect(html).toContain("停止");
    expect(html).toContain("控制台");
    expect(html).toContain('title="http://127.0.0.1:8080/"');
    expect(html).toContain("更多");
    expect(renderOverview({ project: { ...previewDemoApi, links: [] } })).toContain("打开网页");
    expect(html).not.toContain("quick-actions-bar");
    expect(html).not.toContain('class="stats"');
    expect(html).not.toContain("state-bar");
    expect(html).not.toContain(">重启<");
    expect(html).not.toContain("编辑设置");
    expect(html).not.toContain("在 Cursor 中打开");
    expect(html).not.toContain(">设置<");
  });

  it("折叠时只显示优先级最高的一条警告", () => {
    const html = renderOverview();
    expect(html).toContain("端口 8080 被别的进程占着");
    expect(html).toContain("还有 1 条");
    expect(html).not.toContain("启动提示");
    expect(html.match(/class="callout overview-warning/g)).toHaveLength(1);
  });

  it("启动失败折叠时盖过端口警告，并带上残留进程", () => {
    const html = renderOverview({
      project: { ...previewDemoApi, hints: [] },
      status: {
        ...previewDemoStatus,
        running: false,
        state: "failed",
        failure: "端口没有起来",
        pid: 4242,
        port_open: false,
        port_conflict: true,
        hints: [],
        login_probe: { url: "http://127.0.0.1:8080/login", http_code: null, ok: false },
      },
    });
    expect(html).toContain("tone-danger");
    expect(html).toContain("启动失败");
    expect(html).toContain("进程 PID 4242 仍在，停止可清理，重启可重新尝试");
    expect(html).toContain("看日志");
    expect(html).not.toContain("被别的进程占着");
  });

  it("启动中把进度条收在状态行里", () => {
    const html = renderOverview({
      project: { ...previewDemoApi, hints: [] },
      status: {
        ...previewDemoStatus,
        running: false,
        state: "starting",
        elapsed_secs: 8,
        timeout_secs: 180,
        port_open: false,
        port_conflict: false,
        hints: [],
      },
    });
    expect(html).toContain("最长 180 秒");
    expect(html).toContain("status-line-progress");
    expect(html).toContain('aria-label="启动进度"');
  });

  it("更多菜单里仍能到达重启、设置和原来的快捷动作", () => {
    const html = renderOverview({ moreOpen: true });
    for (const label of [
      "重启",
      "编辑设置",
      "刷新状态",
      "用 Cursor 打开",
      "打开项目目录",
      "在系统终端中运行",
      "打开日志文件",
      "复制启动命令",
      "安装依赖（pnpm install）",
      "查看实时日志",
      "删除项目…",
    ]) {
      expect(html).toContain(label);
    }
  });
});
