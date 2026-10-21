import type { ConfigInfo, LogTail, Project, ProjectList, Status } from "./types";

/** Fictional sample used by `?preview=detail` so the detail page can be rendered without Tauri. */
export const previewDemoApi: Project = {
  id: "demo-api",
  name: "Demo API",
  group: null,
  root: "~/code/demo/demo-api",
  enabled: true,
  configured: true,
  port: 8080,
  reason: "",
  node: "20",
  java: null,
  port_mode: "env",
  hints: ["检测到启动脚本会自行追加 --port，和配置里的端口重复"],
  links: [
    { title: "控制台", url: "http://127.0.0.1:8080/" },
    { title: "接口文档", url: "http://127.0.0.1:8080/docs" },
  ],
  default_profile: "local",
  profiles: {
    local: { label: "本地", port: 8080, start: "pnpm dev" },
  },
  depends_on: ["acme-web"],
  start_command: "pnpm dev",
  profile_start_commands: { local: "pnpm dev" },
};

export const previewAcmeWeb: Project = {
  id: "acme-web",
  name: "Acme Web",
  group: null,
  root: "~/code/demo/acme-web",
  enabled: true,
  configured: true,
  port: 5173,
  reason: "",
  node: "20",
  java: null,
  start_command: "pnpm dev",
};

export const previewDemoStatus: Status = {
  id: "demo-api",
  name: "Demo API",
  running: true,
  state: "ready",
  elapsed_secs: 12,
  timeout_secs: 180,
  failure: null,
  pid: 4242,
  port: 8080,
  port_open: true,
  login_probe: {
    url: "http://127.0.0.1:8080/login",
    http_code: 200,
    ok: true,
  },
  log_path: "~/.tongyun-dev/logs/demo-api.log",
  pid_path: "~/.tongyun-dev/pids/demo-api.pid",
  root: "~/code/demo/demo-api",
  port_conflict: true,
  port_holder: "pid 9991（不是本项目启动的进程）",
  install_command: "pnpm install",
  hints: ["检测到启动脚本会自行追加 --port，和配置里的端口重复"],
  active_profile: "local",
};

export const previewAcmeStatus: Status = {
  id: "acme-web",
  name: "Acme Web",
  running: false,
  state: "stopped",
  elapsed_secs: null,
  timeout_secs: 180,
  failure: null,
  pid: null,
  port: 5173,
  port_open: false,
  login_probe: { url: "http://127.0.0.1:5173/", http_code: null, ok: false },
  log_path: "~/.tongyun-dev/logs/acme-web.log",
  pid_path: "~/.tongyun-dev/pids/acme-web.pid",
  root: "~/code/demo/acme-web",
  port_conflict: false,
  port_holder: "",
};

export const previewLog: LogTail = {
  path: "~/.tongyun-dev/logs/demo-api.log",
  missing: false,
  text: [
    "2026-10-21 09:00:01 INFO  demo-api listening on 127.0.0.1:8080",
    "2026-10-21 09:00:02 INFO  ready in 12s",
    "2026-10-21 09:00:03 INFO  login probe 200 http://127.0.0.1:8080/login",
  ].join("\n"),
};

const previewConfig: ConfigInfo = {
  path: "~/.config/tongyun-dev/projects.toml",
  source: "default",
  default_path: "~/.config/tongyun-dev/projects.toml",
};

const previewList: ProjectList = {
  config_path: previewConfig.path ?? "",
  projects: [previewDemoApi, previewAcmeWeb],
  groups: [],
};

export function isDetailPreview(): boolean {
  if (typeof window === "undefined") return false;
  try {
    return new URLSearchParams(window.location.search).get("preview") === "detail";
  } catch {
    return false;
  }
}

export function previewInvoke<T>(cmd: string, args?: Record<string, unknown>): Promise<T> {
  const id = typeof args?.id === "string" ? args.id : previewDemoApi.id;
  switch (cmd) {
    case "config_info":
      return Promise.resolve(previewConfig as T);
    case "list_projects":
      return Promise.resolve(previewList as T);
    case "all_status":
      return Promise.resolve([previewDemoStatus, previewAcmeStatus] as T);
    case "project_status":
      return Promise.resolve((id === previewAcmeWeb.id ? previewAcmeStatus : previewDemoStatus) as T);
    case "log_tail":
      return Promise.resolve(
        (id === previewAcmeWeb.id
          ? { path: previewAcmeStatus.log_path, missing: true, text: "" }
          : previewLog) as T,
      );
    default:
      return Promise.reject(new Error(`预览模式不执行命令：${cmd}`));
  }
}
