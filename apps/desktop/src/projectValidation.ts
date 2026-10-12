import type { AutoAnswer } from "./types";

export interface ProjectFormData {
  id: string;
  name: string;
  group?: string;
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
  /** 缺 node_modules 时启动前自动装依赖（缺省 true）。 */
  auto_install?: boolean;
  port_mode?: string;
  port_arg?: string;
  port_env?: string;
  pty?: boolean;
  env_text?: string;
  auto_answers_text?: string;
}

/**
 * 解析每行 KEY=VALUE 的环境变量文本。
 * 忽略空行；非法行报错并指出行号。
 */
export function parseEnvLines(text: string): {
  env: Record<string, string>;
  error?: string;
} {
  const env: Record<string, string> = {};
  if (!text || text.trim() === "") return { env };
  const lines = text.split("\n");
  for (let i = 0; i < lines.length; i++) {
    const raw = lines[i];
    const line = raw.trim();
    if (!line) continue;
    const eqIdx = line.indexOf("=");
    if (eqIdx === -1) {
      return {
        env: {},
        error: `第 ${i + 1} 行环境变量格式错误（必须为 KEY=VALUE）：${line}`,
      };
    }
    const key = line.slice(0, eqIdx).trim();
    if (!key) {
      return {
        env: {},
        error: `第 ${i + 1} 行环境变量键名不能为空：${line}`,
      };
    }
    const val = line.slice(eqIdx + 1).trim();
    env[key] = val;
  }
  return { env };
}

/**
 * 将环境变量映射格式化为每行 KEY=VALUE 文本。
 */
export function formatEnvLines(env?: Record<string, string> | null): string {
  if (!env) return "";
  return Object.entries(env)
    .map(([k, v]) => `${k}=${v}`)
    .join("\n");
}

/**
 * 解析每行「匹配文字 => 发送内容」的自动应答规则。
 * 忽略空行；非法行报错并指出行号。
 */
export function parseAutoAnswersLines(text: string): {
  answers: AutoAnswer[];
  error?: string;
} {
  const answers: AutoAnswer[] = [];
  if (!text || text.trim() === "") return { answers };
  const lines = text.split("\n");
  for (let i = 0; i < lines.length; i++) {
    const raw = lines[i];
    const line = raw.trim();
    if (!line) continue;
    const arrowIdx = line.indexOf("=>");
    if (arrowIdx === -1) {
      return {
        answers: [],
        error: `第 ${i + 1} 行自动应答格式错误（必须为 匹配文字 => 发送内容）：${line}`,
      };
    }
    const expect = line.slice(0, arrowIdx).trim();
    if (!expect) {
      return {
        answers: [],
        error: `第 ${i + 1} 行自动应答匹配文字不能为空：${line}`,
      };
    }
    const send = line.slice(arrowIdx + 2).trim();
    answers.push({ expect, send, once: true, regex: false });
  }
  return { answers };
}

/**
 * 将自动应答规则数组格式化为每行「匹配文字 => 发送内容」文本。
 */
export function formatAutoAnswersLines(
  answers?: AutoAnswer[] | null
): string {
  if (!answers || answers.length === 0) return "";
  return answers.map((a) => `${a.expect} => ${a.send}`).join("\n");
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

  if (data.port_mode !== undefined && data.port_mode !== "") {
    const mode = data.port_mode.trim();
    if (!["none", "arg", "env", "config"].includes(mode)) {
      errors.port_mode = "端口传递方式必须为 none、arg、env 或 config";
    }
  }

  if (data.port_mode === "arg" && data.port_arg !== undefined && data.port_arg.trim() === "") {
    errors.port_arg = "参数名不能为空（如 --port）";
  }

  if (data.port_mode === "env" && data.port_env !== undefined && data.port_env.trim() === "") {
    errors.port_env = "环境变量名不能为空（如 PORT）";
  }

  if (data.enabled === false) {
    const reason = data.reason?.trim() ?? "";
    if (!reason) {
      errors.reason = "停用项目时必须填写停用原因";
    }
  }

  if (data.env_text !== undefined) {
    const { error: envError } = parseEnvLines(data.env_text);
    if (envError) {
      errors.env_text = envError;
    }
  }

  if (data.auto_answers_text !== undefined) {
    const { error: answersError } = parseAutoAnswersLines(data.auto_answers_text);
    if (answersError) {
      errors.auto_answers_text = answersError;
    }
  }

  return errors;
}

