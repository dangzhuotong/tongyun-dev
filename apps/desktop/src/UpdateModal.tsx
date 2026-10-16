import { useState } from "react";
import type { Update } from "@tauri-apps/plugin-updater";
import { AlertIcon, CloseIcon, RefreshIcon } from "./icons";
import { downloadAndInstallAppUpdate } from "./updater";

interface UpdateModalProps {
  update: Update;
  onClose: () => void;
}

export function UpdateModal({ update, onClose }: UpdateModalProps) {
  const [installing, setInstalling] = useState(false);
  const [downloadedBytes, setDownloadedBytes] = useState(0);
  const [totalBytes, setTotalBytes] = useState<number | undefined>(undefined);
  const [error, setError] = useState<string | null>(null);

  const handleUpdate = async () => {
    setInstalling(true);
    setError(null);
    try {
      await downloadAndInstallAppUpdate(update, (progress) => {
        if (progress.event === "Started" && progress.contentLength) {
          setTotalBytes(progress.contentLength);
        } else if (progress.event === "Progress" && progress.chunkLength) {
          const chunkLength = progress.chunkLength;
          setDownloadedBytes((prev) => prev + chunkLength);
        }
      });
    } catch (err) {
      console.error("[Updater] Install failed:", err);
      const message = err instanceof Error ? err.message : String(err);
      setError(message);
      setInstalling(false);
    }
  };

  const percent =
    totalBytes && totalBytes > 0
      ? Math.min(100, Math.round((downloadedBytes / totalBytes) * 100))
      : null;

  return (
    <div
      className="fixed inset-0 z-50 flex items-center justify-center bg-black/50 backdrop-blur-xs p-4 select-none"
      onClick={installing ? undefined : onClose}
    >
      <div
        className="w-full max-w-md rounded-xl border border-[var(--color-border)] bg-[var(--color-surface)] shadow-2xl overflow-hidden flex flex-col"
        onClick={(e) => e.stopPropagation()}
        role="dialog"
        aria-labelledby="update-title"
      >
        <div className="flex items-center justify-between px-4 py-3 border-b border-[var(--color-border)] bg-[var(--color-surface-raised)]">
          <div className="flex items-center gap-2 min-w-0">
            <span className="p-1 rounded-md bg-[var(--color-accent)]/15 text-[var(--color-accent)]">
              <RefreshIcon width={16} height={16} />
            </span>
            <h2 id="update-title" className="text-sm font-semibold text-[var(--color-text)]">
              发现新版本
            </h2>
            <span className="text-xs px-2 py-0.5 rounded-full bg-[var(--color-accent)] text-white font-mono">
              v{update.version}
            </span>
          </div>
          {!installing && (
            <button
              type="button"
              onClick={onClose}
              title="稍后"
              aria-label="关闭"
              className="p-1 rounded-md text-[var(--color-text-muted)] hover:text-[var(--color-text)] hover:bg-[var(--color-surface)] transition-colors cursor-pointer"
            >
              <CloseIcon width={16} height={16} />
            </button>
          )}
        </div>

        <div className="p-4 space-y-3 text-xs">
          <p className="text-[var(--color-text-muted)]">
            {update.currentVersion ? `当前版本 v${update.currentVersion.replace(/^v/i, "")}` : "有新版本可以安装"}
            {update.date ? ` · ${update.date.slice(0, 10)}` : ""}
          </p>

          {update.body && (
            <div className="max-h-48 overflow-y-auto p-3 rounded-xl border border-[var(--color-border)] bg-[var(--color-surface-raised)] text-[var(--color-text)] whitespace-pre-wrap leading-relaxed">
              {update.body}
            </div>
          )}

          <p className="text-[var(--color-text)] leading-relaxed">
            是否立即下载并安装？安装完成后应用会自动重启。
          </p>

          {installing && (
            <div className="space-y-2">
              <div className="flex items-center justify-between text-[var(--color-text-muted)]">
                <span>正在下载并安装…</span>
                {percent !== null && <span>{percent}%</span>}
              </div>
              <div className="w-full h-2 bg-[var(--color-border)] rounded-full overflow-hidden">
                <div
                  className="h-full bg-[var(--color-accent)] rounded-full transition-all duration-200"
                  style={{ width: percent !== null ? `${percent}%` : "30%" }}
                />
              </div>
            </div>
          )}

          {error && (
            <div className="p-3 rounded-xl border border-[var(--color-danger)]/40 bg-[var(--color-danger)]/10 flex items-start gap-2 text-[var(--color-danger)]">
              <AlertIcon width={16} height={16} className="shrink-0 mt-0.5" />
              <div className="break-all">更新失败：{error}</div>
            </div>
          )}
        </div>

        <div className="px-4 py-2.5 bg-[var(--color-surface-raised)] border-t border-[var(--color-border)] flex justify-end gap-2">
          {!installing && (
            <button
              type="button"
              onClick={onClose}
              className="px-4 py-1.5 rounded-md text-xs font-medium border border-[var(--color-border)] text-[var(--color-text)] hover:bg-[var(--color-surface)] cursor-pointer"
            >
              稍后
            </button>
          )}
          <button
            type="button"
            onClick={handleUpdate}
            disabled={installing}
            className="px-4 py-1.5 rounded-md text-xs font-medium bg-[var(--color-accent)] text-white hover:opacity-90 transition-opacity cursor-pointer disabled:opacity-50 disabled:cursor-not-allowed"
          >
            {installing ? "正在下载并安装…" : "立即更新并重启"}
          </button>
        </div>
      </div>
    </div>
  );
}
