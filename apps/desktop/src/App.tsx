import { invoke } from "@tauri-apps/api/core";
import { useCallback, useEffect, useRef, useState } from "react";
import type { ConfigInfo, LogTail, ProjectDetail, ProjectList, Status } from "./types";
import { LogPanel } from "./LogPanel";
import { stateShortLabel } from "./logFormat";
import { ProjectForm } from "./ProjectForm";

const LOG_LINES = 500;

export function App() {
  const [configInfo, setConfigInfo] = useState<ConfigInfo | null>(null);
  const [missingConfig, setMissingConfig] = useState(false);
  const [projects, setProjects] = useState<ProjectList | null>(null);
  const [selected, setSelected] = useState("");
  const [status, setStatus] = useState<Status | null>(null);
  const [allStatuses, setAllStatuses] = useState<Record<string, Status>>({});
  const [log, setLog] = useState<LogTail | null>(null);
  const [error, setError] = useState("");
  const [busy, setBusy] = useState<"" | "start" | "stop" | "restart">("");
  const busyRef = useRef(busy);
  busyRef.current = busy;

  const [formModal, setFormModal] = useState<{
    open: boolean;
    mode: "create" | "edit";
    detail: ProjectDetail | null;
  }>({ open: false, mode: "create", detail: null });

  const refresh = useCallback(async (id: string) => {
    const [nextStatus, nextLog] = await Promise.all([
      invoke<Status>("project_status", { id }),
      invoke<LogTail>("log_tail", { id, lines: LOG_LINES }),
    ]);
    setStatus(nextStatus);
    setAllStatuses((prev) => ({ ...prev, [nextStatus.id]: nextStatus }));
    setLog((prev) => {
      if (
        prev &&
        prev.path === nextLog.path &&
        prev.missing === nextLog.missing &&
        prev.text === nextLog.text
      ) {
        return prev;
      }
      return nextLog;
    });
  }, []);

  const loadProjects = useCallback(async () => {
    setError("");
    try {
      const info = await invoke<ConfigInfo>("config_info");
      setConfigInfo(info);
      if (!info.path) {
        setMissingConfig(true);
        setProjects(null);
        setSelected("");
        return;
      }
      const list = await invoke<ProjectList>("list_projects");
      setProjects(list);
      setMissingConfig(false);
      setSelected((prev) => {
        if (prev && list.projects.some((p) => p.id === prev)) return prev;
        const preferred =
          list.projects.find((row) => row.enabled && row.id === "legacy_app") ??
          list.projects.find((row) => row.enabled);
        return preferred?.id ?? "";
      });
    } catch (err: unknown) {
      const errStr = String(err);
      if (errStr.toLowerCase().includes("not found")) {
        setMissingConfig(true);
        setProjects(null);
        setSelected("");
      } else {
        setError(errStr);
      }
    }
  }, []);

  useEffect(() => {
    loadProjects();
  }, [loadProjects]);

  // Periodic poll of all project statuses every 3 seconds
  useEffect(() => {
    let cancelled = false;
    const fetchAll = async () => {
      try {
        const list = await invoke<Status[]>("all_status");
        if (!cancelled) {
          setAllStatuses((prev) => {
            const next = { ...prev };
            for (const s of list) {
              next[s.id] = s;
            }
            return next;
          });
        }
      } catch {
        // periodic status poll errors are ignored
      }
    };
    fetchAll();
    const timer = window.setInterval(fetchAll, 3000);
    return () => {
      cancelled = true;
      window.clearInterval(timer);
    };
  }, []);

  // Poll selected project status and log every 1 second
  useEffect(() => {
    if (!selected) return;
    let cancelled = false;
    const tick = () => {
      if (cancelled) return;
      refresh(selected).catch((err: unknown) => {
        if (!cancelled && !busyRef.current) setError(String(err));
      });
    };
    tick();
    const timer = window.setInterval(tick, 1000);
    return () => {
      cancelled = true;
      window.clearInterval(timer);
    };
  }, [selected, refresh]);

  async function handlePickConfig() {
    setError("");
    try {
      const info = await invoke<ConfigInfo | null>("pick_config_file");
      if (info && info.path) {
        setConfigInfo(info);
        await loadProjects();
      }
    } catch (err: unknown) {
      setError(String(err));
    }
  }

  async function handleCreateDefault() {
    setError("");
    try {
      const info = await invoke<ConfigInfo>("create_default_config");
      setConfigInfo(info);
      await loadProjects();
    } catch (err: unknown) {
      setError(String(err));
    }
  }

  async function handleOpenConfigFile() {
    setError("");
    try {
      await invoke("open_config_file");
    } catch (err: unknown) {
      setError(String(err));
    }
  }

  function handleOpenCreate() {
    setError("");
    setFormModal({
      open: true,
      mode: "create",
      detail: null,
    });
  }

  async function handleOpenEdit() {
    if (!selected) return;
    setError("");
    try {
      const detail = await invoke<ProjectDetail>("project_detail", { id: selected });
      setFormModal({
        open: true,
        mode: "edit",
        detail,
      });
    } catch (err: unknown) {
      setError(String(err));
    }
  }

  async function handleDeleteProject() {
    if (!current || !selected) return;
    const isRunning =
      status?.running ||
      status?.state === "starting" ||
      status?.state === "port_open" ||
      status?.state === "ready";
    if (isRunning) {
      window.alert(`项目「${current.name || selected}」正在运行中，请先停止项目再删除。`);
      return;
    }
    const confirmed = window.confirm(
      `确定要删除项目「${current.name || selected}」吗？\n只从配置里移除，不删项目文件。`
    );
    if (!confirmed) return;

    setError("");
    try {
      await invoke("remove_project", { id: selected });
      await loadProjects();
    } catch (err: unknown) {
      setError(String(err));
    }
  }

  async function handleStart() {
    if (!selected || busyRef.current) return;
    const project = projects?.projects.find((row) => row.id === selected);
    if (!project || !project.enabled || !project.configured) return;
    setBusy("start");
    setError("");
    try {
      const next = await invoke<Status>("start_project", { id: selected });
      setStatus(next);
      setAllStatuses((prev) => ({ ...prev, [next.id]: next }));
    } catch (err: unknown) {
      setError(String(err));
      await refresh(selected).catch(() => undefined);
    } finally {
      setBusy("");
    }
  }

  async function handleStop() {
    if (!selected || busyRef.current) return;
    setBusy("stop");
    setError("");
    try {
      const next = await invoke<Status>("stop_project", { id: selected });
      setStatus(next);
      setAllStatuses((prev) => ({ ...prev, [next.id]: next }));
    } catch (err: unknown) {
      setError(String(err));
    } finally {
      await refresh(selected).catch(() => undefined);
      setBusy("");
    }
  }

  async function handleRestart() {
    if (!selected || busyRef.current) return;
    setBusy("restart");
    setError("");
    try {
      const next = await invoke<Status>("restart_project", { id: selected });
      setStatus(next);
      setAllStatuses((prev) => ({ ...prev, [next.id]: next }));
    } catch (err: unknown) {
      setError(String(err));
    } finally {
      await refresh(selected).catch(() => undefined);
      setBusy("");
    }
  }

  async function reveal(kind: "log" | "page") {
    setError("");
    try {
      if (kind === "log") {
        const path = log?.path || status?.log_path || "";
        await invoke("open_log", { path });
      } else {
        await invoke("open_page", { url: pageUrl(status, current) });
      }
    } catch (err: unknown) {
      setError(String(err));
    }
  }

  const current = projects?.projects.find((row) => row.id === selected) ?? null;
  const page = pageUrl(status, current);
  const logPath = log?.path || status?.log_path || "";
  const canOpenLog = Boolean(logPath) && !log?.missing;

  // Button permissions
  const isStoppedOrFailed = status ? status.state === "stopped" || status.state === "failed" : false;
  const canStart = Boolean(
    selected &&
      busy === "" &&
      current?.enabled &&
      current?.configured &&
      isStoppedOrFailed
  );

  const canStop = Boolean(
    selected &&
      busy === "" &&
      current?.enabled &&
      status &&
      status.state !== "stopped"
  );

  const isReadyOrPortOpenOrFailed =
    status?.state === "ready" ||
    status?.state === "port_open" ||
    status?.state === "failed";
  const canRestart = Boolean(
    selected &&
      busy === "" &&
      current?.enabled &&
      current?.configured &&
      isReadyOrPortOpenOrFailed
  );

  const isPortOpenOrReady = status?.state === "port_open" || status?.state === "ready";
  const canOpenPage = Boolean(
    selected &&
      current?.configured &&
      isPortOpenOrReady &&
      page
  );

  function getOpenPageTitle(): string {
    if (!selected || !current) return "未选择项目";
    if (!current.configured) return "未配置端口或启动命令";
    if (!current.enabled) return current.reason || "项目已禁用";
    if (status?.state === "stopped") return "服务未启动（端口未通）";
    if (status?.state === "starting") return "服务启动中，端口尚未通";
    if (status?.state === "failed") return `服务启动失败${status.failure ? `：${status.failure}` : ""}`;
    if (!page) return "未获取到页面地址";
    return `在浏览器中打开：${page}`;
  }

  return (
    <div className="app">
      <aside>
        <div className="brand">
          <span className="mark">td</span>
          <div>
            <strong>tongyun-dev</strong>
            <p>本机项目</p>
          </div>
        </div>
        <div className="sidebar-action-wrap">
          <button
            type="button"
            className="btn-add-project"
            disabled={missingConfig}
            onClick={handleOpenCreate}
          >
            + 新增项目
          </button>
        </div>
        <ul className="projects">
          {projects?.projects.map((project) => {
            const toolchain = [
              project.node ? `Node ${project.node}` : null,
              project.java ? `Java ${project.java}` : null,
            ]
              .filter(Boolean)
              .join(" · ");

            const projStatus =
              project.id === selected && status ? status : allStatuses[project.id];

            return (
              <li key={project.id}>
                <button
                  type="button"
                  className={[
                    project.id === selected ? "active" : "",
                    project.enabled ? "" : "disabled",
                  ]
                    .filter(Boolean)
                    .join(" ")}
                  disabled={busy === "stop"}
                  onClick={() => {
                    setError("");
                    setStatus(null);
                    setLog(null);
                    setSelected(project.id);
                  }}
                >
                  <div className="project-row-top">
                    <span className="project-name">{project.name}</span>
                    {!project.configured ? (
                      <span className="badge-tag unconfigured">未配置</span>
                    ) : !project.enabled ? (
                      <span className="badge-tag disabled">已禁用</span>
                    ) : (
                      <span className={`status-tag ${projStatus?.state || "stopped"}`}>
                        <span className="status-dot" />
                        <span className="status-label">{stateShortLabel(projStatus)}</span>
                      </span>
                    )}
                  </div>
                  <small>
                    {project.id}
                    {project.port > 0 ? ` · ${project.port}` : ""}
                  </small>
                  {toolchain && <small className="toolchain">{toolchain}</small>}
                  {!project.enabled && <small className="reason">{project.reason}</small>}
                </button>
              </li>
            );
          })}
        </ul>
        {!projects && !error && !missingConfig && <p className="muted">正在读取配置…</p>}
        {missingConfig && <p className="muted">未找到配置</p>}
        {projects && projects.projects.length === 0 && <p className="muted">配置里没有项目</p>}
        {projects && (
          <div className="sidebar-footer">
            <p className="config-path" title={projects.config_path}>
              {projects.config_path}
            </p>
            <div className="sidebar-footer-actions">
              <button
                type="button"
                className="btn-sidebar-link"
                onClick={handleOpenConfigFile}
                title="用系统默认编辑器打开配置文件"
              >
                编辑配置文件
              </button>
              <button
                type="button"
                className="btn-sidebar-link"
                onClick={() => {
                  loadProjects().catch((err) => setError(String(err)));
                }}
                title="重新加载配置文件"
              >
                重新加载
              </button>
            </div>
          </div>
        )}
      </aside>
      <main>
        {missingConfig ? (
          <div className="empty-config">
            <h2>没找到项目配置 projects.toml</h2>
            <p className="empty-desc">
              请选择已有的配置文件，或在默认位置创建初始配置：
            </p>
            <p className="config-default-path">
              <code>{configInfo?.default_path ?? "~/.config/tongyun-dev/projects.toml"}</code>
            </p>
            <div className="actions">
              <button type="button" className="primary" onClick={handlePickConfig}>
                选择配置文件…
              </button>
              <button type="button" onClick={handleCreateDefault}>
                创建默认配置
              </button>
            </div>
            {error && <p className="banner">{error}</p>}
          </div>
        ) : (
          <>
            <header>
              <div className="title-area">
                <div className="title-row">
                  <h1>{current?.name ?? "未选择项目"}</h1>
                  {current && (
                    <div className="project-crud-actions">
                      <button
                        type="button"
                        className="btn-header-action"
                        onClick={handleOpenEdit}
                        disabled={busy !== ""}
                      >
                        编辑
                      </button>
                      <button
                        type="button"
                        className="btn-header-action btn-danger"
                        onClick={handleDeleteProject}
                        disabled={busy !== ""}
                      >
                        删除
                      </button>
                    </div>
                  )}
                </div>
                <p>{status?.root ?? current?.root ?? ""}</p>
              </div>
              <div className="actions" aria-busy={busy !== ""}>
                <button
                  type="button"
                  disabled={!canStart}
                  onClick={handleStart}
                >
                  {busy === "start" ? "启动中…" : "启动"}
                </button>
                <button
                  type="button"
                  disabled={!canStop && busy !== "stop"}
                  onClick={handleStop}
                >
                  {busy === "stop" ? "停止中…" : "停止"}
                </button>
                <button
                  type="button"
                  disabled={!canRestart}
                  onClick={handleRestart}
                >
                  {busy === "restart" ? "重启中…" : "重启"}
                </button>
                <button
                  type="button"
                  className="primary"
                  disabled={!canOpenPage}
                  title={getOpenPageTitle()}
                  onClick={() => reveal("page")}
                >
                  打开网页
                </button>
                <button
                  type="button"
                  disabled={!selected || busy !== ""}
                  onClick={() => {
                    setError("");
                    refresh(selected).catch((err: unknown) => setError(String(err)));
                  }}
                >
                  刷新状态
                </button>
              </div>
            </header>

            {/* Prominent State Bar at the top of detail area */}
            {current && (
              <DetailStateBar
                status={status}
                onScrollToLog={() => {
                  document.getElementById("log-panel")?.scrollIntoView({ behavior: "smooth" });
                }}
              />
            )}

            {/* Unconfigured project notice */}
            {current && !current.configured && (
              <div className="unconfigured-banner">
                <div>
                  <strong>缺少端口 / 启动命令</strong>
                  <p>该项目缺少端口或启动配置，启动已禁用。点击右侧去配置项目。</p>
                </div>
                <button type="button" onClick={handleOpenEdit}>
                  去配置
                </button>
              </div>
            )}

            {current && !current.enabled && <p className="banner warn">{current.reason}</p>}
            {status?.port_conflict && (
              <p className="banner warn">
                端口 {status.port} 被其他进程占用
                {status.port_holder ? `：${status.port_holder}` : "。"}
              </p>
            )}
            {error && <p className="banner">{error}</p>}

            <section className="status" aria-label="status">
              <Field
                label="running"
                value={status ? (status.running ? "true" : "false") : "—"}
                tone={status?.running ? "up" : "down"}
              />
              <Field label="pid" value={status?.pid ?? "—"} />
              <Field
                label="port"
                value={status ? `${status.port}${status.port_open ? " · open" : " · closed"}` : "—"}
              />
              <Field
                label="login_probe"
                value={
                  status
                    ? `${status.login_probe.http_code ?? "—"} · ${status.login_probe.ok ? "ok" : "not ok"}`
                    : "—"
                }
                detail={status?.login_probe.url}
                tone={status?.login_probe.ok ? "up" : undefined}
              />
            </section>

            {/* Real-time Log Panel */}
            <LogPanel
              log={log}
              logPath={logPath}
              canOpenLog={canOpenLog}
              onOpenLog={() => reveal("log")}
              selected={selected}
            />
          </>
        )}
      </main>
      {formModal.open && (
        <ProjectForm
          mode={formModal.mode}
          initialDetail={formModal.detail}
          onClose={() => setFormModal((prev) => ({ ...prev, open: false }))}
          onSuccess={async (savedId) => {
            await loadProjects();
            setSelected(savedId);
          }}
        />
      )}
    </div>
  );
}