export interface EditBaseline {
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
  log_file: string;
  node?: string | null;
  java?: string | null;
  enabled: boolean;
  reason: string;
  start_timeout_secs?: number | null;
  auto_install?: boolean;
  port_mode?: string | null;
  port_arg?: string | null;
  port_env?: string | null;
  pty?: boolean;
  env?: Record<string, string>;
  auto_answers?: AutoAnswer[];
}

/**
 * Edit request containing only fields the user actually changed, so untouched
 * keys (e.g. a multi-line `start = '''…'''`) stay byte-for-byte in projects.toml.
 * Empty string = remove the key; timeout 0 = remove.
 */
export function buildEditRequest(
  base: EditBaseline,
  form: ProjectFormData
): Record<string, any> {
  const out: Record<string, any> = {};
  const text = (key: string, before: string | null | undefined, after: string) => {
    if ((before ?? "").trim() !== after.trim()) out[key] = after.trim();
  };
  text("name", base.name, form.name);
  text("group", base.group, form.group ?? "");
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
  if (form.auto_install !== undefined && form.auto_install !== (base.auto_install ?? true)) {
    out.auto_install = form.auto_install;
  }

  const formPortMode = (form.port_mode ?? "none").trim();
  const basePortMode = (base.port_mode ?? "none").trim();
  if (formPortMode !== basePortMode) {
    out.port_mode = formPortMode;
  }

  if (formPortMode === "arg") {
    const formArg = (form.port_arg ?? "").trim();
    const baseArg = (base.port_arg ?? "").trim();
    if (formArg !== baseArg) {
      out.port_arg = formArg;
    }
  } else if (base.port_arg && base.port_arg.trim() !== "") {
    out.port_arg = "";
  }

  if (formPortMode === "env") {
    const formEnv = (form.port_env ?? "").trim();
    const baseEnv = (base.port_env ?? "").trim();
    if (formEnv !== baseEnv) {
      out.port_env = formEnv;
    }
  } else if (base.port_env && base.port_env.trim() !== "") {
    out.port_env = "";
  }

  // pty
  const formPty = Boolean(form.pty);
  const basePty = Boolean(base.pty);
  if (formPty !== basePty) {
    out.pty = formPty;
  }

  // env
  if (form.env_text !== undefined) {
    const { env: parsedEnv } = parseEnvLines(form.env_text);
    const baseEnv = base.env ?? {};
    const keysParsed = Object.keys(parsedEnv);
    const keysBase = Object.keys(baseEnv);
    let envChanged = keysParsed.length !== keysBase.length;
    if (!envChanged) {
      for (const k of keysParsed) {
        if (parsedEnv[k] !== baseEnv[k]) {
          envChanged = true;
          break;
        }
      }
    }
    if (envChanged) {
      out.env = parsedEnv;
    }
  }

  // auto_answers
  if (form.auto_answers_text !== undefined) {
    const { answers: parsedAnswers } = parseAutoAnswersLines(form.auto_answers_text);
    const baseAnswers = base.auto_answers ?? [];
    let answersChanged = parsedAnswers.length !== baseAnswers.length;
    if (!answersChanged) {
      for (let i = 0; i < parsedAnswers.length; i++) {
        if (
          parsedAnswers[i].expect !== baseAnswers[i].expect ||
          parsedAnswers[i].send !== baseAnswers[i].send
        ) {
          answersChanged = true;
          break;
        }
      }
    }
    if (answersChanged) {
      out.auto_answers = parsedAnswers;
    }
  }

  return out;
}
