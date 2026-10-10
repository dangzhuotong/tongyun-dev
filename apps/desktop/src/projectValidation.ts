export interface ProjectFormData {
  id: string;
  name: string;
  root: string;
  start: string;
  stop: string;
  port: number | string;
  login_url: string;
  health_url: string;
  log_file: string;
  node: string;
  java: string;
  enabled: boolean;
  reason: string;
  start_timeout_secs: number | string;
}

export function validateProjectForm(
  data: Partial<ProjectFormData>,
  isEditing = false
): Record<string, string> {
  const errors: Record<string, string> = {};

  if (!isEditing) {
    const id = data.id?.trim() ?? "";
    if (!id) {
      errors.id = "项目 ID 为必填项";
    } else if (!/^[a-zA-Z0-9_-]{1,64}$/.test(id)) {
      errors.id = "项目 ID 只能包含字母、数字、下划线和连字符，长度为 1-64 位";
    }
  }

  const root = data.root?.trim() ?? "";
  if (!root) {
    errors.root = "根目录为必填项";
  }

  const start = data.start?.trim() ?? "";
  if (!start) {
    errors.start = "启动命令为必填项";
  }

  const portRaw = data.port !== undefined && data.port !== "" ? Number(data.port) : NaN;
  if (isNaN(portRaw) || !Number.isInteger(portRaw) || portRaw <= 0 || portRaw > 65535) {
    errors.port = "端口必须为 1-65535 之间的有效端口";
  }

  if (data.enabled === false) {
    const reason = data.reason?.trim() ?? "";
    if (!reason) {
      errors.reason = "停用项目时必须填写停用原因";
    }
  }

  return errors;
}

export interface EditBaseline {
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
  node?: string | null;
  java?: string | null;
  enabled: boolean;
  reason: string;
  start_timeout_secs?: number | null;
}

/**
 * Edit request containing only fields the user actually changed, so untouched
 * keys (e.g. a multi-line `start = '''…'''`) stay byte-for-byte in projects.toml.
 * Empty string = remove the key; timeout 0 = remove.
 */
export function buildEditRequest(
  base: EditBaseline,
  form: ProjectFormData
): Record<string, string | number | boolean> {
  const out: Record<string, string | number | boolean> = {};
  const text = (key: string, before: string | null | undefined, after: string) => {
    if ((before ?? "").trim() !== after.trim()) out[key] = after.trim();
  };
  text("name", base.name, form.name);
  text("root", base.root, form.root);
  if (base.start_editable) text("start", base.start, form.start);
  if (base.stop_editable) text("stop", base.stop, form.stop);
  const port = Number(form.port);
  if (port !== base.port) out.port = port;
  text("login_url", base.login_url, form.login_url);
  text("health_url", base.health_url, form.health_url);
  text("log_file", base.log_file, form.log_file);
  text("node", base.node, form.node);
  text("java", base.java, form.java);
  if (form.enabled !== base.enabled) out.enabled = form.enabled;
  if (form.enabled) {
    if (base.reason.trim() !== "") out.reason = "";
  } else {
    text("reason", base.reason, form.reason);
  }
  const timeout = form.start_timeout_secs === "" ? 0 : Number(form.start_timeout_secs);
  if (timeout !== (base.start_timeout_secs ?? 0)) out.start_timeout_secs = timeout;
  return out;
}
