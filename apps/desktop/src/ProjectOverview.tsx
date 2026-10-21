import { useMemo, useState } from "react";
import type { LogTail, Project, Status } from "./types";
import { LogPanel } from "./LogPanel";
import {
  collectOverviewWarnings,
  overviewStatusLine,
  type OverviewWarning,
} from "./overviewStatus";
import { effectiveStartCommand } from "./startCommand";
import {
  AlertIcon,
  ChevronDownIcon,
  ChevronRightIcon,
  CodeIcon,
  CopyIcon,
  ExternalIcon,
  FileIcon,
  FolderIcon,
  GearIcon,
  PlayIcon,
  RefreshIcon,
  RestartIcon,
  StopIcon,
  TerminalIcon,
  TrashIcon,
} from "./icons";

export type OverviewBusy = "" | "start" | "stop" | "restart" | "install";

export type ProjectOverviewProps = {
  project: Project;
  status: Status | null;
  busy: OverviewBusy;
  error: string;
  log: LogTail | null;
  logPath: string;
  canOpenLog: boolean;
  canStart: boolean;
  canStop: boolean;
  canRestart: boolean;
  canInstall: boolean;
  canOpenPage: boolean;
  selectedProfile: string;
  onProfileChange: (profile: string) => void;
  copiedCmd: boolean;
  pagesOpen: boolean;
  onTogglePages: () => void;
  onClosePages: () => void;
  moreOpen: boolean;
  onToggleMore: () => void;
  onCloseMore: () => void;
  onStart: () => void;
  onStop: () => void;
  onRestart: () => void;
  onOpenPage: (url?: string) => void;
  onOpenAllPages: () => void;
  onEdit: () => void;
  onRefresh: () => void;
  onOpenCursor: () => void;
  onOpenFolder: () => void;
  onOpenTerminal: () => void;
  onOpenLog: () => void;
  onCopyCommand: () => void;
  onInstall: () => void;
  onDelete: () => void;
  onDismissError: () => void;
  onShowLogs: () => void;
  stopTitle?: string;
  openPageTitle: string;
  toolchain: string[];
  rootPath: string;
};

