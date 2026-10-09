export type Project = {
  id: string;
  name: string;
  root: string;
};

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
  pid: number | null;
  port: number;
  port_open: boolean;
  login_probe: LoginProbe;
  log_path: string;
  pid_path: string;
  root: string;
};

export type LogTail = {
  path: string;
  missing: boolean;
  text: string;
};
