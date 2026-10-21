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
    expect(html).toContain("打开网页");
    expect(html).toContain("更多");
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