function DetailStateBar({
  status,
  onScrollToLog,
}: {
  status: Status | null;
  onScrollToLog: () => void;
}) {
  if (!status) {
    return (
      <div className="detail-state-bar stopped">
        <div className="state-bar-main">
          <div className="state-bar-left">
            <span className="state-bar-dot" />
            <span className="state-bar-text">正在探测状态…</span>
          </div>
        </div>
      </div>
    );
  }

  const state = status.state;

  if (state === "stopped") {
    return (
      <div className="detail-state-bar stopped">
        <div className="state-bar-main">
          <div className="state-bar-left">
            <span className="state-bar-dot" />
            <span className="state-bar-text">已停止</span>
          </div>
        </div>
      </div>
    );
  }

  if (state === "starting") {
    const elapsed = status.elapsed_secs ?? 0;
    const timeout = status.timeout_secs || 180;
    const pct = Math.min(100, Math.round((elapsed / timeout) * 100));

    return (
      <div className="detail-state-bar starting">
        <div className="state-bar-main">
          <div className="state-bar-left">
            <span className="state-bar-dot" />
            <span className="state-bar-text">启动中 · 已 {elapsed} 秒</span>
          </div>
        </div>
        <div className="state-progress-wrap">
          <div className="state-progress-track">
            <div
              className="state-progress-bar"
              style={{ width: `${Math.max(4, pct)}%` }}
            />
          </div>
          <span className="state-tip">
            {status.port_open
              ? "端口已通，正在等待服务就绪…"
              : `端口 ${status.port} 还没通，日志在写说明在启动`}
          </span>
        </div>
      </div>
    );
  }

  if (state === "port_open") {
    return (
      <div className="detail-state-bar port_open">
        <div className="state-bar-main">
          <div className="state-bar-left">
            <span className="state-bar-dot" />
            <span className="state-bar-text">端口已通 · 等登录页</span>
          </div>
        </div>
      </div>
    );
  }

  if (state === "ready") {
    const elapsed = status.elapsed_secs;
    return (
      <div className="detail-state-bar ready">
        <div className="state-bar-main">
          <div className="state-bar-left">
            <span className="state-bar-dot" />
            <span className="state-bar-text">
              {elapsed != null ? `就绪 · 用时 ${elapsed} 秒` : "就绪"}
            </span>
          </div>
        </div>
      </div>
    );
  }

  if (state === "failed") {
    return (
      <div className="detail-state-bar failed">
        <div className="state-bar-main">
          <div className="state-bar-left">
            <span className="state-bar-dot" />
            <span className="state-bar-text">
              失败 · {status.failure || "服务启动失败"}
            </span>
          </div>
          <button type="button" className="btn-view-log" onClick={onScrollToLog}>
            看日志
          </button>
        </div>
      </div>
    );
  }

  return null;
}

function pageUrl(status: Status | null, project: { port: number } | null): string {
  const probe = status?.login_probe.url.trim() ?? "";
  if (probe.startsWith("http://") || probe.startsWith("https://")) return probe;
  const port = status?.port ?? project?.port ?? 0;
  if (port > 0) return `http://127.0.0.1:${port}/`;
  return "";
}

function Field({
  label,
  value,
  detail,
  tone,
}: {
  label: string;
  value: string | number;
  detail?: string;
  tone?: "up" | "down";
}) {
  return (
    <article className={tone ? `field ${tone}` : "field"}>
      <h2>{label}</h2>
      <p>{value}</p>
      {detail && <small>{detail}</small>}
    </article>
  );
}
