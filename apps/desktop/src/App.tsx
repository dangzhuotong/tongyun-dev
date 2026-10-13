import { invoke } from "@tauri-apps/api/core";
import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import type { ConfigInfo, LogTail, ProjectDetail, ProjectList, Status } from "./types";
import { LogPanel } from "./LogPanel";
import { stateShortLabel } from "./logFormat";
import { ProjectForm } from "./ProjectForm";
import { ScanDialog } from "./ScanDialog";
import { countByFilter, filterProjects, STATUS_FILTERS, type StatusFilter } from "./projectFilter";
import { canStopProject, stopButtonTitle } from "./buttonState";
import logoUrl from "./assets/logo.svg";
import {
  AlertIcon,
  ChevronDownIcon,
  ChevronRightIcon,
  CpuIcon,
  DoorIcon,
  ExternalIcon,
  FileIcon,
  FolderIcon,
  GearIcon,
  MoreIcon,
  PlayIcon,
  PlugIcon,
  PlusIcon,
  PulseIcon,
  RefreshIcon,
  RestartIcon,
  SearchIcon,
  StopIcon,
  TerminalIcon,
  TrashIcon,
  XIcon,
} from "./icons";
import {
  generateGroupKey,
  groupProjects,
  isGroupCollapsed,
  shouldShowGroupHeaders,
} from "./projectGroups";

const LOG_LINES = 500;
const COLLAPSED_STORAGE_KEY = "tongyun-dev:collapsed-groups";

function loadCollapsedGroups(): Record<string, boolean> {
  try {
    const raw = localStorage.getItem(COLLAPSED_STORAGE_KEY);
    return raw ? JSON.parse(raw) : {};
  } catch {
    return {};
  }
}

function saveCollapsedGroups(map: Record<string, boolean>) {
  try {
    localStorage.setItem(COLLAPSED_STORAGE_KEY, JSON.stringify(map));
  } catch {
    // ignore
  }
}

