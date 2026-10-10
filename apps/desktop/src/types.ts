export type LifeState = "stopped" | "starting" | "port_open" | "ready" | "failed";

export type Project = {
  id: string;
  name: string;
  root: string;
  enabled: boolean;
  port: number;
  reason: string;
  start_timeout_secs?: number;
  stop_grace_secs?: number;
  node?: string | null;
  java?: string | null;
  java_home?: string | null;
  node_home?: string | null;
  shell?: string | null;
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
  state: "stopped" | "starting" | "port_open" | "ready" | "failed";
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
