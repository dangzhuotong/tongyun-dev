import { useState, useRef, useEffect } from "react";
import type { ConfigInfo, Project, ProjectList, Status } from "./types";
import logoUrl from "./assets/logo.svg";
import {
  ChevronDownIcon,
  FolderIcon,
  GearIcon,
  MoonIcon,
  PlusIcon,
  RefreshIcon,
  SearchIcon,
  SunIcon,
  FileIcon,
} from "./icons";

export interface ToolbarProps {
  projects: ProjectList | null;
  selected: string;
  onSelectProject: (id: string) => void;
  currentProject?: Project;
  status: Status | null;
  busy?: "" | "start" | "stop" | "restart" | "install";
  canStart?: boolean;
  canStop?: boolean;
  canRestart?: boolean;
  canOpenPage?: boolean;
  onStart?: () => void;
  onStop?: () => void;
  onRestart?: () => void;
  onOpenPage?: () => void;
  onOpenAllPages?: () => void;
  onOpenTerminal?: () => void;
  onOpenCreate: () => void;
  onOpenScan: () => void;
  onOpenEdit: () => void;
  onRefresh: () => void;
  isRefreshing: boolean;
  theme: "system" | "light" | "dark";
  onToggleTheme: () => void;
  selectedProfile: string;
  onProfileChange: (profile: string) => void;
  onStartBatch: () => void;
  onStopBatch: () => void;
  configInfo: ConfigInfo | null;
  onPickConfig: () => void;
  onOpenConfigFile: () => void;
  onReloadConfig: () => void;
  appVersion: string;
  checkingUpdate?: boolean;
  onCheckUpdate: () => void;
}

