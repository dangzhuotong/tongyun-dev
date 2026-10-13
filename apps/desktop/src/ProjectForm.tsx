import { invoke } from "@tauri-apps/api/core";
import { useEffect, useMemo, useState } from "react";
import type { GroupInfo, NewProjectReq, ProjectDetail, ProjectEditReq, ProjectRow } from "./types";
import {
  buildEditRequest,
  formatAutoAnswersLines,
  formatEnvLines,
  formatLinksLines,
  formatProfilesLines,
  parseAutoAnswersLines,
  parseEnvLines,
  parseLinksLines,
  parseProfilesLines,
  validateProjectForm,
  type ProjectFormData,
} from "./projectValidation";
import { getInitialGroupLabel, resolveFormGroup } from "./projectGroups";

export { validateProjectForm, type ProjectFormData };

interface ProjectFormProps {
  mode: "create" | "edit";
  initialDetail?: ProjectDetail | null;
  groups?: GroupInfo[];
  existingGroups?: string[];
  onClose: () => void;
  onSuccess: (projectId: string) => Promise<void> | void;
}

export function ProjectForm({
  mode,
  initialDetail,
  groups = [],
  existingGroups = [],
  onClose,
  onSuccess,
}: ProjectFormProps) {
  const isEditing = mode === "edit";
  const startEditable = initialDetail ? initialDetail.start_editable : true;
  const stopEditable = initialDetail ? initialDetail.stop_editable : true;

  const [form, setForm] = useState<ProjectFormData>(() => ({
    id: initialDetail?.id ?? "",
    name: initialDetail?.name ?? "",
    group: getInitialGroupLabel(initialDetail?.group, groups),
    root: initialDetail?.root ?? "",
    start: initialDetail?.start ?? "",
    stop: initialDetail?.stop ?? "",
    port: initialDetail ? (initialDetail.port ? String(initialDetail.port) : "") : "8080",
    login_url: initialDetail?.login_url ?? "",
    health_url: initialDetail?.health_url ?? "",
    log_dir: initialDetail?.log_dir ?? "",
    log_file: initialDetail?.log_file ?? "",
    node: initialDetail?.node ?? "",
    java: initialDetail?.java ?? "",
    enabled: initialDetail?.enabled ?? true,
    reason: initialDetail?.reason ?? "",
    start_timeout_secs:
      initialDetail?.start_timeout_secs != null
        ? String(initialDetail.start_timeout_secs)
        : "",
    auto_install: initialDetail?.auto_install ?? true,
    port_mode: initialDetail?.port_mode ?? "none",
    port_arg: initialDetail?.port_arg ?? "--port",
    port_env: initialDetail?.port_env ?? "PORT",
    pty: initialDetail?.pty ?? false,
    env_text: formatEnvLines(initialDetail?.env),
    auto_answers_text: formatAutoAnswersLines(initialDetail?.auto_answers),
    links_text: formatLinksLines(initialDetail?.links),
    default_profile: initialDetail?.default_profile ?? "",
    profiles_text: formatProfilesLines(initialDetail?.profiles),
  }));

  const [errors, setErrors] = useState<Record<string, string>>({});
  const [coreError, setCoreError] = useState("");
  const [saving, setSaving] = useState(false);

  // Close on Escape key
  useEffect(() => {
    function handleKeyDown(e: KeyboardEvent) {
      if (e.key === "Escape" && !saving) {
        onClose();
      }
    }
    window.addEventListener("keydown", handleKeyDown);
    return () => window.removeEventListener("keydown", handleKeyDown);
  }, [saving, onClose]);

  async function handlePickFolder() {
    try {
      const picked = await invoke<string | null>("pick_folder");
      if (picked) {
        setForm((prev) => ({ ...prev, root: picked }));
        setErrors((prev) => {
          const next = { ...prev };
          delete next.root;
          return next;
        });
      }
    } catch (err: unknown) {
      setCoreError(String(err));
    }
  }

  function handleChange(
    field: keyof ProjectFormData,
    value: string | number | boolean
  ) {
    setForm((prev) => ({ ...prev, [field]: value }));
    setErrors((prev) => {
      const next = { ...prev };
      delete next[field];
      return next;
    });
  }

  const availableGroups = useMemo(() => {
    const list: { key: string; label: string }[] = [];
    const seen = new Set<string>();
    for (const g of groups) {
      if (g.key && !seen.has(g.key)) {
        seen.add(g.key);
        list.push({ key: g.key, label: g.label || g.key });
      }
    }
    for (const k of existingGroups) {
      if (k && !seen.has(k)) {
        seen.add(k);
        list.push({ key: k, label: k });
      }
    }
    return list;
  }, [groups, existingGroups]);

  async function handleSubmit(e: React.FormEvent) {
    e.preventDefault();
    setCoreError("");

    const validationErrors = validateProjectForm(form, isEditing);
    if (Object.keys(validationErrors).length > 0) {
      setErrors(validationErrors);
      return;
    }

    setSaving(true);
    try {
      const resolved = resolveFormGroup(form.group, groups, existingGroups);
      if (resolved.newGroup) {
        await invoke("create_group", {
          key: resolved.newGroup.key,
          label: resolved.newGroup.label,
        });
      }

      if (isEditing) {
        const formWithGroup: ProjectFormData = {
          ...form,
          group: resolved.key ?? "",
        };
        const editReq = buildEditRequest(initialDetail!, formWithGroup) as ProjectEditReq;
        if (Object.keys(editReq).length === 0) {
          onClose();
          return;
        }
        await invoke<ProjectRow>("update_project", {
          id: form.id.trim(),
          edit: editReq,
        });
      } else {
        const createReq: NewProjectReq = {
          id: form.id.trim(),
          name: form.name.trim() || undefined,
          group: resolved.key || undefined,
          root: form.root.trim(),
          start: form.start.trim(),
          stop: form.stop.trim() || undefined,
          port: Number(form.port),
          login_url: form.login_url.trim() || undefined,
          health_url: form.health_url.trim() || undefined,
          log_dir: form.log_dir?.trim() || undefined,
          log_file: form.log_file.trim() || undefined,
          node: form.node.trim() || undefined,
          java: form.java.trim() || undefined,
          enabled: form.enabled,
          reason: form.reason.trim() || undefined,
          start_timeout_secs: form.start_timeout_secs
            ? Number(form.start_timeout_secs)
            : undefined,
          auto_install: form.auto_install === false ? false : undefined,
          port_mode: form.port_mode && form.port_mode !== "none" ? form.port_mode : undefined,
          port_arg:
            form.port_mode === "arg"
              ? form.port_arg?.trim() || "--port"
              : undefined,
          port_env:
            form.port_mode === "env"
              ? form.port_env?.trim() || "PORT"
              : undefined,
          pty: form.pty ? true : undefined,
          env: (() => {
            const { env: parsed } = parseEnvLines(form.env_text ?? "");
            return Object.keys(parsed).length > 0 ? parsed : undefined;
          })(),
          auto_answers: (() => {
            const { answers: parsed } = parseAutoAnswersLines(form.auto_answers_text ?? "");
            return parsed.length > 0 ? parsed : undefined;
          })(),
          links: (() => {
            const { links: parsed } = parseLinksLines(form.links_text ?? "");
            return parsed.length > 0 ? parsed : undefined;
          })(),
          default_profile: form.default_profile?.trim() || undefined,
          profiles: (() => {
            const { profiles: parsed } = parseProfilesLines(form.profiles_text ?? "");
            return Object.keys(parsed).length > 0 ? parsed : undefined;
          })(),
        };
        await invoke<ProjectRow>("add_project", { req: createReq });
      }

      await onSuccess(form.id.trim());
      onClose();
    } catch (err: unknown) {
      setCoreError(String(err));
    } finally {
      setSaving(false);
    }
  }

  return (
    <div
      className="modal-backdrop"
      role="dialog"
      aria-modal="true"
      onClick={(e) => {
        if (e.target === e.currentTarget && !saving) onClose();
      }}
    >
      <div className="modal-container">
        <div className="modal-header">
          <h2>{isEditing ? `项目设置 · ${initialDetail?.name || form.id}` : "新增项目"}</h2>
          <button
            type="button"
            className="modal-close"
            disabled={saving}
            onClick={onClose}
            aria-label="关闭"
          >
            ×
          </button>
        </div>

        <form onSubmit={handleSubmit} className="modal-form">
          {coreError && <div className="form-error-banner">{coreError}</div>}

          <div className="form-grid">
            {/* 项目 ID */}
            <div className="form-group">
              <label htmlFor="field-id">
                项目 ID <span className="req">*</span>
              </label>
              <input
                id="field-id"
                type="text"
                value={form.id}
                readOnly={isEditing}
                disabled={isEditing}
                placeholder="英文标识，如 my_api"
                onChange={(e) => handleChange("id", e.target.value)}
                className={errors.id ? "invalid" : ""}
              />
              {errors.id && <span className="field-error">{errors.id}</span>}
              {isEditing && (
                <span className="field-hint">项目 ID 是配置键名，不可修改</span>
              )}
            </div>

            {/* 项目名称 */}
            <div className="form-group">
              <label htmlFor="field-name">项目名称</label>
              <input
                id="field-name"
                type="text"
                value={form.name}
                placeholder="显示名称，缺省为 ID"
                onChange={(e) => handleChange("name", e.target.value)}
              />
            </div>
          </div>

          {/* 所属分组 */}
          <div className="form-group">
            <label htmlFor="field-group">所属分组</label>
            <input
              id="field-group"
              type="text"
              list="group-datalist"
              value={form.group ?? ""}
              placeholder="从已有分组选择，或输入新分组名称（可选）"
              onChange={(e) => handleChange("group", e.target.value)}
            />
            {availableGroups.length > 0 && (
              <datalist id="group-datalist">
                {availableGroups.map((g) => (
                  <option key={g.key} value={g.label} />
                ))}
              </datalist>
            )}
          </div>

          {/* 根目录 */}
          <div className="form-group">
            <label htmlFor="field-root">
              根目录 <span className="req">*</span>
            </label>
            <div className="input-with-button">
              <input
                id="field-root"
                type="text"
                value={form.root}
                placeholder="~ 或绝对路径，如 ~/code/myapp"
                onChange={(e) => handleChange("root", e.target.value)}
                className={errors.root ? "invalid" : ""}
              />
              <button
                type="button"
                className="btn-pick"
                disabled={saving}
                onClick={handlePickFolder}
              >
                选择…
              </button>
            </div>
            {errors.root && <span className="field-error">{errors.root}</span>}
          </div>

          {/* 启动命令 */}
          <div className="form-group">
            <label htmlFor="field-start">
              启动命令 <span className="req">*</span>
            </label>
            <textarea
              id="field-start"
              rows={3}
              value={form.start}
              readOnly={!startEditable}
              placeholder="启动 shell 脚本或命令，如 pnpm dev 或 mvn spring-boot:run"
              onChange={(e) => handleChange("start", e.target.value)}
              className={[
                errors.start ? "invalid" : "",
                !startEditable ? "readonly" : "",
              ]
                .filter(Boolean)
                .join(" ")}
            />
            {errors.start && <span className="field-error">{errors.start}</span>}
            {!startEditable && (
              <span className="field-hint warn">复杂命令请在配置文件里改</span>
            )}
          </div>

          {/* 停止命令 */}
          <div className="form-group">
            <label htmlFor="field-stop">停止命令</label>
            <input
              id="field-stop"
              type="text"
              value={form.stop}
              readOnly={!stopEditable}
              placeholder="可选；非 Tomcat 项目留空按端口进程组停止"
              onChange={(e) => handleChange("stop", e.target.value)}
              className={!stopEditable ? "readonly" : ""}
            />
            {!stopEditable && (
              <span className="field-hint warn">复杂命令请在配置文件里改</span>
            )}
          </div>

          <div className="form-grid">
            {/* 端口 */}
            <div className="form-group">
              <label htmlFor="field-port">
                端口 <span className="req">*</span>
              </label>
              <input
                id="field-port"
                type="number"
                min="1"
                max="65535"
                value={form.port}
                placeholder="如 8080"
                onChange={(e) => handleChange("port", e.target.value)}
                className={errors.port ? "invalid" : ""}
              />
              {errors.port && <span className="field-error">{errors.port}</span>}
            </div>

            {/* 启动超时秒数 */}
            <div className="form-group">
              <label htmlFor="field-timeout">启动超时秒数</label>
              <input
                id="field-timeout"
                type="number"
                min="1"
                value={form.start_timeout_secs}
                placeholder="缺省 180 秒"
                onChange={(e) => handleChange("start_timeout_secs", e.target.value)}
              />
            </div>
          </div>

          {/* 端口传递方式 */}
          <div className="form-grid">
            <div className="form-group">
              <label htmlFor="field-port-mode">端口传递方式</label>
              <select
                id="field-port-mode"
                value={form.port_mode || "none"}
                onChange={(e) => handleChange("port_mode", e.target.value)}
              >
                <option value="none">不传（只用于探活）</option>
                <option value="arg">命令行参数</option>
                <option value="env">环境变量 PORT</option>
                <option value="config">写在项目自己的配置里</option>
              </select>
              {errors.port_mode && <span className="field-error">{errors.port_mode}</span>}
            </div>

            {form.port_mode === "arg" && (
              <div className="form-group">
                <label htmlFor="field-port-arg">参数名</label>
                <input
                  id="field-port-arg"
                  type="text"
                  value={form.port_arg ?? ""}
                  placeholder="如 --port 或 -p"
                  onChange={(e) => handleChange("port_arg", e.target.value)}
                  className={errors.port_arg ? "invalid" : ""}
                />
                {errors.port_arg && <span className="field-error">{errors.port_arg}</span>}
              </div>
            )}

            {form.port_mode === "env" && (
              <div className="form-group">
                <label htmlFor="field-port-env">变量名</label>
                <input
                  id="field-port-env"
                  type="text"
                  value={form.port_env ?? ""}
                  placeholder="如 PORT 或 VITE_PORT"
                  onChange={(e) => handleChange("port_env", e.target.value)}
                  className={errors.port_env ? "invalid" : ""}
                />
                {errors.port_env && <span className="field-error">{errors.port_env}</span>}
              </div>
            )}
          </div>

          <div className="form-grid">
            {/* 登录页地址 */}
            <div className="form-group">
              <label htmlFor="field-login">登录页地址</label>
              <input
                id="field-login"
                type="text"
                value={form.login_url}
                placeholder="http://127.0.0.1:8080/...（探针就绪要求 HTTP 200/301/302）"
                onChange={(e) => handleChange("login_url", e.target.value)}
              />
            </div>

            {/* 健康检查地址 */}
            <div className="form-group">
              <label htmlFor="field-health">健康检查地址</label>
              <input
                id="field-health"
                type="text"
                value={form.health_url}
                placeholder="没写登录页时探针用它"
                onChange={(e) => handleChange("health_url", e.target.value)}
              />
            </div>
          </div>

          <div className="form-grid">
            {/* Java 版本 */}
            <div className="form-group">
              <label htmlFor="field-java">Java 版本</label>
              <input
                id="field-java"
                type="text"
                value={form.java}
                placeholder="如 8 或 17（自动注入 SDKMAN / Adoptium）"
                onChange={(e) => handleChange("java", e.target.value)}
              />
            </div>

            {/* Node 版本 */}
            <div className="form-group">
              <label htmlFor="field-node">Node 版本</label>
              <input
                id="field-node"
                type="text"
                value={form.node}
                placeholder="如 22（自动注入 fnm / nvm）"
                onChange={(e) => handleChange("node", e.target.value)}
              />
            </div>
          </div>

          <div className="form-group">
            <label htmlFor="field-log-dir">日志目录</label>
            <input
              id="field-log-dir"
              type="text"
              value={form.log_dir ?? ""}
              placeholder="默认 ~/.tongyun-dev/logs；可写 ~/logs、{{root}}/logs，或相对项目根的 logs（不能含 ..）"
              onChange={(e) => handleChange("log_dir", e.target.value)}
            />
          </div>

          {/* 日志文件 */}
          <div className="form-group">
            <label htmlFor="field-log">日志文件路径</label>
            <input
              id="field-log"
              type="text"
              value={form.log_file}
              placeholder="默认 ~/.tongyun-dev/logs/<id>.out"
              onChange={(e) => handleChange("log_file", e.target.value)}
            />
          </div>

          {/* 启用开关 + 停用原因 */}
          <div className="form-group checkbox-group">
            <label className="checkbox-label">
              <input
                type="checkbox"
                checked={form.enabled}
                onChange={(e) => handleChange("enabled", e.target.checked)}
              />
              <span>启用此项目</span>
            </label>
          </div>

          {/* 前端依赖：缺 node_modules 时先自动装 */}
          <div className="form-group checkbox-group">
            <label className="checkbox-label">
              <input
                type="checkbox"
                checked={form.auto_install !== false}
                onChange={(e) => handleChange("auto_install", e.target.checked)}
              />
              <span>缺 node_modules 时启动前自动装依赖（按锁文件用 pnpm / yarn / npm）</span>
            </label>
          </div>

          {/* 伪终端运行 */}
          <div className="form-group checkbox-group">
            <label className="checkbox-label">
              <input
                type="checkbox"
                checked={Boolean(form.pty)}
                onChange={(e) => handleChange("pty", e.target.checked)}
              />
              <span>伪终端运行（交互脚本用）</span>
            </label>
          </div>

          {/* 额外环境变量 */}
          <div className="form-group">
            <label htmlFor="field-env">额外环境变量</label>
            <textarea
              id="field-env"
              rows={3}
              value={form.env_text ?? ""}
              placeholder={"每行 KEY=VALUE，例如：\nCI=1\nAPP_ENV=dev"}
              onChange={(e) => handleChange("env_text", e.target.value)}
              className={errors.env_text ? "invalid" : ""}
            />
            {errors.env_text && <span className="field-error">{errors.env_text}</span>}
          </div>

          {/* 自动应答 */}
          <div className="form-group">
            <label htmlFor="field-auto-answers">自动应答</label>
            <textarea
              id="field-auto-answers"
              rows={3}
              value={form.auto_answers_text ?? ""}
              placeholder={"每行 匹配文字 => 发送内容，例如：\nSelect environment => dev\n(y/N) => y"}
              onChange={(e) => handleChange("auto_answers_text", e.target.value)}
              className={errors.auto_answers_text ? "invalid" : ""}
            />
            {errors.auto_answers_text && (
              <span className="field-error">{errors.auto_answers_text}</span>
            )}
          </div>

          {/* 快捷网页链接 */}
          <div className="form-group">
            <label htmlFor="field-links">快捷网页链接</label>
            <textarea
              id="field-links"
              rows={2}
              value={form.links_text ?? ""}
              placeholder={"每行「标题 => URL」或纯「URL」，例如：\n管理后台 => http://localhost:8080/admin\nSwagger => http://localhost:8080/swagger-ui.html"}
              onChange={(e) => handleChange("links_text", e.target.value)}
              className={errors.links_text ? "invalid" : ""}
            />
            {errors.links_text && <span className="field-error">{errors.links_text}</span>}
          </div>

          {/* 多环境预设 Profiles */}
          <div className="form-group">
            <label htmlFor="field-profiles">环境预设 (Profiles)</label>
            <textarea
              id="field-profiles"
              rows={3}
              value={form.profiles_text ?? ""}
              placeholder={"每行「标识: 启动命令」或「标识: 标签 | 启动命令 [| 端口]」，例如：\ndev: 本地开发 | pnpm dev | 3000\nprod: 生产预览 | pnpm preview | 4173"}
              onChange={(e) => handleChange("profiles_text", e.target.value)}
              className={errors.profiles_text ? "invalid" : ""}
            />
            {errors.profiles_text && <span className="field-error">{errors.profiles_text}</span>}
          </div>

          {/* 默认环境 */}
          <div className="form-group">
            <label htmlFor="field-default-profile">默认启动环境</label>
            <input
              id="field-default-profile"
              type="text"
              value={form.default_profile ?? ""}
              placeholder="留空则使用默认启动命令，或填写上面的环境标识（如 dev）"
              onChange={(e) => handleChange("default_profile", e.target.value)}
            />
          </div>

          {!form.enabled && (
            <div className="form-group">
              <label htmlFor="field-reason">
                停用原因 <span className="req">*</span>
              </label>
              <input
                id="field-reason"
                type="text"
                value={form.reason}
                placeholder="说明项目停用原因，如「环境迁移中」"
                onChange={(e) => handleChange("reason", e.target.value)}
                className={errors.reason ? "invalid" : ""}
              />
              {errors.reason && (
                <span className="field-error">{errors.reason}</span>
              )}
            </div>
          )}

          <div className="modal-actions">
            <button
              type="button"
              className="btn-cancel"
              disabled={saving}
              onClick={onClose}
            >
              取消
            </button>
            <button type="submit" className="primary btn-submit" disabled={saving}>
              {saving
                ? "保存中…"
                : isEditing
                ? "保存修改"
                : "创建项目"}
            </button>
          </div>
        </form>
      </div>
    </div>
  );
}
