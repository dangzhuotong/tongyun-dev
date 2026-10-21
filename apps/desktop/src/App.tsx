import { invoke as tauriInvoke, type InvokeArgs } from "@tauri-apps/api/core";
import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { isDetailPreview, previewInvoke } from "./previewData";
import type { BatchOutput, ConfigInfo, LogTail, ProjectDetail, ProjectList, Status } from "./types";
import { Toolbar } from "./Toolbar";
import { StatusBar } from "./StatusBar";
import { AboutModal } from "./AboutModal";
import { UpdateModal } from "./UpdateModal";
import { checkForAppUpdate, resolveDisplayedAppVersion } from "./updater";
import type { Update } from "@tauri-apps/plugin-updater";
import { version as bundledVersion } from "../package.json";
import { ShortcutsHelpModal } from "./ShortcutsHelpModal";
import { ProjectDetailsTab } from "./ProjectDetailsTab";
import { ProjectOverview } from "./ProjectOverview";
import { LogPanel } from "./LogPanel";
import { stateShortLabel } from "./logFormat";
import { runLabel } from "./overviewStatus";
import { ProjectForm } from "./ProjectForm";
import { ScanDialog } from "./ScanDialog";
import { countByFilter, filterProjects, STATUS_FILTERS, type StatusFilter } from "./projectFilter";
import { canStopProject, stopButtonTitle } from "./buttonState";
import { effectiveStartCommand } from "./startCommand";
import logoUrl from "./assets/logo.svg";
import {
  ChevronDownIcon,
  ChevronRightIcon,
  FileIcon,
  FolderIcon,
  GearIcon,
  PlusIcon,
  RefreshIcon,
  SearchIcon,
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

function invoke<T>(cmd: string, args?: InvokeArgs): Promise<T> {
  if (import.meta.env.DEV && isDetailPreview()) {
    const record = args && !ArrayBuffer.isView(args) && !(args instanceof ArrayBuffer) && !Array.isArray(args)
      ? (args as Record<string, unknown>)
      : undefined;
    return previewInvoke<T>(cmd, record);
  }
  return tauriInvoke<T>(cmd, args);
}

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
  const [copiedCmd, setCopiedCmd] = useState(false);
  const [pagesDropdownOpen, setPagesDropdownOpen] = useState(false);
  const [activeTab, setActiveTab] = useState<"overview" | "logs" | "details">(() => {
    try {
      const urlTab = new URLSearchParams(window.location.search).get("tab");
      if (urlTab === "overview" || urlTab === "logs" || urlTab === "details") return urlTab;
    } catch {
      // ignore
    }
    return "overview";
  });
  const [aboutOpen, setAboutOpen] = useState(false);
  const [shortcutsOpen, setShortcutsOpen] = useState(false);
  const [isRefreshing, setIsRefreshing] = useState(false);
  const [runtimeVersion, setRuntimeVersion] = useState<string | null>(null);
  const [checkingUpdate, setCheckingUpdate] = useState(false);
  const [activeUpdate, setActiveUpdate] = useState<Update | null>(null);
  const [updateToast, setUpdateToast] = useState<{ text: string; tone: "ok" | "danger" } | null>(
    null,
  );
  const checkingUpdateRef = useRef(false);
  const displayVersion = resolveDisplayedAppVersion(runtimeVersion, bundledVersion);

  // Theme
  const [theme, setTheme] = useState<"system" | "light" | "dark">(() => {
    try {
      const urlTheme = new URLSearchParams(window.location.search).get("theme");
      if (urlTheme === "system" || urlTheme === "light" || urlTheme === "dark") return urlTheme;
      return (localStorage.getItem("tongyun_theme") as "system" | "light" | "dark") || "system";
    } catch {
      return "system";
    }
  });

  const handleCheckUpdate = useCallback(async (manual: boolean) => {
    if (checkingUpdateRef.current) return;
    checkingUpdateRef.current = true;
    setCheckingUpdate(true);
    try {
      const update = await checkForAppUpdate();
      if (update) {
        setActiveUpdate((prev) => {
          prev?.close().catch(() => {});
          return update;
        });
      } else if (manual) {
        setUpdateToast({ text: "已是最新版本", tone: "ok" });
      }
    } catch (err) {
      const message = err instanceof Error ? err.message : String(err);
      if (manual) {
        setUpdateToast({ text: `检查更新失败：${message}`, tone: "danger" });
      } else {
        console.warn("[Updater] Silent check skipped/failed:", err);
      }
    } finally {
      checkingUpdateRef.current = false;
      setCheckingUpdate(false);
    }
  }, []);

  useEffect(() => {
    if (import.meta.env.DEV && isDetailPreview()) return;
    let cancelled = false;
    import("@tauri-apps/api/app")
      .then(({ getVersion }) => getVersion())
      .then((version) => {
        if (!cancelled) setRuntimeVersion(version);
      })
      .catch(() => {});
    return () => {
      cancelled = true;
    };
  }, []);

  useEffect(() => {
    if (import.meta.env.DEV && isDetailPreview()) return;
    const timer = window.setTimeout(() => {
      void handleCheckUpdate(false);
    }, 3000);
    return () => window.clearTimeout(timer);
  }, [handleCheckUpdate]);

  useEffect(() => {
    if (!updateToast) return;
    const timer = window.setTimeout(() => setUpdateToast(null), 3600);
    return () => window.clearTimeout(timer);
  }, [updateToast]);

  useEffect(() => {
    const root = document.documentElement;
    if (theme === "dark") {
      root.setAttribute("data-theme", "dark");
      root.classList.add("dark");
    } else if (theme === "light") {
      root.setAttribute("data-theme", "light");
      root.classList.remove("dark");
    } else {
      root.removeAttribute("data-theme");
      root.classList.remove("dark");
    }
    localStorage.setItem("tongyun_theme", theme);
  }, [theme]);

  // Sidebar drag resizer
  const [sidebarWidth, setSidebarWidth] = useState<number>(() => {
    try {
      const saved = localStorage.getItem("tongyun_sidebar_width");
      return saved ? Math.max(200, Math.min(550, parseInt(saved, 10))) : 280;
    } catch {
      return 280;
    }
  });
  const isResizingRef = useRef(false);

  const handleMouseDownResize = (e: React.MouseEvent) => {
    e.preventDefault();
    isResizingRef.current = true;
    const handleMouseMove = (moveEvent: MouseEvent) => {
      if (!isResizingRef.current) return;
      const newWidth = Math.max(200, Math.min(550, moveEvent.clientX));
      setSidebarWidth(newWidth);
    };
    const handleMouseUp = (upEvent: MouseEvent) => {
      isResizingRef.current = false;
      const finalWidth = Math.max(200, Math.min(550, upEvent.clientX));
      try {
        localStorage.setItem("tongyun_sidebar_width", finalWidth.toString());
      } catch {
        // ignore
      }
      window.removeEventListener("mousemove", handleMouseMove);
      window.removeEventListener("mouseup", handleMouseUp);
    };
    window.addEventListener("mousemove", handleMouseMove);
    window.addEventListener("mouseup", handleMouseUp);
  };

  const toggleGroupCollapse = useCallback((key: string) => {
    setCollapsedGroups((prev) => {
      const next = { ...prev, [key]: !prev[key] };
      saveCollapsedGroups(next);
      return next;
    });
  }, []);

  // 「/」或 Cmd/Ctrl+K 聚焦项目搜索；在其他输入框里不拦截「/」；F1 或 ? 唤出快捷键
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
      } else if ((e.key === "F1" || (e.key === "?" && !typing)) && !document.querySelector(".modal-backdrop")) {
        e.preventDefault();
        setShortcutsOpen((prev) => !prev);
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

  const refreshAllStatuses = useCallback(async () => {
    try {
      const list = await invoke<Status[]>("all_status");
      setAllStatuses((prev) => {
        const next = { ...prev };
        for (const s of list) {
          next[s.id] = s;
        }
        return next;
      });
    } catch {
      // periodic status poll errors are ignored
    }
  }, []);

  const handleRefreshAll = useCallback(async () => {
    setIsRefreshing(true);
    try {
      await refreshAllStatuses();
      if (selected) {
        await refresh(selected).catch(() => undefined);
      }
    } finally {
      setIsRefreshing(false);
    }
  }, [refreshAllStatuses, selected, refresh]);

  // Periodic poll of all project statuses every 3 seconds
  useEffect(() => {
    let cancelled = false;
    const poll = async () => {
      if (!cancelled) {
        await refreshAllStatuses();
      }
    };
    poll();
    const timer = window.setInterval(poll, 3000);
    return () => {
      cancelled = true;
      window.clearInterval(timer);
    };
  }, [refreshAllStatuses]);

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

  async function handleStartBatch(groupKey?: string) {
    if (busyRef.current) return;
    setBusy("start");
    setError("");
    try {
      const res = await invoke<BatchOutput>("start_batch", {
        group: groupKey || undefined,
        ids: undefined,
      });
      if (res.failed > 0) {
        const rows = res.results || res.items || [];
        const failedMsgs = rows
          .filter((i) => !i.success && !i.ok)
          .map((i) => `${i.id}: ${i.error || i.message || "失败"}`)
          .join("; ");
        setError(`批量启动部分失败 (${res.succeeded}/${res.total}): ${failedMsgs}`);
      }
    } catch (err: unknown) {
      setError(String(err));
    } finally {
      await refreshAllStatuses();
      if (selected) {
        await refresh(selected).catch(() => undefined);
      }
      setBusy("");
    }
  }

  async function handleStopBatch(groupKey?: string) {
    if (busyRef.current) return;
    setBusy("stop");
    setError("");
    try {
      const res = await invoke<BatchOutput>("stop_batch", {
        group: groupKey || undefined,
        ids: undefined,
      });
      if (res.failed > 0) {
        const rows = res.results || res.items || [];
        const failedMsgs = rows
          .filter((i) => !i.success && !i.ok)
          .map((i) => `${i.id}: ${i.error || i.message || "失败"}`)
          .join("; ");
        setError(`批量停止部分失败 (${res.succeeded}/${res.total}): ${failedMsgs}`);
      }
    } catch (err: unknown) {
      setError(String(err));
    } finally {
      await refreshAllStatuses();
      if (selected) {
        await refresh(selected).catch(() => undefined);
      }
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

  async function reveal(kind: "log" | "page" | "folder") {
    setError("");
    try {
      if (kind === "log") {
        const path = log?.path || status?.log_path || "";
        await invoke("open_log", { path });
      } else if (kind === "folder") {
        const folder = status?.root || current?.root || "";
        if (folder) await invoke("open_folder", { path: folder });
      } else {
        await invoke("open_page", { url: pageUrl(status, current) });
      }
    } catch (err: unknown) {
      setError(String(err));
    }
  }

  async function openInCursor() {
    const folder = status?.root || current?.root || "";
    if (!folder) return;
    setError("");
    try {
      await invoke("open_in_cursor", { path: folder });
    } catch (err: unknown) {
      setError(String(err));
    }
  }

  async function copyStartCommand() {
    const cmd = effectiveStartCommand(current, selectedProfile);
    if (!cmd) {
      setError("没有可复制的启动命令");
      return;
    }
    try {
      await navigator.clipboard.writeText(cmd);
      setCopiedCmd(true);
      window.setTimeout(() => setCopiedCmd(false), 1500);
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
      <Toolbar
        projects={projects}
        selected={selected}
        onSelectProject={(id) => {
          setError("");
          if (id === selected) return;
          setStatus(null);
          setLog(null);
          setSelected(id);
        }}
        currentProject={current ?? undefined}
        status={status}
        busy={busy}
        canStart={canStart}
        canStop={canStop}
        canRestart={canRestart}
        canOpenPage={canOpenPage}
        onStart={() => handleStart()}
        onStop={handleStop}
        onRestart={() => handleRestart()}
        onOpenPage={() => reveal("page")}
        onOpenAllPages={handleOpenAllPages}
        onOpenTerminal={() => handleOpenTerminal()}
        onOpenCreate={handleOpenCreate}
        onOpenScan={handleOpenScan}
        onOpenEdit={handleOpenEdit}
        onRefresh={handleRefreshAll}
        isRefreshing={isRefreshing}
        theme={theme}
        onToggleTheme={() => setTheme(theme === "dark" ? "light" : "dark")}
        selectedProfile={selectedProfile}
        onProfileChange={setSelectedProfile}
        onStartBatch={() => handleStartBatch()}
        onStopBatch={() => handleStopBatch()}
        configInfo={configInfo}
        onPickConfig={handlePickConfig}
        onOpenConfigFile={handleOpenConfigFile}
        onReloadConfig={() => loadProjects().catch((err) => setError(String(err)))}
        appVersion={displayVersion}
        checkingUpdate={checkingUpdate}
        onCheckUpdate={() => void handleCheckUpdate(true)}
      />

      <div className="workspace-shell">
        <aside className="sidebar" style={{ width: `${sidebarWidth}px` }}>
          <div className="sidebar-search pt-2.5">
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
            <div className="sidebar-group-toolbar-actions">
              <button
                type="button"
                className="btn btn-ghost btn-xs"
                onClick={() => handleStartBatch()}
                disabled={busy !== ""}
                title="启动全部项目（遵循服务依赖顺序）"
              >
                全启
              </button>
              <button
                type="button"
                className="btn btn-ghost btn-xs"
                onClick={() => handleStopBatch()}
                disabled={busy !== ""}
                title="停止全部项目（逆依赖顺序）"
              >
                全停
              </button>
              <button
                type="button"
                className="btn btn-ghost btn-xs btn-new-group"
                onClick={() => setCreateGroupOpen((prev) => !prev)}
                title="新建分组"
              >
                <PlusIcon /> 新建分组
              </button>
            </div>
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
                    <div className="group-card">
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
                      {!section.isUngrouped && (
                      <div className="group-header-actions">
                        <button
                          type="button"
                          className="btn btn-ghost btn-xs"
                          onClick={(e) => {
                            e.stopPropagation();
                            handleStartBatch(section.key);
                          }}
                          disabled={busy !== ""}
                          title="按配置顺序启动，前一个就绪后再启下一个"
                        >
                          全部启动
                        </button>
                        <button
                          type="button"
                          className="btn btn-ghost btn-xs"
                          onClick={(e) => {
                            e.stopPropagation();
                            handleStopBatch(section.isUngrouped ? undefined : section.key);
                          }}
                          disabled={busy !== ""}
                          title={`停止组内全部项目 (${section.label})`}
                        >
                          停止
                        </button>
                      </div>
                      )}
                    </div>
                    {!section.isUngrouped && section.members.length > 0 && (
                      <div className="group-order" title="全部启动时按此顺序，前一个就绪后再启下一个">
                        {section.members.join(" → ")}
                      </div>
                    )}
                    {!collapsed &&
                      (section.visibleProjects.length === 0 ? (
                        <div className="group-empty-drop-zone">拖动项目至此归类</div>
                      ) : (
                        <ul className="projects">
                          {section.visibleProjects.map((project) => renderProjectItem(project))}
                        </ul>
                      ))}
                    </div>
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
                  <span className="menu-label">当前版本 v{displayVersion}</span>
                </div>
                <button
                  type="button"
                  role="menuitem"
                  disabled={checkingUpdate}
                  onClick={() => {
                    setMenu("");
                    void handleCheckUpdate(true);
                  }}
                >
                  <RefreshIcon /> {checkingUpdate ? "正在检查更新…" : "检查更新"}
                </button>
                <div className="menu-sep" />
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

      {/* Resizer Splitter */}
      <div
        onMouseDown={handleMouseDownResize}
        className="w-1 -mr-0.5 z-10 cursor-col-resize hover:bg-[var(--color-accent)]/50 transition-colors select-none shrink-0"
        title="拖拽调整侧栏宽度"
      />

      <main className="main" style={{ flex: 1, minWidth: 0, height: "100%", overflowY: "auto" }}>
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
          <div className="flex-1 flex flex-col h-full min-h-0 overflow-hidden">
            {/* 34px Tab Bar */}
            <div className="h-[34px] px-4 border-b border-[var(--color-border)] bg-[var(--color-surface)] flex items-center justify-between shrink-0 select-none">
              <div className="flex items-center gap-4 h-full" role="tablist" aria-label="项目视图">
                <button
                  type="button"
                  role="tab"
                  aria-selected={activeTab === "overview"}
                  onClick={() => setActiveTab("overview")}
                  className={`h-full flex items-center px-1 text-xs cursor-pointer border-b-2 font-medium transition-colors ${
                    activeTab === "overview"
                      ? "border-[var(--color-accent)] text-[var(--color-text)]"
                      : "border-transparent text-[var(--color-text-muted)] hover:text-[var(--color-text)]"
                  }`}
                >
                  概览与控制
                </button>
                <button
                  type="button"
                  role="tab"
                  aria-selected={activeTab === "logs"}
                  onClick={() => setActiveTab("logs")}
                  className={`h-full flex items-center px-1 text-xs cursor-pointer border-b-2 font-medium transition-colors ${
                    activeTab === "logs"
                      ? "border-[var(--color-accent)] text-[var(--color-text)]"
                      : "border-transparent text-[var(--color-text-muted)] hover:text-[var(--color-text)]"
                  }`}
                >
                  实时日志
                </button>
                <button
                  type="button"
                  role="tab"
                  aria-selected={activeTab === "details"}
                  onClick={() => setActiveTab("details")}
                  className={`h-full flex items-center px-1 text-xs cursor-pointer border-b-2 font-medium transition-colors ${
                    activeTab === "details"
                      ? "border-[var(--color-accent)] text-[var(--color-text)]"
                      : "border-transparent text-[var(--color-text-muted)] hover:text-[var(--color-text)]"
                  }`}
                >
                  配置与环境
                </button>
              </div>

              <div className="flex items-center gap-2">
                <span className="font-mono text-[11px] text-[var(--color-text-subtle)] truncate max-w-[240px]">
                  {current.id}
                </span>
              </div>
            </div>

            {activeTab === "overview" && (
              <ProjectOverview
                project={current}
                status={status}
                busy={busy}
                error={error}
                log={log}
                logPath={logPath}
                canOpenLog={canOpenLog}
                canStart={canStart}
                canStop={canStop}
                canRestart={canRestart}
                canInstall={canInstall}
                canOpenPage={canOpenPage}
                selectedProfile={selectedProfile}
                onProfileChange={setSelectedProfile}
                copiedCmd={copiedCmd}
                pagesOpen={pagesDropdownOpen}
                onTogglePages={() => {
                  setMenu("");
                  setPagesDropdownOpen((prev) => !prev);
                }}
                onClosePages={() => setPagesDropdownOpen(false)}
                moreOpen={menu === "more"}
                onToggleMore={() => {
                  setPagesDropdownOpen(false);
                  setMenu(menu === "more" ? "" : "more");
                }}
                onCloseMore={() => setMenu("")}
                onStart={() => handleStart()}
                onStop={handleStop}
                onRestart={() => handleRestart()}
                onOpenPage={(url) => {
                  if (url) {
                    invoke("open_page", { url }).catch((err: unknown) => setError(String(err)));
                  } else {
                    void reveal("page");
                  }
                }}
                onOpenAllPages={handleOpenAllPages}
                onEdit={handleOpenEdit}
                onRefresh={() => {
                  setError("");
                  refresh(selected).catch((err: unknown) => setError(String(err)));
                }}
                onOpenCursor={openInCursor}
                onOpenFolder={() => reveal("folder")}
                onOpenTerminal={() => handleOpenTerminal()}
                onOpenLog={() => reveal("log")}
                onCopyCommand={copyStartCommand}
                onInstall={handleInstall}
                onDelete={handleDeleteProject}
                onDismissError={() => setError("")}
                onShowLogs={() => setActiveTab("logs")}
                stopTitle={stopButtonTitle(status)}
                openPageTitle={getOpenPageTitle()}
                toolchain={toolchainOf(current)}
                rootPath={status?.root ?? current.root}
              />
            )}

        {activeTab === "logs" && (
          <div className="flex-1 flex flex-col h-full min-h-0 overflow-hidden bg-[var(--color-bg)]">
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
        )}

        {activeTab === "details" && (
          <ProjectDetailsTab
            project={current}
            status={status}
            onEdit={handleOpenEdit}
            onOpenFolder={() => reveal("folder")}
          />
        )}
      </div>
    )}
      </main>
      </div>

      <StatusBar
        selectedProject={current ?? undefined}
        status={status}
        totalProjects={projectList.length}
        runningCount={runningCount}
        isRefreshing={isRefreshing}
        configPath={projects?.config_path ?? configInfo?.path ?? configInfo?.default_path}
        onOpenConfigFile={handleOpenConfigFile}
        onOpenTerminal={current ? () => handleOpenTerminal() : undefined}
        onOpenShortcuts={() => setShortcutsOpen(true)}
        onOpenAbout={() => setAboutOpen(true)}
      />

      <AboutModal
        isOpen={aboutOpen}
        onClose={() => setAboutOpen(false)}
        version={displayVersion}
        checkingUpdate={checkingUpdate}
        onCheckUpdate={() => void handleCheckUpdate(true)}
        configPath={projects?.config_path ?? configInfo?.path ?? configInfo?.default_path}
      />

      {activeUpdate && (
        <UpdateModal
          update={activeUpdate}
          onClose={() => {
            activeUpdate.close().catch(() => {});
            setActiveUpdate(null);
          }}
        />
      )}

      {updateToast && (
        <div
          role="status"
          className={`fixed bottom-10 left-1/2 -translate-x-1/2 z-[60] px-4 py-2 rounded-lg text-xs font-medium shadow-lg border ${
            updateToast.tone === "danger"
              ? "bg-[var(--color-danger)]/15 border-[var(--color-danger)]/40 text-[var(--color-danger)]"
              : "bg-[var(--color-surface)] border-[var(--color-border)] text-[var(--color-text)]"
          }`}
        >
          {updateToast.text}
        </div>
      )}

      <ShortcutsHelpModal
        isOpen={shortcutsOpen}
        onClose={() => setShortcutsOpen(false)}
      />

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

function pageUrl(status: Status | null, project: { port: number } | null): string {
  const probe = status?.login_probe.url.trim() ?? "";
  if (probe.startsWith("http://") || probe.startsWith("https://")) return probe;
  const port = status?.port ?? project?.port ?? 0;
  if (port > 0) return `http://127.0.0.1:${port}/`;
  return "";
}
