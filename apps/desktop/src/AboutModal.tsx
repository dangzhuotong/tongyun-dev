import { CloseIcon, InfoIcon } from "./icons";
import logoUrl from "./assets/logo.svg";
import { useEffect } from "react";

export interface AboutModalProps {
  isOpen: boolean;
  onClose: () => void;
  version?: string;
  configPath?: string;
  checkingUpdate?: boolean;
  onCheckUpdate?: () => void;
}

export function AboutModal({
  isOpen,
  onClose,
  version = "0.2.0",
  configPath,
  checkingUpdate = false,
  onCheckUpdate,
}: AboutModalProps) {
  useEffect(() => {
    if (!isOpen) return;
    const handleKeyDown = (e: KeyboardEvent) => {
      if (e.key === "Escape") onClose();
    };
    window.addEventListener("keydown", handleKeyDown);
    return () => window.removeEventListener("keydown", handleKeyDown);
  }, [isOpen, onClose]);

  if (!isOpen) return null;

  return (
    <div className="fixed inset-0 z-50 flex items-center justify-center bg-black/50 backdrop-blur-xs p-4 select-none animate-in fade-in duration-100">
      <div className="w-full max-w-md rounded-xl border border-[var(--color-border)] bg-[var(--color-surface)] shadow-2xl overflow-hidden flex flex-col">
        {/* Header */}
        <div className="flex items-center justify-between px-4 py-3 border-b border-[var(--color-border)] bg-[var(--color-surface-raised)]">
          <div className="flex items-center gap-2">
            <span className="p-1 rounded-md bg-[var(--color-accent)]/15 text-[var(--color-accent)]">
              <InfoIcon width={16} height={16} />
            </span>
            <h2 className="text-sm font-semibold text-[var(--color-text)]">
              关于 tongyun-dev
            </h2>
          </div>
          <button
            type="button"
            onClick={onClose}
            title="关闭 (Esc)"
            aria-label="关闭"
            className="p-1 rounded-md text-[var(--color-text-muted)] hover:text-[var(--color-text)] hover:bg-[var(--color-surface)] transition-colors cursor-pointer"
          >
            <CloseIcon width={16} height={16} />
          </button>
        </div>

        {/* Content */}
        <div className="p-6 text-center space-y-4">
          <div className="w-16 h-16 mx-auto rounded-2xl bg-[var(--color-accent)]/10 text-[var(--color-accent)] border border-[var(--color-accent)]/20 flex items-center justify-center shadow-xs">
            <img src={logoUrl} alt="tongyun-dev" width={36} height={40} />
          </div>

          <div>
            <h3 className="text-lg font-bold text-[var(--color-text)]">
              tongyun-dev
            </h3>
            <p className="text-xs text-[var(--color-text-muted)] mt-1">
              本机微服务与多项目协同开发管理工具 · 服务 AI 与人
            </p>
            <div className="inline-block mt-2 px-2.5 py-0.5 rounded-full bg-[var(--color-accent)]/10 border border-[var(--color-accent)]/20 text-[var(--color-accent)] text-xs font-mono font-medium">
              当前版本 v{version}
            </div>
          </div>

          {/* Details */}
          <div className="p-3.5 rounded-xl border border-[var(--color-border)] bg-[var(--color-surface-raised)] text-left text-xs space-y-2">
            <div className="flex justify-between items-center">
              <span className="text-[var(--color-text-muted)]">核心架构</span>
              <span className="font-medium text-[var(--color-text)]">Rust + Tauri 2 + React 19</span>
            </div>
            <div className="flex justify-between items-center">
              <span className="text-[var(--color-text-muted)]">网页控制台</span>
              <span className="font-mono text-[var(--color-text)]">http://127.0.0.1:9099</span>
            </div>
            {configPath && (
              <div className="flex flex-col gap-0.5 pt-1 border-t border-[var(--color-border)]/50">
                <span className="text-[var(--color-text-muted)]">项目配置</span>
                <span className="font-mono text-[11px] text-[var(--color-text)] break-all">{configPath}</span>
              </div>
            )}
          </div>

          <div className="pt-2 text-[11px] text-[var(--color-text-subtle)]">
            橦云异梦 · Tongyun Ecosystem
          </div>
        </div>

        {/* Footer */}
        <div className="px-4 py-2.5 bg-[var(--color-surface-raised)] border-t border-[var(--color-border)] flex justify-end gap-2">
          {onCheckUpdate && (
            <button
              type="button"
              onClick={onCheckUpdate}
              disabled={checkingUpdate}
              className="px-4 py-1.5 rounded-md text-xs font-medium border border-[var(--color-border)] text-[var(--color-text)] hover:bg-[var(--color-surface)] transition-colors cursor-pointer disabled:opacity-50 disabled:cursor-not-allowed"
            >
              {checkingUpdate ? "正在检查更新…" : "检查更新"}
            </button>
          )}
          <button
            type="button"
            onClick={onClose}
            className="px-4 py-1.5 rounded-md text-xs font-medium bg-[var(--color-accent)] text-white hover:opacity-90 transition-opacity cursor-pointer"
          >
            确定
          </button>
        </div>
      </div>
    </div>
  );
}
