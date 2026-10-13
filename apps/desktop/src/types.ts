export type LifeState = "stopped" | "installing" | "starting" | "port_open" | "ready" | "failed";

export type WebLink = {
  title: string;
  url: string;
};

export type ProfileConfig = {
  label?: string | null;
  start?: string | null;
  port?: number | null;
  env?: Record<string, string>;
};

export type Project = {
  id: string;
  name: string;
  group?: string | null;
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
  port_mode?: "none" | "arg" | "env" | "config" | string;
  port_arg?: string | null;
  port_env?: string | null;
  pty?: boolean;
  env?: Record<string, string>;
  auto_answers?: AutoAnswer[];
  hints?: string[];
  links?: WebLink[];
  default_profile?: string | null;
  profiles?: Record<string, ProfileConfig>;
  depends_on?: string[];
  /** 这一条解析失败（例如 log_dir 展不开）。列表里的其他项目仍然显示。 */
  resolve_error?: string | null;
};

export type AutoAnswer = {
  expect: string;
  send: string;
  once?: boolean;
  regex?: boolean;
};

export type ProjectRow = Project;

export type GroupInfo = {
  key: string;
  label: string;
  order: number | null;
  depends_on?: string[];
};

export type ProjectList = {
  config_path: string;
  projects: Project[];
  groups?: GroupInfo[];
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
  hints?: string[];
  active_profile?: string | null;
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
  group?: string | null;
  root: string;
  start: string;
  start_editable: boolean;
  stop: string;
  stop_editable: boolean;
  port: number;
  login_url: string;
  health_url: string;
  log_dir: string;
  log_file: string;
  node: string | null;
  java: string | null;
  enabled: boolean;
  reason: string;
  start_timeout_secs?: number | null;
  stop_grace_secs?: number | null;
  auto_install?: boolean;
  port_mode?: "none" | "arg" | "env" | "config" | string;
  port_arg?: string | null;
  port_env?: string | null;
  pty?: boolean;
  env?: Record<string, string>;
  auto_answers?: AutoAnswer[];
  hints?: string[];
  links?: WebLink[];
  default_profile?: string | null;
  profiles?: Record<string, ProfileConfig>;
  depends_on?: string[];
};

export type NewProjectReq = {
  id: string;
  name?: string | null;
  group?: string | null;
  root: string;
  start: string;
  stop?: string | null;
  port?: number | null;
  login_url?: string | null;
  health_url?: string | null;
  log_dir?: string | null;
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
  port_mode?: string | null;
  port_arg?: string | null;
  port_env?: string | null;
  pty?: boolean;
  env?: Record<string, string>;
  auto_answers?: AutoAnswer[];
  links?: WebLink[];
  default_profile?: string | null;
  profiles?: Record<string, ProfileConfig>;
  depends_on?: string[];
};

export type ProjectEditReq = {
  name?: string | null;
  group?: string | null;
  root?: string | null;
  start?: string | null;
  stop?: string | null;
  port?: number | null;
  login_url?: string | null;
  health_url?: string | null;
  log_dir?: string | null;
  log_file?: string | null;
  node?: string | null;
  java?: string | null;
  enabled?: boolean;
  reason?: string | null;
  start_timeout_secs?: number | null;
  stop_grace_secs?: number | null;
  auto_install?: boolean;
  port_mode?: string | null;
  port_arg?: string | null;
  port_env?: string | null;
  pty?: boolean;
  env?: Record<string, string>;
  auto_answers?: AutoAnswer[];
  links?: WebLink[];
  default_profile?: string | null;
  profiles?: Record<string, ProfileConfig>;
  depends_on?: string[];
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

export type BatchItemResult = {
  id: string;
  success: boolean;
  ok: boolean;
  message: string;
  elapsed_secs: number;
};

export type BatchOutput = {
  total: number;
  succeeded: number;
  failed: number;
  items: BatchItemResult[];
};

