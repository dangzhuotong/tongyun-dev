import { invoke } from "@tauri-apps/api/core";
import { useCallback, useEffect, useRef, useState } from "react";
import type { LogTail, ProjectList, Status } from "./types";

const LOG_LINES = 200;

export function App() {
  const [projects, setProjects] = useState<ProjectList | null>(null);
  const [selected, setSelected] = useState("");
  const [status, setStatus] = useState<Status | null>(null);
  const [log, setLog] = useState<LogTail | null>(null);
  const [error, setError] = useState("");
  const [busy, setBusy] = useState<"" | "start" | "stop">("");
  const logRef = useRef<HTMLPreElement>(null);
  const busyRef = useRef(busy);
  busyRef.current = busy;

  const refresh = useCallback(async (id: string) => {
    const [nextStatus, nextLog] = await Promise.all([
      invoke<Status>("project_status", { id }),
      invoke<LogTail>("log_tail", { id, lines: LOG_LINES }),
    ]);
    setStatus(nextStatus);
    setLog(nextLog);
  }, []);

  useEffect(() => {
    let cancelled = false;
    invoke<ProjectList>("list_projects")
      .then((list) => {
        if (cancelled) return;
        setProjects(list);
        const preferred =
          list.projects.find((row) => row.enabled && row.id === "legacy_app") ??
          list.projects.find((row) => row.enabled);
        if (preferred) setSelected(preferred.id);
      })
      .catch((err: unknown) => {
        if (!cancelled) setError(String(err));
      });
    return () => {
      cancelled = true;
    };
  }, []);

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
    const timer = window.setInterval(tick, 1500);
    return () => {
      cancelled = true;
      window.clearInterval(timer);
    };
  }, [selected, refresh]);

  useEffect(() => {
    const node = logRef.current;
    if (node) node.scrollTop = node.scrollHeight;
  }, [log?.text]);

  async function run(action: "start" | "stop") {
    if (!selected || busyRef.current) return;
    const project = projects?.projects.find((row) => row.id === selected);
    if (action === "start" && project && !project.enabled) return;
    setBusy(action);
    setError("");
    try {
      const command = action === "start" ? "start_project" : "stop_project";
      const next = await invoke<Status>(command, { id: selected });
      setStatus(next);
      const tail = await invoke<LogTail>("log_tail", { id: selected, lines: LOG_LINES });
      setLog(tail);
    } catch (err: unknown) {
      setError(String(err));
      await refresh(selected).catch(() => undefined);
    } finally {
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
        <ul className="projects">
          {projects?.projects.map((project) => {
            const toolchain = [
              project.node ? `Node ${project.node}` : null,
              project.java ? `Java ${project.java}` : null,
            ]
              .filter(Boolean)
              .join(" · ");

            return (
              <li key={project.id}>
                <button
                  type="button"
                  className={[project.id === selected ? "active" : "", project.enabled ? "" : "disabled"]
                    .filter(Boolean)
                    .join(" ")}
                  disabled={busy !== ""}
                  onClick={() => {
                    setError("");
                    setStatus(null);
                    setLog(null);
                    setSelected(project.id);
                  }}
                >
                  <span>{project.name}</span>
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
        {!projects && !error && <p className="muted">正在读取配置…</p>}
        {projects && projects.projects.length === 0 && <p className="muted">配置里没有项目</p>}
        {projects && <p className="config-path">{projects.config_path}</p>}
      </aside>
      <main>
        <header>
          <div>
            <h1>{current?.name ?? "未选择项目"}</h1>
            <p>{status?.root ?? current?.root ?? ""}</p>
          </div>
          <div className="actions" aria-busy={busy !== ""}>
            <button
              type="button"
              className={busy === "start" ? "primary busy" : "primary"}
              disabled={!selected || busy !== "" || current?.enabled === false}
              onClick={() => run("start")}
            >
              {busy === "start" ? "启动中…" : "启动"}
            </button>
            <button type="button" disabled={!selected || busy !== ""} onClick={() => run("stop")}>
              {busy === "stop" ? "停止中…" : "停止"}
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
            <button type="button" disabled={!page} onClick={() => reveal("page")}>
              打开页面
            </button>
          </div>
        </header>
        {busy === "start" && <p className="banner busy">正在启动… 按钮会在端口或页面就绪后恢复。</p>}
        {busy === "stop" && <p className="banner busy">正在停止… 按钮会在端口释放后恢复。</p>}
        {current && !current.enabled && <p className="banner warn">{current.reason}</p>}
        {status?.port_conflict && (
          <p className="banner warn">
            端口 {status.port} 被其他进程占用
            {status.port_holder ? `：${status.port_holder}` : "。"}
          </p>
        )}
        {error && <p className="banner">{error}</p>}
        <section className="status" aria-label="status">
          <Field label="running" value={status ? (status.running ? "true" : "false") : "—"} tone={status?.running ? "up" : "down"} />
          <Field label="pid" value={status?.pid ?? "—"} />
          <Field label="port" value={status ? `${status.port}${status.port_open ? " · open" : " · closed"}` : "—"} />
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
        <section className="log">
          <div className="log-bar">
            <strong>log</strong>
            <span>{logPath}</span>
            <button type="button" disabled={!canOpenLog} onClick={() => reveal("log")}>
              打开日志
            </button>
          </div>
          <pre ref={logRef}>
            {log?.missing
              ? "日志文件还不存在。启动后会在这里跟上新行。"
              : log?.text || (selected ? "日志是空的。" : "")}
          </pre>
        </section>
      </main>
    </div>
  );
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