export function App() {
  const [configInfo, setConfigInfo] = useState<ConfigInfo | null>(null);
  const [missingConfig, setMissingConfig] = useState(false);
  const [projects, setProjects] = useState<ProjectList | null>(null);
  const [selected, setSelected] = useState("");
  const [status, setStatus] = useState<Status | null>(null);
  const [allStatuses, setAllStatuses] = useState<Record<string, Status>>({});
  const [log, setLog] = useState<LogTail | null>(null);
  const [error, setError] = useState("");
  const [busy, setBusy] = useState<"" | "start" | "stop" | "restart" | "install">("");
  const busyRef = useRef(busy);
  busyRef.current = busy;

  const [formModal, setFormModal] = useState<{
    open: boolean;
    mode: "create" | "edit";
    detail: ProjectDetail | null;
  }>({ open: false, mode: "create", detail: null });

  const [scanOpen, setScanOpen] = useState(false);
  const [query, setQuery] = useState("");
  const [statusFilter, setStatusFilter] = useState<StatusFilter>("all");
  const [menu, setMenu] = useState<"" | "settings" | "more">("");
  const searchRef = useRef<HTMLInputElement>(null);

  const [collapsedGroups, setCollapsedGroups] = useState<Record<string, boolean>>(() =>
    loadCollapsedGroups()
  );
  const [dragOverGroup, setDragOverGroup] = useState<string | null>(null);
  const [createGroupOpen, setCreateGroupOpen] = useState(false);
  const [newGroupLabel, setNewGroupLabel] = useState("");
  const [selectedProfile, setSelectedProfile] = useState<string>("");
  const [pagesDropdownOpen, setPagesDropdownOpen] = useState(false);

  const toggleGroupCollapse = useCallback((key: string) => {
    setCollapsedGroups((prev) => {
      const next = { ...prev, [key]: !prev[key] };
      saveCollapsedGroups(next);
      return next;
    });
  }, []);

  // 「/」或 Cmd/Ctrl+K 聚焦项目搜索；在其他输入框里不拦截「/」
  useEffect(() => {
    function onKey(e: KeyboardEvent) {
      const target = e.target as HTMLElement | null;
      const typing =
        !!target &&
        (target.tagName === "INPUT" || target.tagName === "TEXTAREA" || target.isContentEditable);
      if ((e.metaKey || e.ctrlKey) && e.key.toLowerCase() === "k") {
        e.preventDefault();
        searchRef.current?.focus();
        searchRef.current?.select();
      } else if (e.key === "/" && !typing && !document.querySelector(".modal-backdrop")) {
        e.preventDefault();
        searchRef.current?.focus();
      } else if (e.key === "Escape") {
        if (menu) setMenu("");
        if (pagesDropdownOpen) setPagesDropdownOpen(false);
      }
    }
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [menu, pagesDropdownOpen]);

  // 点菜单外面关闭菜单
  useEffect(() => {
    if (!menu && !pagesDropdownOpen) return;
    function onDown(e: MouseEvent) {
      const el = e.target as HTMLElement | null;
      if (menu && !el?.closest(".menu-anchor")) {
        setMenu("");
      }
      if (pagesDropdownOpen && !el?.closest(".pages-split-btn")) {
        setPagesDropdownOpen(false);
      }
    }
    window.addEventListener("mousedown", onDown);
    return () => window.removeEventListener("mousedown", onDown);
  }, [menu, pagesDropdownOpen]);

  const refresh = useCallback(async (id: string) => {
    const [nextStatus, nextLog] = await Promise.all([
      invoke<Status>("project_status", { id }),
      invoke<LogTail>("log_tail", { id, lines: LOG_LINES }),
    ]);
    setStatus(nextStatus);
    setAllStatuses((prev) => ({ ...prev, [nextStatus.id]: nextStatus }));
    setLog((prev) => {
      if (
        prev &&
        prev.path === nextLog.path &&
        prev.missing === nextLog.missing &&
        prev.text === nextLog.text
      ) {
        return prev;
      }
      return nextLog;
    });
  }, []);

  const loadProjects = useCallback(async () => {
    setError("");
    try {
      const info = await invoke<ConfigInfo>("config_info");
      setConfigInfo(info);
      if (!info.path) {
        setMissingConfig(true);
        setProjects(null);
        setSelected("");
        return;
      }
      const list = await invoke<ProjectList>("list_projects");
      setProjects(list);
      setMissingConfig(false);
      setSelected((prev) => {
        if (prev && list.projects.some((p) => p.id === prev)) return prev;
        const preferred = list.projects.find((row) => row.enabled);
        return preferred?.id ?? "";
      });
    } catch (err: unknown) {
      const errStr = String(err);
      if (errStr.toLowerCase().includes("not found")) {
        setMissingConfig(true);
        setProjects(null);
        setSelected("");
      } else {
        setError(errStr);
      }
    }
  }, []);

  useEffect(() => {
    loadProjects();
  }, [loadProjects]);

  // Periodic poll of all project statuses every 3 seconds
  useEffect(() => {
    let cancelled = false;
    const fetchAll = async () => {
      try {
        const list = await invoke<Status[]>("all_status");
        if (!cancelled) {
          setAllStatuses((prev) => {
            const next = { ...prev };
            for (const s of list) {
              next[s.id] = s;
            }
            return next;
          });
        }
      } catch {
        // periodic status poll errors are ignored
      }
    };
    fetchAll();
    const timer = window.setInterval(fetchAll, 3000);
    return () => {
      cancelled = true;
      window.clearInterval(timer);
    };
  }, []);

  // Poll selected project status and log every 1 second
  useEffect(() => {
    if (!selected) return;
    const broken = projects?.projects.find((p) => p.id === selected)?.resolve_error;
    if (broken) {
      setStatus(null);
      setError(broken);
      return;
    }
    let cancelled = false;
    const tick = () => {
      if (cancelled) return;
      refresh(selected).catch((err: unknown) => {
        if (!cancelled && !busyRef.current) setError(String(err));
      });
    };
    tick();
    const timer = window.setInterval(tick, 1000);
    return () => {
      cancelled = true;
      window.clearInterval(timer);
    };
  }, [selected, refresh, projects]);

  const prevSelectedRef = useRef<string>("");
  const prevActiveProfileRef = useRef<string | null | undefined>(undefined);

  useEffect(() => {
    if (!selected) {
      setSelectedProfile("");
      prevSelectedRef.current = "";
      prevActiveProfileRef.current = undefined;
      return;
    }
    const proj = projects?.projects.find((p) => p.id === selected);
    const currentActive = status?.active_profile;

    if (prevSelectedRef.current !== selected) {
      // 切换项目：选项目当前运行环境或默认配置
      setSelectedProfile(currentActive || proj?.default_profile || "");
      prevSelectedRef.current = selected;
      prevActiveProfileRef.current = currentActive;
    } else if (prevActiveProfileRef.current !== currentActive) {
      // 外部状态变化（如刚启动或停止）
      if (currentActive) {
        setSelectedProfile(currentActive);
      } else if (prevActiveProfileRef.current !== undefined) {
        // 项目由运行变为停止，恢复为默认配置
        setSelectedProfile(proj?.default_profile || "");
      }
      prevActiveProfileRef.current = currentActive;
    }
  }, [selected, projects, status?.active_profile]);

  async function handlePickConfig() {
    setError("");
    try {
      const info = await invoke<ConfigInfo | null>("pick_config_file");
      if (info && info.path) {
        setConfigInfo(info);
        await loadProjects();
      }
    } catch (err: unknown) {
      setError(String(err));
    }
  }

  async function handleCreateDefault() {
    setError("");
    try {
      const info = await invoke<ConfigInfo>("create_default_config");
      setConfigInfo(info);
      await loadProjects();
    } catch (err: unknown) {
      setError(String(err));
    }
  }

  async function handleOpenConfigFile() {
    setError("");
    try {
      await invoke("open_config_file");
    } catch (err: unknown) {
      setError(String(err));
    }
  }

  async function handleOpenScan() {
    setError("");
    if (missingConfig) {
      // First run: create ~/.config/tongyun-dev/projects.toml so the scan has somewhere to write.
      try {
        const info = await invoke<ConfigInfo>("create_default_config");
        setConfigInfo(info);
        await loadProjects();
      } catch (err: unknown) {
        setError(String(err));
        return;
      }
    }
    setScanOpen(true);
  }

  function handleOpenCreate() {
    setError("");
    setFormModal({
      open: true,
      mode: "create",
      detail: null,
    });
  }

  async function handleOpenEdit() {
    if (!selected) return;
    setError("");
    try {
      const detail = await invoke<ProjectDetail>("project_detail", { id: selected });
      setFormModal({
        open: true,
        mode: "edit",
        detail,
      });
    } catch (err: unknown) {
      setError(String(err));
    }
  }

  async function handleDeleteProject() {
    if (!current || !selected) return;
    const isRunning =
      status?.running ||
      status?.state === "starting" ||
      status?.state === "port_open" ||
      status?.state === "ready";
    if (isRunning) {
      window.alert(`项目「${current.name || selected}」正在运行中，请先停止项目再删除。`);
      return;
    }
    const confirmed = window.confirm(
      `确定要删除项目「${current.name || selected}」吗？\n只从配置里移除，不删项目文件。`
    );
    if (!confirmed) return;

    setError("");
    try {
      await invoke("remove_project", { id: selected });
      await loadProjects();
    } catch (err: unknown) {
      setError(String(err));
    }
  }

  async function handleStart(profileOverride?: string) {
    if (!selected || busyRef.current) return;
    const project = projects?.projects.find((row) => row.id === selected);
    if (!project || !project.enabled || !project.configured) return;
    setBusy("start");
    setError("");
    try {
      const prof = profileOverride !== undefined ? profileOverride : (selectedProfile || undefined);
      const next = await invoke<Status>("start_project", { id: selected, profile: prof });
      setStatus(next);
      setAllStatuses((prev) => ({ ...prev, [next.id]: next }));
    } catch (err: unknown) {
      setError(String(err));
      await refresh(selected).catch(() => undefined);
    } finally {
      setBusy("");
    }
  }

  async function handleStop() {
    if (!selected || busyRef.current) return;
    setBusy("stop");
    setError("");
    try {
      const next = await invoke<Status>("stop_project", { id: selected });
      setStatus(next);
      setAllStatuses((prev) => ({ ...prev, [next.id]: next }));
    } catch (err: unknown) {
      setError(String(err));
    } finally {
      await refresh(selected).catch(() => undefined);
      setBusy("");
    }
  }

  async function handleInstall() {
    if (!selected || busyRef.current) return;
    setBusy("install");
    setError("");
    try {
      const next = await invoke<Status>("install_deps", { id: selected });
      setStatus(next);
      setAllStatuses((prev) => ({ ...prev, [next.id]: next }));
    } catch (err: unknown) {
      setError(String(err));
    } finally {
      await refresh(selected).catch(() => undefined);
      setBusy("");
    }
  }

  async function handleRestart(profileOverride?: string) {
    if (!selected || busyRef.current) return;
    setBusy("restart");
    setError("");
    try {
      const prof = profileOverride !== undefined ? profileOverride : (selectedProfile || undefined);
      const next = await invoke<Status>("restart_project", { id: selected, profile: prof });
      setStatus(next);
      setAllStatuses((prev) => ({ ...prev, [next.id]: next }));
    } catch (err: unknown) {
      setError(String(err));
    } finally {
      await refresh(selected).catch(() => undefined);
      setBusy("");
    }
  }

  async function handleOpenTerminal(profileOverride?: string) {
    if (!selected) return;
    setError("");
    try {
      const prof = profileOverride !== undefined ? profileOverride : (selectedProfile || undefined);
      await invoke("open_terminal", { id: selected, profile: prof });
    } catch (err: unknown) {
      setError(String(err));
    }
  }

  async function handleOpenAllPages() {
    setError("");
    try {
      if (current?.links && current.links.length > 0) {
        await invoke("open_pages", { urls: current.links.map((l) => l.url) });
      } else {
        await reveal("page");
      }
    } catch (err: unknown) {
      setError(String(err));
    }
  }

  async function reveal(kind: "log" | "page") {
    setError("");
    try {
      if (kind === "log") {
        const path = log?.path || status?.log_path || "";
        await invoke("open_log", { path });
      } else {
        await invoke("open_page", { url: pageUrl(status, current) });
      }
    } catch (err: unknown) {
      setError(String(err));
    }
  }

  const current = projects?.projects.find((row) => row.id === selected) ?? null;
  const page = pageUrl(status, current);
  const logPath = log?.path || status?.log_path || "";
  const canOpenLog = Boolean(logPath) && !log?.missing;

  // Button permissions
  const isStoppedOrFailed = status ? status.state === "stopped" || status.state === "failed" : false;
  const canStart = Boolean(
    selected &&
      busy === "" &&
      current?.enabled &&
      current?.configured &&
      isStoppedOrFailed
  );

  const canStop = canStopProject({ selected, busy });

  const isReadyOrPortOpenOrFailed =
    status?.state === "ready" ||
    status?.state === "port_open" ||
    status?.state === "failed";
  const canRestart = Boolean(
    selected &&
      busy === "" &&
      current?.enabled &&
      current?.configured &&
      isReadyOrPortOpenOrFailed
  );

  const canInstall = Boolean(
    selected &&
      busy === "" &&
      current?.enabled &&
      status?.install_command &&
      status.state !== "installing" &&
      status.state !== "starting"
  );

  const isPortOpenOrReady = status?.state === "port_open" || status?.state === "ready";
  const canOpenPage = Boolean(
    selected &&
      current?.configured &&
      isPortOpenOrReady &&
      page
  );

  function getOpenPageTitle(): string {
    if (!selected || !current) return "未选择项目";
    if (!current.configured) return "未配置端口或启动命令";
    if (!current.enabled) return current.reason || "项目已禁用";
    if (status?.state === "stopped") return "服务未启动（端口未通）";
    if (status?.state === "installing") return "正在装依赖，装完会自动启动";
    if (status?.state === "starting") return "服务启动中，端口尚未通";
    if (status?.state === "failed") return `服务启动失败${status.failure ? `：${status.failure}` : ""}`;
    if (!page) return "未获取到页面地址";
    return `在浏览器中打开：${page}`;
  }

  const projectList = projects?.projects ?? [];
  const liveStatuses = useMemo(() => {
    const merged: Record<string, Status> = { ...allStatuses };
    if (status) merged[status.id] = status;
    return merged;
  }, [allStatuses, status]);
  const counts = useMemo(
    () => countByFilter(projectList, liveStatuses, query),
    [projectList, liveStatuses, query]
  );
  const visible = useMemo(
    () => filterProjects(projectList, liveStatuses, query, statusFilter),
    [projectList, liveStatuses, query, statusFilter]
  );
  const runningCount = projectList.filter((p) => {
    const st = liveStatuses[p.id]?.state;
    return p.enabled && (st === "ready" || st === "port_open");
  }).length;
  const toolchainOf = (p: { node?: string | null; java?: string | null }) =>
    [p.node ? `Node ${p.node}` : null, p.java ? `Java ${p.java}` : null].filter(
      (x): x is string => Boolean(x)
    );

  const isFiltering = Boolean(query.trim() || statusFilter !== "all");

  const sections = useMemo(
    () =>
      groupProjects({
        allProjects: projectList,
        visibleProjects: visible,
        statuses: liveStatuses,
        groups: projects?.groups,
        isFiltering,
      }),
    [projectList, visible, liveStatuses, projects?.groups, isFiltering]
  );

  const showHeaders = useMemo(() => shouldShowGroupHeaders(sections), [sections]);

  const existingGroupKeys = useMemo(() => {
    const keys = new Set<string>();
    if (projects?.groups) {
      for (const g of projects.groups) {
        if (g.key) keys.add(g.key);
      }
    }
    for (const p of projectList) {
      const k = p.group?.trim();
      if (k) keys.add(k);
    }
    return Array.from(keys);
  }, [projects?.groups, projectList]);

  const handleCreateGroup = useCallback(
    async (e: React.FormEvent) => {
      e.preventDefault();
      const label = newGroupLabel.trim();
      if (!label) return;
      try {
        const key = generateGroupKey(label, existingGroupKeys);
        await invoke("create_group", { key, label });
        setNewGroupLabel("");
        setCreateGroupOpen(false);
        await loadProjects();
      } catch (err: unknown) {
        setError(String(err));
      }
    },
    [newGroupLabel, existingGroupKeys, loadProjects]
  );

  function clearFilters() {
    setQuery("");
    setStatusFilter("all");
  }

  const renderProjectItem = (project: (typeof projectList)[0]) => {
    const toolchain = toolchainOf(project);
    const projStatus = liveStatuses[project.id];
    const isActive = project.id === selected;
    return (
      <li
        key={project.id}
        draggable
        onDragStart={(e) => {
          e.dataTransfer.setData("text/plain", project.id);
          e.dataTransfer.effectAllowed = "move";
        }}
      >
        <button
          type="button"
          className={[
            "project-item",
            isActive ? "active" : "",
            project.enabled ? "" : "disabled",
          ]
            .filter(Boolean)
            .join(" ")}
          aria-current={isActive ? "true" : undefined}
          disabled={busy === "stop"}
          onClick={() => {
            setError("");
            if (project.id === selected) return;
            setStatus(null);
            setLog(null);
            setSelected(project.id);
          }}
        >
          <div className="project-row-top">
            <span className="project-name">
              <Highlight text={project.name} query={query} />
            </span>
            <StatusPill project={project} status={projStatus} compact />
          </div>
          <div className="project-meta">
            <code>
              <Highlight text={project.id} query={query} />
            </code>
            {project.port > 0 && (
              <span className="port">
                :<Highlight text={String(project.port)} query={query} />
              </span>
            )}
            {toolchain.map((t) => (
              <span key={t} className="tool-tag">
                {t}
              </span>
            ))}
          </div>
          {project.resolve_error && (
            <div className="project-reason">{project.resolve_error}</div>
          )}
          {!project.enabled && project.reason && (
            <div className="project-reason">{project.reason}</div>
          )}
        </button>
      </li>
    );
  };

  return (
    <div className="app">
      <aside className="sidebar">
        <div className="brand">
          <img className="brand-logo" src={logoUrl} alt="" width={30} height={34} />
          <div className="brand-text">
            <strong>tongyun-dev</strong>
            <span>
              {projects ? `${projectList.length} 个项目 · ${runningCount} 个运行中` : "本机项目"}
            </span>
          </div>
        </div>

        <div className="sidebar-search">
          <label className="search-field">
            <SearchIcon className="search-icon" />
            <input
              ref={searchRef}
              type="search"
              value={query}
              placeholder="搜索名称、id、端口、目录"
              aria-label="搜索项目"
              onChange={(e) => setQuery(e.target.value)}
              onKeyDown={(e) => {
                if (e.key === "Escape") {
                  e.preventDefault();
                  if (query) setQuery("");
                  else (e.target as HTMLInputElement).blur();
                } else if (e.key === "Enter" && visible.length > 0) {
                  setError("");
                  if (visible[0].id !== selected) {
                    setStatus(null);
                    setLog(null);
                    setSelected(visible[0].id);
                  }
                }
              }}
            />
            {query ? (
              <button
                type="button"
                className="search-clear"
                onClick={() => {
                  setQuery("");
                  searchRef.current?.focus();
                }}
                aria-label="清空搜索"
                title="清空 (Esc)"
              >
                <XIcon />
              </button>
            ) : (
              <kbd className="search-kbd" title="按 / 或 ⌘K 聚焦">/</kbd>
            )}
          </label>
          <div className="filter-chips" role="tablist" aria-label="按状态筛选">
            {STATUS_FILTERS.map((f) => (
              <button
                key={f.key}
                type="button"
                role="tab"
                aria-selected={statusFilter === f.key}
                className={`chip chip-${f.key}${statusFilter === f.key ? " selected" : ""}`}
                onClick={() => setStatusFilter(f.key)}
              >
                {f.label}
                <span className="chip-count">{counts[f.key]}</span>
              </button>
            ))}
          </div>
        </div>

        {projects && !missingConfig && (
          <div className="sidebar-group-toolbar">
            <span className="sidebar-group-toolbar-title">项目列表</span>
            <button
              type="button"
              className="btn btn-ghost btn-xs btn-new-group"
              onClick={() => setCreateGroupOpen((prev) => !prev)}
              title="新建分组"
            >
              <PlusIcon /> 新建分组
            </button>
          </div>
        )}

        {createGroupOpen && (
          <form className="create-group-form" onSubmit={handleCreateGroup}>
            <input
              type="text"
              autoFocus
              className="create-group-input"
              placeholder="输入分组名称（如 前端开发）"
              value={newGroupLabel}
              onChange={(e) => setNewGroupLabel(e.target.value)}
              onKeyDown={(e) => {
                if (e.key === "Escape") {
                  e.preventDefault();
                  setNewGroupLabel("");
                  setCreateGroupOpen(false);
                }
              }}
            />
            <div className="create-group-actions">
              <button
                type="submit"
                className="btn btn-primary btn-xs"
                disabled={!newGroupLabel.trim()}
              >
                确定
              </button>
              <button
                type="button"
                className="btn btn-ghost btn-xs"
                onClick={() => {
                  setNewGroupLabel("");
                  setCreateGroupOpen(false);
                }}
              >
                取消
              </button>
            </div>
          </form>
        )}

        {(() => {
          const hasSectionsToShow = !isFiltering
            ? sections.length > 0 &&
              (projectList.length > 0 || sections.some((s) => !s.isUngrouped))
            : visible.length > 0;

          if (!hasSectionsToShow) return null;

          if (!showHeaders) {
            return (
              <ul className="projects">
                {visible.map((project) => renderProjectItem(project))}
              </ul>
            );
          }

          return (
            <div className="project-groups">
              {sections.map((section) => {
                const collapsed = isGroupCollapsed(section.key, collapsedGroups, isFiltering);
                return (
                  <div
                    key={section.key}
                    className={`group-section${dragOverGroup === section.key ? " drag-over" : ""}`}
                    onDragOver={(e) => {
                      e.preventDefault();
                      e.dataTransfer.dropEffect = "move";
                      if (dragOverGroup !== section.key) {
                        setDragOverGroup(section.key);
                      }
                    }}
                    onDragLeave={(e) => {
                      if (!e.currentTarget.contains(e.relatedTarget as Node)) {
                        setDragOverGroup(null);
                      }
                    }}
                    onDrop={async (e) => {
                      e.preventDefault();
                      setDragOverGroup(null);
                      const draggedId = e.dataTransfer.getData("text/plain");
                      if (!draggedId) return;
                      try {
                        const targetGroup = section.isUngrouped ? null : section.key;
                        await invoke("set_project_group", { id: draggedId, group: targetGroup });
                        await loadProjects();
                        if (selected === draggedId) {
                          await refresh(draggedId);
                        }
                      } catch (err: unknown) {
                        setError(String(err));
                      }
                    }}
                  >
                    <div className="group-header">
                      <button
                        type="button"
                        className="group-header-btn"
                        onClick={() => toggleGroupCollapse(section.key)}
                        aria-expanded={!collapsed}
                      >
                        <span className="group-chevron" aria-hidden="true">
                          {collapsed ? <ChevronRightIcon /> : <ChevronDownIcon />}
                        </span>
                        <span className="group-name" title={section.label}>
                          {section.label}
                        </span>
                        <span className="group-stats">
                          运行中 {section.runningCount} / 共 {section.totalCount}
                        </span>
                      </button>
                    </div>
                    {!collapsed &&
                      (section.visibleProjects.length === 0 ? (
                        <div className="group-empty-drop-zone">拖动项目至此归类</div>
                      ) : (
                        <ul className="projects">
                          {section.visibleProjects.map((project) => renderProjectItem(project))}
                        </ul>
                      ))}
                  </div>
                );
              })}
            </div>
          );
        })()}

        {!projects && !error && !missingConfig && (
          <div className="sidebar-empty">
            <span className="spinner" aria-hidden="true" />
            正在读取配置…
          </div>
        )}
        {missingConfig && <div className="sidebar-empty">还没有项目配置</div>}
        {projects && projectList.length === 0 && (
          <div className="sidebar-empty">
            <p>还没有项目</p>
            <button type="button" className="btn btn-primary btn-sm" onClick={handleOpenScan}>
              <SearchIcon /> 扫描目录…
            </button>
            <button type="button" className="btn btn-ghost btn-sm" onClick={handleOpenCreate}>
              <PlusIcon /> 手动新增
            </button>
          </div>
        )}
        {projects && projectList.length > 0 && visible.length === 0 && (
          <div className="sidebar-empty">
            <SearchIcon className="empty-icon" />
            <p>没有匹配的项目</p>
            <button type="button" className="btn btn-ghost btn-sm" onClick={clearFilters}>
              清除筛选
            </button>
          </div>
        )}

        <div className="sidebar-footer">
          <button type="button" className="btn btn-secondary btn-block" disabled={missingConfig} onClick={handleOpenCreate}>
            <PlusIcon /> 新增项目
          </button>
          <button
            type="button"
            className="btn icon-btn"
            onClick={handleOpenScan}
            title="扫描目录：自动找出项目"
            aria-label="扫描目录"
          >
            <SearchIcon />
          </button>
          <div className="menu-anchor">
            <button
              type="button"
              className={`btn icon-btn${menu === "settings" ? " pressed" : ""}`}
              onClick={() => setMenu(menu === "settings" ? "" : "settings")}
              title="设置"
              aria-label="设置"
              aria-expanded={menu === "settings"}
            >
              <GearIcon />
            </button>
            {menu === "settings" && (
              <div className="menu menu-up" role="menu">
                <div className="menu-section">
                  <span className="menu-label">配置文件</span>
                  <code className="menu-path" title={configInfo?.path ?? projects?.config_path ?? ""}>
                    {projects?.config_path ?? configInfo?.path ?? configInfo?.default_path ?? "—"}
                  </code>
                </div>
                <button
                  type="button"
                  role="menuitem"
                  onClick={() => {
                    setMenu("");
                    loadProjects().catch((err) => setError(String(err)));
                  }}
                >
                  <RefreshIcon /> 重新加载配置
                </button>
                <button
                  type="button"
                  role="menuitem"
                  onClick={() => {
                    setMenu("");
                    handlePickConfig();
                  }}
                >
                  <FolderIcon /> 换一个配置文件…
                </button>
                <div className="menu-sep" />
                <button
                  type="button"
                  role="menuitem"
                  className="menu-advanced"
                  disabled={!projects}
                  onClick={() => {
                    setMenu("");
                    handleOpenConfigFile();
                  }}
                  title="高级：直接用文本编辑器改 projects.toml"
                >
                  <FileIcon /> 用编辑器打开 projects.toml（高级）
                </button>
              </div>
            )}
          </div>
        </div>
      </aside>

      <main className="main">
        {missingConfig ? (
          <div className="empty-config">
            <img src={logoUrl} alt="" width={56} height={64} />
            <h2>还没有项目</h2>
            <p className="empty-desc">
              选一个放代码的目录，自动认出里面的项目；配置会写到：
            </p>
            <p className="config-default-path">
              <code>{configInfo?.default_path ?? "~/.config/tongyun-dev/projects.toml"}</code>
            </p>
            <div className="actions">
              <button type="button" className="btn btn-primary" onClick={handleOpenScan}>
                <SearchIcon /> 扫描一个目录
              </button>
              <button type="button" className="btn btn-secondary" onClick={handleCreateDefault}>
                新建空配置
              </button>
              <button type="button" className="btn btn-secondary" onClick={handlePickConfig}>
                选择配置文件…
              </button>
            </div>
            {error && <p className="banner danger">{error}</p>}
          </div>
        ) : !current ? (
          <div className="empty-config">
            <img src={logoUrl} alt="" width={56} height={64} />
            {projects && projects.projects.length === 0 ? (
              <>
                <h2>还没有项目</h2>
                <p className="empty-desc">
                  扫描一个目录（比如 ~/code），自动认出 Spring Boot、Vite、Vue、Next 等项目；也可以手动新增。
                </p>
                <div className="actions empty-actions">
                  <button type="button" className="btn btn-primary" onClick={handleOpenScan}>
                    <SearchIcon /> 扫描一个目录
                  </button>
                  <button type="button" className="btn btn-secondary" onClick={handleOpenCreate}>
                    <PlusIcon /> 手动新增
                  </button>
                </div>
              </>
            ) : (
              <>
                <h2>{!projects && error ? "没能读出项目" : projects ? "选一个项目" : "正在读取…"}</h2>
                {projects && <p className="empty-desc">在左边选一个项目，或者新增一个。</p>}
                {!projects && error && (
                  <p className="empty-desc">配置没读出来，项目列表是空的。改好后从设置里重新加载。</p>
                )}
              </>
            )}
            {error && <p className="banner danger">{error}</p>}
          </div>
        ) : (
          <>
            <header className="detail-header">
              <div className="title-area">
                <div className="title-row">
                  <h1>{current.name}</h1>
                  <StatusPill project={current} status={status} />
                  {status?.active_profile && (
                    <span className="profile-active-tag" title={`当前运行环境：${status.active_profile}`}>
                      ⚡ {current.profiles?.[status.active_profile]?.label || status.active_profile}
                    </span>
                  )}
                </div>
                <div className="subtitle">
                  <code className="id-tag">{current.id}</code>
                  {toolchainOf(current).map((t) => (
                    <span key={t} className="tool-tag">
                      {t}
                    </span>
                  ))}
                  <span className="path" title={status?.root ?? current.root}>
                    <FolderIcon />
                    {status?.root ?? current.root}
                  </span>
                </div>
              </div>
              <div className="header-actions" aria-busy={busy !== ""}>
                {current.profiles && Object.keys(current.profiles).length > 0 && (
                  <div className="profile-selector-container">
                    <label htmlFor="header-profile-select" className="profile-selector-label">
                      环境:
                    </label>
                    <select
                      id="header-profile-select"
                      className="profile-selector-select"
                      value={selectedProfile}
                      disabled={busy !== ""}
                      onChange={(e) => setSelectedProfile(e.target.value)}
                      title="选择启动环境预设"
                    >
                      <option value="">默认配置</option>
                      {Object.entries(current.profiles).map(([key, prof]) => (
                        <option key={key} value={key}>
                          {prof.label ? `${prof.label} (${key})` : key}
                        </option>
                      ))}
                    </select>
                  </div>
                )}
                <div className="btn-group">
                  <button type="button" className="btn btn-secondary" disabled={!canStart} onClick={() => handleStart()}>
                    <PlayIcon className="icon-play" />
                    {busy === "start" ? "启动中…" : "启动"}
                  </button>
                  <button
                    type="button"
                    className="btn btn-secondary"
                    disabled={!canStop}
                    title={stopButtonTitle(status)}
                    onClick={handleStop}
                  >
                    <StopIcon className="icon-stop" />
                    {busy === "stop" ? "停止中…" : "停止"}
                  </button>
                  <button type="button" className="btn btn-secondary" disabled={!canRestart} onClick={() => handleRestart()}>
                    <RestartIcon />
                    {busy === "restart" ? "重启中…" : "重启"}
                  </button>
                </div>
                {current.links && current.links.length > 1 ? (
                  <div className="pages-split-btn">
                    <button
                      type="button"
                      className="btn btn-primary pages-primary-btn"
                      disabled={!canOpenPage}
                      title={current.links[0].url}
                      onClick={() => invoke("open_page", { url: current.links![0].url })}
                    >
                      <ExternalIcon />
                      {current.links[0].title || "打开网页"}
                    </button>
                    <button
                      type="button"
                      className={`btn btn-primary pages-dropdown-toggle${pagesDropdownOpen ? " active" : ""}`}
                      disabled={!canOpenPage}
                      onClick={() => setPagesDropdownOpen((prev) => !prev)}
                      title="展开全部网页链接"
                      aria-label="展开全部网页链接"
                    >
                      <ChevronDownIcon />
                    </button>
                    {pagesDropdownOpen && (
                      <div className="menu menu-down pages-menu" role="menu">
                        {current.links.map((link, idx) => (
                          <button
                            key={idx}
                            type="button"
                            role="menuitem"
                            onClick={() => {
                              setPagesDropdownOpen(false);
                              invoke("open_page", { url: link.url }).catch((e) => setError(String(e)));
                            }}
                            title={link.url}
                          >
                            <ExternalIcon /> {link.title || link.url}
                          </button>
                        ))}
                        <div className="menu-sep" />
                        <button
                          type="button"
                          role="menuitem"
                          className="pages-open-all-btn"
                          onClick={() => {
                            setPagesDropdownOpen(false);
                            handleOpenAllPages();
                          }}
                        >
                          全部打开 ({current.links.length})
                        </button>
                      </div>
                    )}
                  </div>
                ) : (
                  <button
                    type="button"
                    className="btn btn-primary"
                    disabled={!canOpenPage}
                    title={getOpenPageTitle()}
                    onClick={() => reveal("page")}
                  >
                    <ExternalIcon />
                    {current.links?.[0]?.title || "打开网页"}
                  </button>
                )}
                <button
                  type="button"
                  className="btn btn-secondary btn-terminal-header"
                  onClick={() => handleOpenTerminal()}
                  title="在系统独立终端中运行项目（支持上下键选择与按键交互）"
                >
                  <TerminalIcon />
                  终端
                </button>
                <button
                  type="button"
                  className="btn btn-secondary"
                  onClick={handleOpenEdit}
                  disabled={busy !== ""}
                  title="编辑这个项目的目录、命令、端口、日志、Java/Node 版本"
                >
                  <GearIcon />
                  项目设置
                </button>
                <div className="menu-anchor">
                  <button
                    type="button"
                    className={`btn icon-btn${menu === "more" ? " pressed" : ""}`}
                    onClick={() => setMenu(menu === "more" ? "" : "more")}
                    title="更多"
                    aria-label="更多"
                    aria-expanded={menu === "more"}
                  >
                    <MoreIcon />
                  </button>
                  {menu === "more" && (
                    <div className="menu menu-down" role="menu">
                      <button
                        type="button"
                        role="menuitem"
                        disabled={busy !== ""}
                        onClick={() => {
                          setMenu("");
                          setError("");
                          refresh(selected).catch((err: unknown) => setError(String(err)));
                        }}
                      >
                        <RefreshIcon /> 刷新状态
                      </button>
                      <button
                        type="button"
                        role="menuitem"
                        onClick={() => {
                          setMenu("");
                          handleOpenTerminal();
                        }}
                        title="在系统独立终端中交互运行"
                      >
                        <TerminalIcon /> 在系统终端中运行
                      </button>
                      <button
                        type="button"
                        role="menuitem"
                        disabled={!canOpenLog}
                        onClick={() => {
                          setMenu("");
                          reveal("log");
                        }}
                      >
                        <FileIcon /> 打开日志文件
                      </button>
                      {status?.install_command && (
                        <button
                          type="button"
                          role="menuitem"
                          disabled={!canInstall}
                          title={`在项目目录运行 ${status.install_command}`}
                          onClick={() => {
                            setMenu("");
                            handleInstall();
                          }}
                        >
                          <RefreshIcon /> 安装依赖（{status.install_command}）
                        </button>
                      )}
                      <div className="menu-sep" />
                      <button
                        type="button"
                        role="menuitem"
                        className="danger"
                        disabled={busy !== ""}
                        onClick={() => {
                          setMenu("");
                          handleDeleteProject();
                        }}
                      >
                        <TrashIcon /> 删除项目…
                      </button>
                    </div>
                  )}
                </div>
              </div>
            </header>

            <div className="detail-body">
              <DetailStateBar
                status={status}
                project={current}
                canInstall={canInstall}
                installing={busy === "install"}
                onInstall={handleInstall}
                onScrollToLog={() => {
                  document.getElementById("log-panel")?.scrollIntoView({ behavior: "smooth" });
                }}
              />

              {current.resolve_error && (
                <div className="callout warn">
                  <AlertIcon className="callout-icon" />
                  <div className="callout-text">
                    <strong>这个项目的配置解析失败</strong>
                    <p>{current.resolve_error}</p>
                  </div>
                  <button type="button" className="btn btn-primary" onClick={handleOpenEdit}>
                    <GearIcon /> 去配置
                  </button>
                </div>
              )}

              {!current.configured && !current.resolve_error && (
                <div className="callout warn">
                  <AlertIcon className="callout-icon" />
                  <div className="callout-text">
                    <strong>这个项目还没配置好</strong>
                    <p>缺端口或启动命令，所以先不能启动。填好就能用。</p>
                  </div>
                  <button type="button" className="btn btn-primary" onClick={handleOpenEdit}>
                    <GearIcon /> 去配置
                  </button>
                </div>
              )}

              {!current.enabled && (
                <div className="callout muted">
                  <AlertIcon className="callout-icon" />
                  <div className="callout-text">
                    <strong>已停用</strong>
                    <p>{current.reason || "这个项目在配置里被停用了。"}</p>
                  </div>
                  <button type="button" className="btn btn-secondary" onClick={handleOpenEdit}>
                    <GearIcon /> 项目设置
                  </button>
                </div>
              )}
              {status?.port_conflict && (
                <div className="callout warn">
                  <AlertIcon className="callout-icon" />
                  <div className="callout-text">
                    <strong>端口 {status.port} 被别的进程占着</strong>
                    <p>{status.port_holder || "不是本项目启动的进程。"}</p>
                  </div>
                </div>
              )}
              {((status?.hints && status.hints.length > 0) ? status.hints : (current.hints ?? [])).map((hint, idx) => (
                <div key={idx} className="callout warn">
                  <AlertIcon className="callout-icon" />
                  <div className="callout-text">
                    <strong>启动提示</strong>
                    <p>{hint}</p>
                  </div>
                </div>
              ))}
              {error && (
                <div className="callout danger">
                  <AlertIcon className="callout-icon" />
                  <div className="callout-text">
                    <p>{error}</p>
                  </div>
                  <button type="button" className="btn btn-ghost btn-sm" onClick={() => setError("")}>
                    知道了
                  </button>
                </div>
              )}

              <section className="stats" aria-label="运行信息">
                <Stat
                  icon={<PulseIcon />}
                  label="运行状态"
                  value={runLabel(status, current)}
                  tone={runTone(status, current)}
                />
                <Stat
                  icon={<CpuIcon />}
                  label="进程"
                  value={status?.pid ? `PID ${status.pid}` : status ? "未运行" : "—"}
                  tone={status?.pid ? "ok" : "muted"}
                />
                <Stat
                  icon={<PlugIcon />}
                  label="端口"
                  value={
                    !current.port
                      ? "未配置"
                      : status
                        ? `${status.port} · ${status.port_open ? "已通" : "未通"}`
                        : String(current.port)
                  }
                  tone={!current.port ? "warn" : status?.port_open ? "ok" : "muted"}
                />
                <Stat
                  icon={<DoorIcon />}
                  label="登录页"
                  value={
                    !current.configured || !current.port
                      ? "未配置"
                      : !status
                      ? "—"
                      : status.login_probe.ok
                        ? `正常 · ${status.login_probe.http_code ?? ""}`.trim()
                        : status.login_probe.http_code
                          ? `异常 · ${status.login_probe.http_code}`
                          : "未响应"
                  }
                  detail={current.configured && current.port ? status?.login_probe.url : undefined}
                  tone={status?.login_probe.ok ? "ok" : status?.login_probe.http_code ? "danger" : "muted"}
                />
              </section>

              <LogPanel
                log={log}
                logPath={logPath}
                canOpenLog={canOpenLog}
                onOpenLog={() => reveal("log")}
                selected={selected}
                running={status?.running ?? false}
                onOpenTerminal={() => handleOpenTerminal()}
              />
            </div>
          </>
        )}
      </main>
      {scanOpen && (
        <ScanDialog
          onClose={() => setScanOpen(false)}
          onImported={async (firstId) => {
            await loadProjects();
            if (firstId) setSelected(firstId);
          }}
        />
      )}
      {formModal.open && (
        <ProjectForm
          mode={formModal.mode}
          initialDetail={formModal.detail}
          groups={projects?.groups}
          existingGroups={existingGroupKeys}
          onClose={() => setFormModal((prev) => ({ ...prev, open: false }))}
          onSuccess={async (savedId) => {
            await loadProjects();
            setSelected(savedId);
          }}
        />
      )}
    </div>
  );
}

type Tone = "ok" | "warn" | "danger" | "info" | "muted";

function runLabel(status: Status | null, project: { enabled: boolean; configured: boolean }): string {
  if (!project.configured) return "未配置";
  if (!project.enabled) return "已停用";
  if (!status) return "探测中…";
  switch (status.state) {
    case "ready":
      return "就绪";
    case "port_open":
      return "端口已通";
    case "installing":
      return `安装依赖中 · ${status.elapsed_secs ?? 0} 秒`;
    case "starting":
      return `启动中 · ${status.elapsed_secs ?? 0} 秒`;
    case "failed":
      return status.deps_missing ? "依赖未安装" : "启动失败";
    default:
      return "已停止";
  }
}

function runTone(status: Status | null, project: { enabled: boolean; configured: boolean }): Tone {
  if (!project.configured) return "warn";
  if (!project.enabled || !status) return "muted";
  switch (status.state) {
    case "ready":
    case "port_open":
      return "ok";
    case "installing":
    case "starting":
      return "info";
    case "failed":
      return "danger";
    default:
      return "muted";
  }
}

function StatusPill({
  project,
  status,
  compact,
}: {
  project: { enabled: boolean; configured: boolean };
  status: Status | null | undefined;
  compact?: boolean;
}) {
  let cls: string = status?.state || "stopped";
  let label = compact ? stateShortLabel(status ?? { state: "stopped" }) : runLabel(status ?? null, project);
  if (!project.configured) {
    cls = "unconfigured";
    label = "未配置";
  } else if (!project.enabled) {
    cls = "disabled";
    label = "已停用";
  } else if (!status && !compact) {
    label = "探测中…";
  }
  return (
    <span className={`pill pill-${cls}`}>
      <span className="pill-dot" />
      {label}
    </span>
  );
}

function Highlight({ text, query }: { text: string; query: string }) {
  const tokens = query.trim().toLowerCase().split(/\s+/).filter(Boolean);
  if (tokens.length === 0) return <>{text}</>;
  const lower = text.toLowerCase();
  const marks: boolean[] = new Array(text.length).fill(false);
  for (const t of tokens) {
    let from = 0;
    for (;;) {
      const idx = lower.indexOf(t, from);
      if (idx === -1) break;
      for (let k = idx; k < idx + t.length; k++) marks[k] = true;
      from = idx + Math.max(1, t.length);
    }
  }
  const parts: React.ReactNode[] = [];
  let i = 0;
  while (i < text.length) {
    const on = marks[i];
    let j = i;
    while (j < text.length && marks[j] === on) j++;
    const seg = text.slice(i, j);
    parts.push(on ? <mark key={i}>{seg}</mark> : seg);
    i = j;
  }
  return <>{parts}</>;
}

function DetailStateBar({
  status,
  project,
  canInstall,
  installing,
  onInstall,
  onScrollToLog,
}: {
  status: Status | null;
  project: { enabled: boolean; configured: boolean };
  canInstall: boolean;
  installing: boolean;
  onInstall: () => void;
  onScrollToLog: () => void;
}) {
  if (!project.configured || !project.enabled) return null;
  if (!status) {
    return (
      <div className="state-bar probing">
        <span className="state-dot" />
        <span className="state-text">正在探测状态…</span>
      </div>
    );
  }
  const state = status.state;
  const installBtn = (
    <button type="button" className="btn btn-primary btn-sm" disabled={!canInstall} onClick={onInstall}>
      {installing ? "安装中…" : "安装依赖"}
    </button>
  );
  if (state === "installing") {
    const elapsed = status.elapsed_secs ?? 0;
    return (
      <div className="state-bar starting installing">
        <span className="state-dot" />
        <div className="state-main">
          <div className="state-line">
            <span className="state-text">安装依赖中 · 已 {elapsed} 秒</span>
            <span className="state-tip">
              缺 node_modules，正在运行 {status.install_command || "install"}，装完会自动启动；输出在下面日志里
            </span>
          </div>
          <div className="progress indeterminate">
            <div className="progress-bar" />
          </div>
        </div>
        <button type="button" className="btn btn-ghost btn-sm" onClick={onScrollToLog}>
          看日志
        </button>
      </div>
    );
  }
  if (state === "starting") {
    const elapsed = status.elapsed_secs ?? 0;
    const timeout = status.timeout_secs || 180;
    const pct = Math.min(100, Math.round((elapsed / timeout) * 100));
    return (
      <div className="state-bar starting">
        <span className="state-dot" />
        <div className="state-main">
          <div className="state-line">
            <span className="state-text">启动中 · 已 {elapsed} 秒</span>
            <span className="state-tip">
              {status.port_open
                ? "端口已通，正在等待服务就绪…"
                : `端口 ${status.port} 还没通，日志在写说明在启动`}
            </span>
          </div>
          <div className="progress">
            <div className="progress-bar" style={{ width: `${Math.max(4, pct)}%` }} />
          </div>
        </div>
        <span className="state-timeout">最长 {timeout} 秒</span>
      </div>
    );
  }
  if (state === "port_open") {
    return (
      <div className="state-bar port_open">
        <span className="state-dot" />
        <span className="state-text">端口已通 · 等登录页</span>
      </div>
    );
  }
  if (state === "ready") {
    const elapsed = status.elapsed_secs;
    return (
      <div className="state-bar ready">
        <span className="state-dot" />
        <span className="state-text">{elapsed != null ? `就绪 · 用时 ${elapsed} 秒` : "就绪"}</span>
        <span className="state-tip">服务已能访问，可以点「打开网页」</span>
      </div>
    );
  }
  if (state === "failed") {
    return (
      <div className="state-bar failed">
        <span className="state-dot" />
        <span className="state-text">失败 · {status.failure || "服务启动失败"}</span>
        {status.pid != null && <span className="state-tip">进程 PID {status.pid} 仍在，停止可清理，重启可重新尝试</span>}
        {status.install_command && (status.deps_missing || (status.failure ?? "").includes("依赖")) && installBtn}
        <button type="button" className="btn btn-ghost btn-sm" onClick={onScrollToLog}>
          看日志
        </button>
      </div>
    );
  }
  return (
    <div className="state-bar stopped">
      <span className="state-dot" />
      <span className="state-text">已停止</span>
      <span className="state-tip">
        {status.deps_missing
          ? `还没装依赖（缺 node_modules），点「启动」会先自动运行 ${status.install_command || "install"}`
          : "点「启动」运行这个项目"}
      </span>
      {status.deps_missing && status.install_command && installBtn}
    </div>
  );
}

function pageUrl(status: Status | null, project: { port: number } | null): string {
  const probe = status?.login_probe.url.trim() ?? "";
  if (probe.startsWith("http://") || probe.startsWith("https://")) return probe;
  const port = status?.port ?? project?.port ?? 0;
  if (port > 0) return `http://127.0.0.1:${port}/`;
  return "";
}

function Stat({
  icon,
  label,
  value,
  detail,
  tone,
}: {
  icon: React.ReactNode;
  label: string;
  value: string;
  detail?: string;
  tone: Tone;
}) {
  return (
    <article className={`stat tone-${tone}`}>
      <div className="stat-head">
        <span className="stat-icon">{icon}</span>
        <span className="stat-label">{label}</span>
      </div>
      <p className="stat-value">{value}</p>
      {detail && (
        <small className="stat-detail" title={detail}>
          {detail}
        </small>
      )}
    </article>
  );
}