export function ProjectOverview(props: ProjectOverviewProps) {
  const { project, status } = props;
  const model = overviewStatusLine(project, status);
  const warnings = useMemo(
    () => collectOverviewWarnings({ project, status, error: props.error }),
    [project, status, props.error],
  );
  const links = project.links ?? [];
  const profiles = project.profiles ? Object.entries(project.profiles) : [];
  const startCommand = effectiveStartCommand(project, props.selectedProfile);

  return (
    <div className="overview-pane">
      <header className="detail-header shrink-0">
        <div className="title-area">
          <div className="title-row">
            <h1>{project.name}</h1>
            {status?.active_profile && (
              <span className="profile-active-tag" title={`当前运行环境：${status.active_profile}`}>
                ⚡ {project.profiles?.[status.active_profile]?.label || status.active_profile}
              </span>
            )}
          </div>
          <div className="subtitle">
            <code className="id-tag">{project.id}</code>
            {props.toolchain.map((item) => (
              <span key={item} className="tool-tag">
                {item}
              </span>
            ))}
            {project.depends_on && project.depends_on.length > 0 && (
              <span className="tool-tag depends-on-tag" title={`依赖服务：${project.depends_on.join(", ")}`}>
                依赖: {project.depends_on.join(", ")}
              </span>
            )}
            <button
              type="button"
              onClick={props.onOpenFolder}
              className="path subtitle-path-btn"
              title={`在系统文件管理器中打开：${props.rootPath}`}
            >
              <FolderIcon />
              <span className="truncate max-w-[320px]">{props.rootPath}</span>
            </button>
          </div>
        </div>

        <div className="header-actions" aria-busy={props.busy !== ""}>
          {profiles.length > 0 && (
            <div className="profile-selector-container">
              <label htmlFor="header-profile-select" className="profile-selector-label">
                环境:
              </label>
              <select
                id="header-profile-select"
                className="profile-selector-select"
                value={props.selectedProfile}
                disabled={props.busy !== ""}
                onChange={(event) => props.onProfileChange(event.target.value)}
                title="选择启动环境预设"
              >
                <option value="">默认配置</option>
                {profiles.map(([key, prof]) => (
                  <option key={key} value={key}>
                    {prof.label ? `${prof.label} (${key})` : key}
                  </option>
                ))}
              </select>
            </div>
          )}

          <div className="btn-group">
            <button type="button" className="btn btn-secondary" disabled={!props.canStart} onClick={props.onStart}>
              <PlayIcon className="icon-play" />
              {props.busy === "start" ? "启动中…" : "启动"}
            </button>
            <button
              type="button"
              className="btn btn-secondary"
              disabled={!props.canStop}
              title={props.stopTitle}
              onClick={props.onStop}
            >
              <StopIcon className="icon-stop" />
              {props.busy === "stop" ? "停止中…" : "停止"}
            </button>
          </div>

          {links.length > 1 ? (
            <div className="pages-split-btn">
              <button
                type="button"
                className="btn btn-primary pages-primary-btn"
                disabled={!props.canOpenPage}
                title={links[0].url}
                onClick={() => props.onOpenPage(links[0].url)}
              >
                <ExternalIcon />
                打开网页
              </button>
              <button
                type="button"
                className={`btn btn-primary pages-dropdown-toggle${props.pagesOpen ? " active" : ""}`}
                disabled={!props.canOpenPage}
                onClick={props.onTogglePages}
                title="展开全部网页链接"
                aria-label="展开全部网页链接"
                aria-expanded={props.pagesOpen}
              >
                <ChevronDownIcon />
              </button>
              {props.pagesOpen && (
                <div className="menu menu-down pages-menu" role="menu">
                  {links.map((link, idx) => (
                    <button
                      key={idx}
                      type="button"
                      role="menuitem"
                      onClick={() => {
                        props.onClosePages();
                        props.onOpenPage(link.url);
                      }}
                      title={link.url}
                    >
                      <ExternalIcon /> {link.title || link.url}
                    </button>
                  ))}
                  <div className="menu-sep" />
                  <button
                    type="button"
                    role="menuitem"
                    className="pages-open-all-btn"
                    onClick={() => {
                      props.onClosePages();
                      props.onOpenAllPages();
                    }}
                  >
                    全部打开 ({links.length})
                  </button>
                </div>
              )}
            </div>
          ) : (
            <button
              type="button"
              className="btn btn-primary"
              disabled={!props.canOpenPage}
              title={props.openPageTitle}
              onClick={() => props.onOpenPage()}
            >
              <ExternalIcon />
              打开网页
            </button>
          )}

          <div className="menu-anchor">
            <button
              type="button"
              className={`btn btn-secondary${props.moreOpen ? " pressed" : ""}`}
              onClick={props.onToggleMore}
              title="更多操作"
              aria-label="更多"
              aria-expanded={props.moreOpen}
              aria-haspopup="menu"
            >
              更多
              <ChevronDownIcon />
            </button>
            {props.moreOpen && (
              <div className="menu menu-down" role="menu" aria-label="更多操作">
                <button type="button" role="menuitem" disabled={!props.canRestart} onClick={() => { props.onCloseMore(); props.onRestart(); }}>
                  <RestartIcon /> {props.busy === "restart" ? "重启中…" : "重启"}
                </button>
                <button type="button" role="menuitem" disabled={props.busy !== ""} onClick={() => { props.onCloseMore(); props.onEdit(); }} title="编辑项目设置">
                  <GearIcon /> 编辑设置
                </button>
                <div className="menu-sep" />
                <button
                  type="button"
                  role="menuitem"
                  disabled={props.busy !== ""}
                  onClick={() => {
                    props.onCloseMore();
                    props.onRefresh();
                  }}
                >
                  <RefreshIcon /> 刷新状态
                </button>
                <button type="button" role="menuitem" onClick={() => { props.onCloseMore(); props.onOpenCursor(); }} title="用 Cursor 打开项目目录">
                  <CodeIcon /> 用 Cursor 打开
                </button>
                <button type="button" role="menuitem" onClick={() => { props.onCloseMore(); props.onOpenFolder(); }} title="在系统文件管理器中打开项目目录">
                  <FolderIcon /> 打开项目目录
                </button>
                <button type="button" role="menuitem" onClick={() => { props.onCloseMore(); props.onOpenTerminal(); }} title="在系统独立命令行终端中运行">
                  <TerminalIcon /> 在系统终端中运行
                </button>
                <button type="button" role="menuitem" disabled={!props.canOpenLog} onClick={() => { props.onCloseMore(); props.onOpenLog(); }} title="用系统默认程序打开日志文件">
                  <FileIcon /> 打开日志文件
                </button>
                <button type="button" role="menuitem" onClick={() => { props.onCloseMore(); props.onCopyCommand(); }} title={startCommand || "复制启动命令"}>
                  <CopyIcon /> {props.copiedCmd ? "已复制启动命令" : "复制启动命令"}
                </button>
                {status?.install_command && (
                  <button
                    type="button"
                    role="menuitem"
                    disabled={!props.canInstall}
                    title={`在项目目录运行 ${status.install_command}`}
                    onClick={() => {
                      props.onCloseMore();
                      props.onInstall();
                    }}
                  >
                    <RefreshIcon /> 安装依赖（{status.install_command}）
                  </button>
                )}
                <button type="button" role="menuitem" onClick={() => { props.onCloseMore(); props.onShowLogs(); }}>
                  <FileIcon /> 查看实时日志
                </button>
                <div className="menu-sep" />
                <button
                  type="button"
                  role="menuitem"
                  className="danger"
                  disabled={props.busy !== ""}
                  onClick={() => {
                    props.onCloseMore();
                    props.onDelete();
                  }}
                >
                  <TrashIcon /> 删除项目…
                </button>
              </div>
            )}
          </div>
        </div>
      </header>

      <div className="detail-body">
        <section className={`status-line tone-${model.tone}`} aria-label="运行信息">
          <div className="status-line-main">
            <span className="state-dot" />
            <strong className="status-line-headline">{model.headline}</strong>
            <span className="status-line-port">{model.portText}</span>
          </div>
          <div className="status-line-facts">
            {model.facts.map((fact) => (
              <span
                key={fact.key}
                className={`status-fact tone-${fact.tone}`}
                title={fact.title || `${fact.label} ${fact.value}`}
              >
                <span className="status-fact-label">{fact.label}</span>
                <span className="status-fact-value">{fact.value}</span>
              </span>
            ))}
          </div>
        </section>

        <OverviewWarnings
          key={`${project.id}:${warnings.map((item) => item.id).join("|")}`}
          warnings={warnings}
          canInstall={props.canInstall}
          installing={props.busy === "install"}
          onEdit={props.onEdit}
          onDismissError={props.onDismissError}
          onInstall={props.onInstall}
          onShowLogs={props.onShowLogs}
        />

        <div className="overview-log-card">
          <div className="overview-log-header">
            <div className="overview-log-title">
              <span className={`log-dot ${status?.running ? "pulse" : ""}`} />
              <span>实时日志</span>
              {props.logPath && (
                <code className="overview-log-path font-mono text-[11px] text-[var(--color-text-muted)] truncate max-w-[280px]" title={props.logPath}>
                  {props.logPath}
                </code>
              )}
            </div>
            <button type="button" className="overview-log-expand-btn" onClick={props.onShowLogs} title="切换到独立全屏沉浸式日志视图">
              <span>进入全屏日志</span>
              <ChevronRightIcon />
            </button>
          </div>
          <LogPanel
            log={props.log}
            logPath={props.logPath}
            canOpenLog={props.canOpenLog}
            onOpenLog={props.onOpenLog}
            selected={project.id}
            running={status?.running ?? false}
            onOpenTerminal={props.onOpenTerminal}
          />
        </div>
      </div>
    </div>
  );
}