export function Toolbar({
  projects,
  selected,
  onSelectProject,
  currentProject,
  status,
  busy = "",
  onOpenCreate,
  onOpenScan,
  onRefresh,
  isRefreshing,
  theme,
  onToggleTheme,
  selectedProfile,
  onProfileChange,
  onStartBatch,
  onStopBatch,
  configInfo,
  onPickConfig,
  onOpenConfigFile,
  onReloadConfig,
  appVersion,
  checkingUpdate = false,
  onCheckUpdate,
}: ToolbarProps) {
  const [projectDropdownOpen, setProjectDropdownOpen] = useState(false);
  const [projectFilter, setProjectFilter] = useState("");
  const [settingsOpen, setSettingsOpen] = useState(false);

  const projectDropdownRef = useRef<HTMLDivElement>(null);
  const settingsDropdownRef = useRef<HTMLDivElement>(null);

  useEffect(() => {
    function handleClickOutside(event: MouseEvent) {
      const target = event.target as Node;
      if (projectDropdownRef.current && !projectDropdownRef.current.contains(target)) {
        setProjectDropdownOpen(false);
      }
      if (settingsDropdownRef.current && !settingsDropdownRef.current.contains(target)) {
        setSettingsOpen(false);
      }
    }
    document.addEventListener("mousedown", handleClickOutside);
    return () => document.removeEventListener("mousedown", handleClickOutside);
  }, []);

  const projectList = projects?.projects ?? [];
  const filteredProjects = projectFilter.trim()
    ? projectList.filter((p) =>
        [p.name, p.id, String(p.port || "")].some((s) =>
          s.toLowerCase().includes(projectFilter.trim().toLowerCase())
        )
      )
    : projectList;

  return (
    <header className="h-[40px] px-3 flex items-center justify-between border-b border-[var(--color-border)] bg-[var(--color-surface)] text-xs text-[var(--color-text)] select-none shrink-0 z-30">
      {/* Left: App Logo, Project Selector & Quick Creation */}
      <div className="flex items-center gap-2">
        <img src={logoUrl} alt="tongyun-dev" width={20} height={22} className="shrink-0" />
        <div className="h-3.5 w-px bg-[var(--color-border)] shrink-0" />

        {/* Project Selector Dropdown */}
        <div className="relative" ref={projectDropdownRef}>
          <button
            type="button"
            onClick={() => setProjectDropdownOpen(!projectDropdownOpen)}
            className="flex items-center gap-1.5 px-2.5 py-1 rounded hover:bg-[var(--color-surface-raised)] transition-colors font-medium text-xs border border-transparent hover:border-[var(--color-border)]"
            title="选择或切换项目"
          >
            <FolderIcon width={14} height={14} className="text-[var(--color-accent)] shrink-0" />
            <span className="max-w-[150px] truncate font-semibold">
              {currentProject ? currentProject.name : "未选择项目"}
            </span>
            <ChevronDownIcon width={12} height={12} className="text-[var(--color-text-muted)] shrink-0" />
          </button>

          {projectDropdownOpen && (
            <div className="absolute left-0 mt-1 w-72 rounded-lg border border-[var(--color-border)] bg-[var(--color-surface)] shadow-xl z-50 py-1.5 flex flex-col text-xs animate-in fade-in zoom-in-95 duration-100">
              <div className="px-2.5 py-1.5 border-b border-[var(--color-border)]/60">
                <input
                  type="search"
                  autoFocus
                  placeholder="过滤项目..."
                  value={projectFilter}
                  onChange={(e) => setProjectFilter(e.target.value)}
                  className="w-full px-2 py-1 text-xs rounded border border-[var(--color-border)] bg-[var(--color-bg)] text-[var(--color-text)] focus:border-[var(--color-accent)] focus:outline-none"
                />
              </div>

              <div className="max-h-60 overflow-y-auto py-1">
                {filteredProjects.length === 0 ? (
                  <div className="px-3 py-2 text-[var(--color-text-muted)] text-center">无匹配项目</div>
                ) : (
                  filteredProjects.map((p) => {
                    const isSelected = p.id === selected;
                    return (
                      <button
                        key={p.id}
                        type="button"
                        onClick={() => {
                          onSelectProject(p.id);
                          setProjectDropdownOpen(false);
                        }}
                        className={`w-full text-left px-3 py-1.5 flex items-center justify-between hover:bg-[var(--color-surface-raised)] transition-colors ${
                          isSelected ? "bg-[var(--color-selection)] text-[var(--color-accent-text)] font-semibold" : ""
                        }`}
                      >
                        <span className="truncate flex-1">{p.name}</span>
                        {p.port > 0 && (
                          <span className="font-mono text-[11px] text-[var(--color-text-muted)] ml-2">
                            :{p.port}
                          </span>
                        )}
                      </button>
                    );
                  })
                )}
              </div>

              <div className="pt-1.5 mt-1 border-t border-[var(--color-border)]/60 px-2 flex items-center gap-1">
                <button
                  type="button"
                  onClick={() => {
                    setProjectDropdownOpen(false);
                    onOpenScan();
                  }}
                  className="flex-1 px-2 py-1 rounded hover:bg-[var(--color-surface-raised)] text-left flex items-center gap-1.5 text-[11px]"
                >
                  <SearchIcon width={12} height={12} className="text-[var(--color-accent)]" />
                  <span>扫描目录...</span>
                </button>
                <button
                  type="button"
                  onClick={() => {
                    setProjectDropdownOpen(false);
                    onOpenCreate();
                  }}
                  className="flex-1 px-2 py-1 rounded hover:bg-[var(--color-surface-raised)] text-left flex items-center gap-1.5 text-[11px]"
                >
                  <PlusIcon width={12} height={12} className="text-[var(--color-accent)]" />
                  <span>新建项目...</span>
                </button>
              </div>
            </div>
          )}
        </div>

        {/* Quick Tools */}
        <button
          type="button"
          onClick={onOpenScan}
          className="flex items-center gap-1 px-2 py-1 rounded border border-[var(--color-border)] hover:bg-[var(--color-surface-raised)] transition-colors text-xs font-medium cursor-pointer"
          title="扫描本地目录自动识别项目"
        >
          <SearchIcon width={13} height={13} />
          <span>扫描</span>
        </button>

        <button
          type="button"
          onClick={onOpenCreate}
          className="flex items-center gap-1 px-2 py-1 rounded border border-[var(--color-border)] hover:bg-[var(--color-surface-raised)] transition-colors text-xs font-medium cursor-pointer"
          title="手动登记项目配置"
        >
          <PlusIcon width={13} height={13} />
          <span>新建</span>
        </button>
      </div>

      {/* Center: Clean project indicator or status summary */}
      <div className="hidden md:flex items-center gap-2">
        {currentProject ? (
          <div className="flex items-center gap-2 px-3 py-1 rounded-full bg-[var(--color-surface-raised)] border border-[var(--color-border)] text-xs shadow-xs">
            <span
              className={`w-2 h-2 rounded-full ${
                status?.state === "ready"
                  ? "bg-emerald-500 shadow-[0_0_8px_rgba(16,185,129,0.5)]"
                  : status?.state === "starting" || status?.state === "installing"
                  ? "bg-amber-500 animate-pulse"
                  : status?.state === "failed"
                  ? "bg-rose-500"
                  : "bg-slate-400"
              }`}
            />
            <span className="font-semibold text-[var(--color-text)]">{currentProject.name}</span>
            {currentProject.port ? (
              <span className="font-mono text-[11px] text-[var(--color-text-muted)]">
                :{currentProject.port}
              </span>
            ) : null}
            {currentProject.group && (
              <span className="text-[10px] px-1.5 py-0.2 rounded bg-[var(--color-surface)] text-[var(--color-text-muted)] border border-[var(--color-border)]">
                {currentProject.group}
              </span>
            )}
          </div>
        ) : (
          <span className="text-xs text-[var(--color-text-muted)]">
            选择左侧项目进行启停控制与日志监控
          </span>
        )}
      </div>

      {/* Right: Environment, Batch Tools, Theme, Settings */}
      <div className="flex items-center gap-2">
        {/* Profile Selector if available */}
        {currentProject?.profiles && Object.keys(currentProject.profiles).length > 0 && (
          <div className="flex items-center gap-1">
            <span className="text-[11px] text-[var(--color-text-muted)]">环境:</span>
            <select
              value={selectedProfile}
              disabled={busy !== ""}
              onChange={(e) => onProfileChange(e.target.value)}
              className="px-2 py-0.5 rounded border border-[var(--color-border)] bg-[var(--color-surface)] text-[11px] font-medium text-[var(--color-text)] focus:border-[var(--color-accent)] focus:outline-none cursor-pointer"
              title="选择启动环境预设"
            >
              <option value="">默认配置</option>
              {Object.entries(currentProject.profiles).map(([key, prof]) => (
                <option key={key} value={key}>
                  {prof.label ? `${prof.label} (${key})` : key}
                </option>
              ))}
            </select>
          </div>
        )}

        {/* Batch Start/Stop */}
        <div className="hidden lg:flex items-center gap-1">
          <button
            type="button"
            disabled={busy !== ""}
            onClick={onStartBatch}
            className="px-2 py-1 rounded text-[11px] text-[var(--color-text-muted)] hover:text-[var(--color-text)] hover:bg-[var(--color-surface-raised)] transition-colors cursor-pointer disabled:opacity-50"
            title="按依赖顺序启动全部项目"
          >
            全启
          </button>
          <button
            type="button"
            disabled={busy !== ""}
            onClick={onStopBatch}
            className="px-2 py-1 rounded text-[11px] text-[var(--color-text-muted)] hover:text-[var(--color-text)] hover:bg-[var(--color-surface-raised)] transition-colors cursor-pointer disabled:opacity-50"
            title="按逆依赖顺序停止全部项目"
          >
            全停
          </button>
        </div>

        <div className="h-3.5 w-px bg-[var(--color-border)] shrink-0" />

        {/* Refresh button */}
        <button
          type="button"
          onClick={onRefresh}
          className="p-1.5 rounded hover:bg-[var(--color-surface-raised)] text-[var(--color-text-muted)] hover:text-[var(--color-text)] transition-colors cursor-pointer"
          title="刷新所有项目状态"
        >
          <RefreshIcon width={14} height={14} className={isRefreshing ? "animate-spin text-[var(--color-accent)]" : ""} />
        </button>

        {/* Theme Toggle */}
        <button
          type="button"
          onClick={onToggleTheme}
          className="p-1.5 rounded hover:bg-[var(--color-surface-raised)] text-[var(--color-text-muted)] hover:text-[var(--color-text)] transition-colors cursor-pointer"
          title={`切换主题（当前：${theme === "dark" ? "暗色" : theme === "light" ? "亮色" : "跟随系统"}）`}
        >
          {theme === "dark" ? <MoonIcon width={14} height={14} /> : <SunIcon width={14} height={14} />}
        </button>

        {/* Settings Menu */}
        <div className="relative" ref={settingsDropdownRef}>
          <button
            type="button"
            onClick={() => setSettingsOpen(!settingsOpen)}
            className={`p-1.5 rounded hover:bg-[var(--color-surface-raised)] text-[var(--color-text-muted)] hover:text-[var(--color-text)] transition-colors cursor-pointer ${
              settingsOpen ? "bg-[var(--color-surface-raised)] text-[var(--color-text)]" : ""
            }`}
            title="设置与配置管理"
          >
            <GearIcon width={14} height={14} />
          </button>

          {settingsOpen && (
            <div className="absolute right-0 mt-1 w-64 rounded-lg border border-[var(--color-border)] bg-[var(--color-surface)] shadow-xl z-50 py-1.5 text-xs animate-in fade-in zoom-in-95 duration-100">
              <div className="px-3 py-2 border-b border-[var(--color-border)]/60 flex items-center justify-between gap-2">
                <div className="min-w-0">
                  <span className="text-[10px] uppercase font-bold text-[var(--color-text-muted)]">当前版本</span>
                  <p className="font-mono text-[11px] text-[var(--color-text)] mt-0.5">v{appVersion}</p>
                </div>
                <button
                  type="button"
                  disabled={checkingUpdate}
                  onClick={() => {
                    setSettingsOpen(false);
                    onCheckUpdate();
                  }}
                  className="shrink-0 px-2 py-1 rounded-md text-[11px] font-medium bg-[var(--color-accent)] text-white hover:opacity-90 disabled:opacity-50 disabled:cursor-not-allowed cursor-pointer"
                >
                  {checkingUpdate ? "正在检查…" : "检查更新"}
                </button>
              </div>
              <div className="px-3 py-1.5 border-b border-[var(--color-border)]/60">
                <span className="text-[10px] uppercase font-bold text-[var(--color-text-muted)]">配置文件</span>
                <p className="font-mono text-[11px] text-[var(--color-text)] truncate mt-0.5" title={configInfo?.path || ""}>
                  {configInfo?.path || configInfo?.default_path || "—"}
                </p>
              </div>

              <div className="py-1">
                <button
                  type="button"
                  onClick={() => {
                    setSettingsOpen(false);
                    onReloadConfig();
                  }}
                  className="w-full text-left px-3 py-1.5 flex items-center gap-2 hover:bg-[var(--color-surface-raised)] transition-colors"
                >
                  <RefreshIcon width={13} height={13} />
                  <span>重新加载配置</span>
                </button>
                <button
                  type="button"
                  onClick={() => {
                    setSettingsOpen(false);
                    onPickConfig();
                  }}
                  className="w-full text-left px-3 py-1.5 flex items-center gap-2 hover:bg-[var(--color-surface-raised)] transition-colors"
                >
                  <FolderIcon width={13} height={13} />
                  <span>切换配置文件...</span>
                </button>
                <button
                  type="button"
                  onClick={() => {
                    setSettingsOpen(false);
                    onOpenConfigFile();
                  }}
                  className="w-full text-left px-3 py-1.5 flex items-center gap-2 hover:bg-[var(--color-surface-raised)] transition-colors"
                >
                  <FileIcon width={13} height={13} />
                  <span>用编辑器打开 projects.toml</span>
                </button>
              </div>
            </div>
          )}
        </div>
      </div>
    </header>
  );
}
