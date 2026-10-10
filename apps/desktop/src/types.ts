export type LifeState = "stopped" | "installing" | "starting" | "port_open" | "ready" | "failed";

export type Project = {
  id: string;
  name: string;
  root: string;
  enabled: boolean;
  configured: boolean;
  port: number;
  reason: string;
  start_timeout_secs?: number;
  stop_grace_secs?: number;
  node?: string | null;
  java?: string | null;
  java_home?: string | null;
  node_home?: string | null;
  shell?: string | null;
  /** 缺 node_modules 时启动前自动装依赖（缺省 true）。 */
  auto_install?: boolean;
};

export type ProjectRow = Project;

export type ProjectList = {
  config_path: string;
  projects: Project[];
};

export type LoginProbe = {
  url: string;
  http_code: number | null;
  ok: boolean;
};

export type Status = {
  id: string;
  name: string;
  running: boolean;
  state: LifeState;
  elapsed_secs: number | null;
  timeout_secs: number;
  failure: string | null;
  pid: number | null;
  port: number;
  port_open: boolean;
  login_probe: LoginProbe;
  log_path: string;
  pid_path: string;
  root: string;
  port_conflict: boolean;
  port_holder: string;
  /** 根目录有 package.json 但没有 node_modules。 */
  deps_missing?: boolean;
  /** 装依赖的命令，例如 "pnpm install"；不是前端项目时为空。 */
  install_command?: string;
};

export type LogTail = {
  path: string;
  missing: boolean;
  text: string;
};

export type ConfigInfo = {
  path: string | null;
  source: string;
  default_path: string;
};

export type ProjectDetail = {
  id: string;
  name: string;
  root: string;
  start: string;
  start_editable: boolean;
  stop: string;
  stop_editable: boolean;
  port: number;
  login_url: string;
  health_url: string;
  log_file: string;
  node: string | null;
  java: string | null;
  enabled: boolean;
  reason: string;
  start_timeout_secs?: number | null;
  stop_grace_secs?: number | null;
  auto_install?: boolean;
};

export type NewProjectReq = {
  id: string;
  name?: string | null;
  root: string;
  start: string;
  stop?: string | null;
  port?: number | null;
  login_url?: string | null;
  health_url?: string | null;
  log_file?: string | null;
  node?: string | null;
  java?: string | null;
  enabled?: boolean;
  reason?: string | null;
  start_timeout_secs?: number | null;
  stop_grace_secs?: number | null;
  auto_install?: boolean;
  background?: boolean | null;
  tomcat?: string | null;
};

export type ProjectEditReq = {
  name?: string | null;
  root?: string | null;
  start?: string | null;
  stop?: string | null;
  port?: number | null;
  login_url?: string | null;
  health_url?: string | null;
  log_file?: string | null;
  node?: string | null;
  java?: string | null;
  enabled?: boolean;
  reason?: string | null;
  start_timeout_secs?: number | null;
  stop_grace_secs?: number | null;
  auto_install?: boolean;
};

export type Confidence = "high" | "medium" | "low";

/** One project found by `scan_dir` (mirrors tongyun_dev_core::scan::Candidate). */
export type ScanCandidate = {
  id: string;
  name: string;
  root: string;
  kind: string;
  label: string;
  confidence: Confidence;
  start: string;
  stop: string | null;
  port: number;
  port_guessed: boolean;
  health_url: string | null;
  login_url: string | null;
  java: string | null;
  node: string | null;
  background: boolean;
  needs_tomcat: boolean;
  why: string[];
  existing_id: string | null;
};

export type ScanReport = {
  root: string;
  depth: number;
  dirs_visited: number;
  truncated: boolean;
  candidates: ScanCandidate[];
};

export type ImportReport = {
  added: ProjectRow[];
  skipped: { id: string; reason: string }[];
};