function OverviewWarnings({
  warnings,
  canInstall,
  installing,
  onEdit,
  onDismissError,
  onInstall,
  onShowLogs,
}: {
  warnings: OverviewWarning[];
  canInstall: boolean;
  installing: boolean;
  onEdit: () => void;
  onDismissError: () => void;
  onInstall: () => void;
  onShowLogs: () => void;
}) {
  const [open, setOpen] = useState(false);
  if (warnings.length === 0) return null;
  const top = warnings[0];
  const extra = warnings.length - 1;
  const summary = top.detail?.replace(/\s*\n\s*/g, "；");
  const canExpand = Boolean(top.detail) || extra > 0;

  const run = (warning: OverviewWarning) => {
    if (warning.action === "edit") onEdit();
    else if (warning.action === "dismiss") onDismissError();
    else if (warning.action === "install") onInstall();
    else if (warning.action === "logs") onShowLogs();
  };

  return (
    <div className={`callout overview-warning ${top.tone}`}>
      <div className="overview-warning-row">
        <button
          type="button"
          className="overview-warning-toggle"
          aria-expanded={canExpand ? open : undefined}
          onClick={() => {
            if (canExpand) setOpen((value) => !value);
          }}
        >
          <AlertIcon className="callout-icon" />
          <span className="overview-warning-title">{top.title}</span>
          {!open && summary && <span className="overview-warning-summary">{summary}</span>}
          {!open && extra > 0 && <span className="overview-warning-count">还有 {extra} 条</span>}
          {canExpand && <ChevronDownIcon className={`chevron${open ? " open" : ""}`} />}
        </button>
        {top.action && top.actionLabel && (
          <WarningAction warning={top} canInstall={canInstall} installing={installing} onRun={run} />
        )}
      </div>
      {open && (
        <div className="overview-warning-body">
          {top.detail && <p>{top.detail}</p>}
          {warnings.slice(1).map((warning) => (
            <div key={warning.id} className="overview-warning-extra">
              <strong>{warning.title}</strong>
              {warning.detail && <p>{warning.detail}</p>}
              {warning.action && warning.actionLabel && (
                <WarningAction warning={warning} canInstall={canInstall} installing={installing} onRun={run} />
              )}
            </div>
          ))}
        </div>
      )}
    </div>
  );
}

function WarningAction({
  warning,
  canInstall,
  installing,
  onRun,
}: {
  warning: OverviewWarning;
  canInstall: boolean;
  installing: boolean;
  onRun: (warning: OverviewWarning) => void;
}) {
  const disabled = warning.action === "install" && !canInstall;
  const label = warning.action === "install" && installing ? "安装中…" : warning.actionLabel;
  return (
    <button type="button" className="btn btn-secondary btn-sm" disabled={disabled} onClick={() => onRun(warning)}>
      {label}
    </button>
  );
}
