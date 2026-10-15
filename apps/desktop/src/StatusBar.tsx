import type { Project, Status } from "./types";
import {
  FolderIcon,
  InfoIcon,
  KeyboardIcon,
  TerminalIcon,
} from "./icons";

export interface StatusBarProps {
  selectedProject?: Project;
  status: Status | null;
  totalProjects: number;
  runningCount: number;
  isRefreshing?: boolean;
  configPath?: string;
  onOpenConfigFile: () => void;
  onOpenTerminal?: () => void;
  onOpenShortcuts: () => void;
  onOpenAbout: () => void;
}

export function StatusBar({
  selectedProject,
  status,
  totalProjects,
  runningCount,
  isRefreshing = false,
  configPath,
  onOpenConfigFile,
  onOpenTerminal,
  onOpenShortcuts,
  onOpenAbout,
}: StatusBarProps) {
  const isRunning =
    status?.running ||
    status?.state === "ready" ||
    status?.state === "port_open";

  const isTransitioning =
    status?.state === "starting" || status?.state === "installing";

  return (
    <footer className="h-[26px] px-3 bg-[var(--color-surface)] border-t border-[var(--color-border)] flex items-center justify-between text-[11px] text-[var(--color-text-muted)] select-none shrink-0 z-20">
      {/* Left: Project or System Status */}
      <div className="flex items-center gap-3">
        {selectedProject ? (
          <div className="flex items-center gap-2">
            <span
              className={`w-2 h-2 rounded-full ${
                isRunning
                  ? "bg-[var(--color-accent)] animate-pulse"
                  : isTransitioning
                  ? "bg-[var(--ty-honey)] animate-ping"
                  : status?.state === "failed"
                  ? "bg-[var(--color-danger)]"
                  : "bg-[var(--color-text-subtle)]"
              }`}
            />
            <span className="font-semibold text-[var(--color-text)]">
              {selectedProject.name}
            </span>
            {selectedProject.port > 0 && (
              <span className="font-mono text-[var(--color-text-muted)]">
                :{selectedProject.port}
              </span>
            )}
            <span className="text-[var(--color-border-strong)]">|</span>
            <span className="text-[var(--color-text-muted)]">
              {status?.state === "ready"
                ? `就绪 (PID ${status.pid ?? "—"})`
                : status?.state === "port_open"
                ? `端口已通 (PID ${status.pid ?? "—"})`
                : status?.state === "starting"
                ? `启动中 (${status.elapsed_secs ?? 0}s)`
                : status?.state === "installing"
                ? `安装依赖中 (${status.elapsed_secs ?? 0}s)`
                : status?.state === "failed"
                ? "启动失败"
                : !selectedProject.configured
                ? "未配置"
                : "已停止"}
            </span>
          </div>
        ) : (
          <div className="flex items-center gap-2">
            <span className="w-2 h-2 rounded-full bg-[var(--color-accent)]" />
            <span className="font-medium text-[var(--color-text)]">就绪</span>
            <span className="text-[var(--color-border-strong)]">|</span>
            <span>
              共 {totalProjects} 个项目 · {runningCount} 个运行中
            </span>
          </div>
        )}

        {isRefreshing && (
          <div className="flex items-center gap-1.5 pl-2 border-l border-[var(--color-border)] text-[var(--color-accent)]">
            <span className="w-1.5 h-1.5 rounded-full bg-[var(--color-accent)] animate-ping" />
            <span className="text-[10px]">刷新中...</span>
          </div>
        )}
      </div>

      {/* Right: Config info, shortcuts & tools */}
      <div className="flex items-center gap-3">
        {/* Config file path */}
        {configPath && (
          <button
            type="button"
            onClick={onOpenConfigFile}
            className="hidden md:flex items-center gap-1 font-mono text-[10px] text-[var(--color-text-muted)] hover:text-[var(--color-text)] hover:underline cursor-pointer"
            title={`点击在编辑器中打开配置: ${configPath}`}
          >
            <FolderIcon width={11} height={11} className="text-[var(--color-accent)]" />
            <span className="max-w-[180px] truncate">{configPath}</span>
          </button>
        )}

        {/* Web console hint */}
        <span className="hidden sm:inline font-mono text-[10px] text-[var(--color-text-subtle)]" title="网页控制台服务地址">
          :9099
        </span>

        {/* Terminal quick launcher */}
        {onOpenTerminal && selectedProject && (
          <button
            type="button"
            onClick={onOpenTerminal}
            className="flex items-center gap-1 hover:text-[var(--color-accent)] cursor-pointer transition-colors px-1.5 py-0.5 rounded hover:bg-[var(--color-surface-raised)]"
            title="在系统独立终端中打开"
          >
            <TerminalIcon width={12} height={12} />
            <span className="hidden lg:inline">终端</span>
          </button>
        )}

        {/* Keyboard Shortcuts */}
        <button
          type="button"
          onClick={onOpenShortcuts}
          className="flex items-center gap-1 hover:text-[var(--color-accent)] cursor-pointer transition-colors px-1.5 py-0.5 rounded hover:bg-[var(--color-surface-raised)]"
          title="键盘快捷键帮助 (/ 或 ?)"
        >
          <KeyboardIcon width={12} height={12} />
          <span className="hidden sm:inline">快捷键</span>
        </button>

        {/* About Dialog */}
        <button
          type="button"
          onClick={onOpenAbout}
          className="hover:text-[var(--color-text)] cursor-pointer transition-colors p-1 rounded hover:bg-[var(--color-surface-raised)]"
          title="关于 tongyun-dev"
        >
          <InfoIcon width={12} height={12} />
        </button>
      </div>
    </footer>
  );
}
