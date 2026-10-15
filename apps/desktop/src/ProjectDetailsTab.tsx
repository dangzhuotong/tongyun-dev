import { useState } from "react";
import type { Project, Status } from "./types";
import {
  CheckIcon,
  CopyIcon,
  FolderIcon,
  GearIcon,
  TerminalIcon,
} from "./icons";

export interface ProjectDetailsTabProps {
  project: Project;
  status: Status | null;
  onEdit: () => void;
  onOpenFolder: () => void;
}

export function ProjectDetailsTab({
  project,
  status,
  onEdit,
  onOpenFolder,
}: ProjectDetailsTabProps) {
  const [copiedKey, setCopiedKey] = useState<string | null>(null);

  const copyToClipboard = (key: string, text: string) => {
    navigator.clipboard.writeText(text);
    setCopiedKey(key);
    setTimeout(() => {
      setCopiedKey(null);
    }, 1500);
  };

  const envEntries = Object.entries(project.env || {});
  const profileEntries = Object.entries(project.profiles || {});

  return (
    <div className="flex-1 p-6 overflow-y-auto space-y-6 max-w-4xl text-xs">
      {/* Header card */}
      <div className="p-4 rounded-xl border border-[var(--color-border)] bg-[var(--color-surface)] shadow-xs flex items-center justify-between">
        <div className="space-y-1">
          <div className="flex items-center gap-2">
            <h2 className="text-sm font-bold text-[var(--color-text)]">{project.name}</h2>
            <code className="px-1.5 py-0.5 rounded font-mono text-[11px] bg-[var(--color-surface-raised)] border border-[var(--color-border)]">
              {project.id}
            </code>
            {project.group && (
              <span className="px-2 py-0.5 rounded-full text-[10px] bg-[var(--color-surface-raised)] text-[var(--color-text-muted)]">
                {project.group}
              </span>
            )}
          </div>
          <p className="text-[var(--color-text-muted)]">
            根目录: <span className="font-mono text-[var(--color-text)]">{project.root}</span>
          </p>
        </div>

        <div className="flex items-center gap-2">
          <button
            type="button"
            onClick={onOpenFolder}
            className="flex items-center gap-1.5 px-3 py-1.5 rounded-md border border-[var(--color-border)] bg-[var(--color-surface)] hover:bg-[var(--color-surface-raised)] transition-colors cursor-pointer"
            title="在系统资源管理器中打开项目目录"
          >
            <FolderIcon width={13} height={13} className="text-[var(--color-accent)]" />
            <span>打开目录</span>
          </button>
          <button
            type="button"
            onClick={onEdit}
            className="flex items-center gap-1.5 px-3 py-1.5 rounded-md bg-[var(--color-accent)] text-white hover:opacity-90 transition-opacity cursor-pointer shadow-xs font-medium"
          >
            <GearIcon width={13} height={13} />
            <span>编辑配置</span>
          </button>
        </div>
      </div>

      {/* Commands & Runtime Settings */}
      <div className="rounded-xl border border-[var(--color-border)] bg-[var(--color-surface)] shadow-xs overflow-hidden">
        <div className="px-4 py-2.5 bg-[var(--color-surface-raised)] border-b border-[var(--color-border)] flex items-center justify-between font-semibold text-[var(--color-text)]">
          <span>命令与端口配置</span>
          <TerminalIcon width={14} height={14} className="text-[var(--color-text-muted)]" />
        </div>
        <div className="p-4 grid grid-cols-1 md:grid-cols-2 gap-4 text-[11px]">
          <div>
            <span className="text-[var(--color-text-muted)] block mb-1">启动命令 (start)</span>
            <code className="block p-2 rounded bg-[var(--color-bg)] border border-[var(--color-border)] font-mono text-[var(--color-text)] break-all select-all">
              {/* @ts-expect-error detail or string */}
              {project.start || "—"}
            </code>
          </div>

          <div>
            <span className="text-[var(--color-text-muted)] block mb-1">停止命令 (stop)</span>
            <code className="block p-2 rounded bg-[var(--color-bg)] border border-[var(--color-border)] font-mono text-[var(--color-text)] break-all select-all">
              {/* @ts-expect-error detail or string */}
              {project.stop || "(按 PID kill)"}
            </code>
          </div>

          <div>
            <span className="text-[var(--color-text-muted)] block mb-1">服务端口 (port)</span>
            <div className="p-2 rounded bg-[var(--color-bg)] border border-[var(--color-border)] font-mono text-[var(--color-text)] flex items-center justify-between">
              <span>{project.port > 0 ? project.port : "未配置"}</span>
              {project.port_mode && (
                <span className="text-[10px] px-1.5 py-0.2 rounded bg-[var(--color-surface-raised)] text-[var(--color-text-muted)]">
                  模式: {project.port_mode}
                </span>
              )}
            </div>
          </div>

          <div>
            <span className="text-[var(--color-text-muted)] block mb-1">依赖安装命令</span>
            <code className="block p-2 rounded bg-[var(--color-bg)] border border-[var(--color-border)] font-mono text-[var(--color-text)]">
              {status?.install_command || "自动识别 (pnpm/yarn/npm)"}
            </code>
          </div>
        </div>
      </div>

      {/* Toolchain & Dependencies */}
      <div className="rounded-xl border border-[var(--color-border)] bg-[var(--color-surface)] shadow-xs overflow-hidden">
        <div className="px-4 py-2.5 bg-[var(--color-surface-raised)] border-b border-[var(--color-border)] font-semibold text-[var(--color-text)]">
          运行环境与依赖关系
        </div>
        <div className="p-4 grid grid-cols-1 sm:grid-cols-3 gap-4 text-[11px]">
          <div>
            <span className="text-[var(--color-text-muted)] block mb-1">Node.js</span>
            <div className="p-2 rounded bg-[var(--color-bg)] border border-[var(--color-border)] font-mono text-[var(--color-text)]">
              {project.node ? `Node ${project.node}` : "系统默认"}
            </div>
          </div>
          <div>
            <span className="text-[var(--color-text-muted)] block mb-1">Java</span>
            <div className="p-2 rounded bg-[var(--color-bg)] border border-[var(--color-border)] font-mono text-[var(--color-text)]">
              {project.java ? `Java ${project.java}` : "系统默认"}
            </div>
          </div>
          <div>
            <span className="text-[var(--color-text-muted)] block mb-1">服务依赖 (depends_on)</span>
            <div className="p-2 rounded bg-[var(--color-bg)] border border-[var(--color-border)] text-[var(--color-text)] truncate">
              {project.depends_on && project.depends_on.length > 0
                ? project.depends_on.join(", ")
                : "无"}
            </div>
          </div>
        </div>
      </div>

      {/* Environment Variables */}
      {envEntries.length > 0 && (
        <div className="rounded-xl border border-[var(--color-border)] bg-[var(--color-surface)] shadow-xs overflow-hidden">
          <div className="px-4 py-2.5 bg-[var(--color-surface-raised)] border-b border-[var(--color-border)] flex items-center justify-between font-semibold text-[var(--color-text)]">
            <span>环境变量 ({envEntries.length})</span>
          </div>
          <div className="divide-y divide-[var(--color-border)]/60 max-h-60 overflow-y-auto">
            {envEntries.map(([k, v]) => (
              <div key={k} className="px-4 py-2 flex items-center justify-between hover:bg-[var(--color-surface-raised)]/50 transition-colors">
                <span className="font-mono font-semibold text-[var(--color-accent-text)]">{k}</span>
                <div className="flex items-center gap-2">
                  <span className="font-mono text-[var(--color-text-muted)] max-w-sm truncate select-all">{v}</span>
                  <button
                    type="button"
                    onClick={() => copyToClipboard(k, v)}
                    className="p-1 rounded text-[var(--color-text-subtle)] hover:text-[var(--color-text)] cursor-pointer"
                    title="复制值"
                  >
                    {copiedKey === k ? <CheckIcon width={12} height={12} className="text-[var(--color-accent)]" /> : <CopyIcon width={12} height={12} />}
                  </button>
                </div>
              </div>
            ))}
          </div>
        </div>
      )}

      {/* Web Links */}
      {project.links && project.links.length > 0 && (
        <div className="rounded-xl border border-[var(--color-border)] bg-[var(--color-surface)] shadow-xs overflow-hidden">
          <div className="px-4 py-2.5 bg-[var(--color-surface-raised)] border-b border-[var(--color-border)] font-semibold text-[var(--color-text)]">
            关联网页 ({project.links.length})
          </div>
          <div className="divide-y divide-[var(--color-border)]/60">
            {project.links.map((link, idx) => (
              <div key={idx} className="px-4 py-2.5 flex items-center justify-between">
                <span className="font-medium text-[var(--color-text)]">{link.title || `链接 ${idx + 1}`}</span>
                <a
                  href={link.url}
                  target="_blank"
                  rel="noreferrer"
                  className="font-mono text-[var(--color-accent)] hover:underline truncate max-w-md"
                >
                  {link.url}
                </a>
              </div>
            ))}
          </div>
        </div>
      )}

      {/* Environment Profiles */}
      {profileEntries.length > 0 && (
        <div className="rounded-xl border border-[var(--color-border)] bg-[var(--color-surface)] shadow-xs overflow-hidden">
          <div className="px-4 py-2.5 bg-[var(--color-surface-raised)] border-b border-[var(--color-border)] font-semibold text-[var(--color-text)]">
            环境预设配置 (Profiles)
          </div>
          <div className="divide-y divide-[var(--color-border)]/60">
            {profileEntries.map(([key, prof]) => (
              <div key={key} className="px-4 py-2.5 flex items-center justify-between">
                <div>
                  <span className="font-semibold text-[var(--color-text)]">{prof.label || key}</span>
                  <span className="font-mono text-[var(--color-text-muted)] ml-2">({key})</span>
                </div>
                {prof.port && (
                  <span className="font-mono text-[var(--color-text-muted)]">端口: {prof.port}</span>
                )}
              </div>
            ))}
          </div>
        </div>
      )}
    </div>
  );
}
