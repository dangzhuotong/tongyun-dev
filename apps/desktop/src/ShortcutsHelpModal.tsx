import { CloseIcon, KeyboardIcon } from "./icons";
import { useEffect } from "react";

export interface ShortcutsHelpModalProps {
  isOpen: boolean;
  onClose: () => void;
}

interface ShortcutItem {
  key: string;
  desc: string;
  category: "全局导航" | "项目控制" | "日志查看";
}

export function ShortcutsHelpModal({ isOpen, onClose }: ShortcutsHelpModalProps) {
  useEffect(() => {
    if (!isOpen) return;
    const handleKeyDown = (e: KeyboardEvent) => {
      if (e.key === "Escape") onClose();
    };
    window.addEventListener("keydown", handleKeyDown);
    return () => window.removeEventListener("keydown", handleKeyDown);
  }, [isOpen, onClose]);

  if (!isOpen) return null;

  const isMac = typeof navigator !== "undefined" && /Mac|iPod|iPhone|iPad/.test(navigator.platform);

  const shortcuts: ShortcutItem[] = [
    {
      key: isMac ? "Cmd+K" : "Ctrl+K",
      desc: "聚焦搜索框并选中全部文本",
      category: "全局导航",
    },
    {
      key: "/",
      desc: "快速聚焦项目搜索框",
      category: "全局导航",
    },
    {
      key: "Enter",
      desc: "在搜索框输入后直接选中第一个匹配项目",
      category: "全局导航",
    },
    {
      key: "Esc",
      desc: "清空搜索内容 / 关闭打开的弹窗或下拉菜单",
      category: "全局导航",
    },
    {
      key: "Tab",
      desc: "在项目列表、操作按钮与日志输入框间移动焦点",
      category: "全局导航",
    },
    {
      key: "Space (空格)",
      desc: "切换分组折叠状态 / 勾选选项",
      category: "全局导航",
    },
    {
      key: "1 / 2 / 3",
      desc: "在交互命令行中输入序号选择",
      category: "项目控制",
    },
    {
      key: "y / n",
      desc: "交互确认提示 (Yes / No)",
      category: "项目控制",
    },
    {
      key: isMac ? "Cmd+F" : "Ctrl+F",
      desc: "聚焦实时日志搜索框",
      category: "日志查看",
    },
  ];

  const categories = ["全局导航", "项目控制", "日志查看"] as const;

  return (
    <div className="fixed inset-0 z-50 flex items-center justify-center bg-black/50 backdrop-blur-xs p-4 select-none animate-in fade-in duration-100">
      <div className="w-full max-w-lg rounded-xl border border-[var(--color-border)] bg-[var(--color-surface)] shadow-2xl overflow-hidden flex flex-col">
        {/* Header */}
        <div className="flex items-center justify-between px-4 py-3 border-b border-[var(--color-border)] bg-[var(--color-surface-raised)]">
          <div className="flex items-center gap-2">
            <span className="p-1 rounded-md bg-[var(--color-accent)]/15 text-[var(--color-accent)]">
              <KeyboardIcon width={16} height={16} />
            </span>
            <h2 className="text-sm font-semibold text-[var(--color-text)]">
              键盘快捷键
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
        <div className="p-5 space-y-4 max-h-[70vh] overflow-y-auto">
          {categories.map((cat) => {
            const items = shortcuts.filter((s) => s.category === cat);
            if (items.length === 0) return null;
            return (
              <div key={cat} className="space-y-2">
                <h3 className="text-xs font-bold text-[var(--color-accent)] tracking-wider">
                  {cat}
                </h3>
                <div className="rounded-lg border border-[var(--color-border)] bg-[var(--color-surface-raised)] divide-y divide-[var(--color-border)]/50 overflow-hidden">
                  {items.map((s, idx) => (
                    <div
                      key={idx}
                      className="flex items-center justify-between px-3 py-2 text-xs"
                    >
                      <span className="text-[var(--color-text-muted)]">{s.desc}</span>
                      <kbd className="font-mono text-[11px] px-2 py-0.5 rounded bg-[var(--color-surface)] border border-[var(--color-border-strong)] text-[var(--color-text)] shadow-xs">
                        {s.key}
                      </kbd>
                    </div>
                  ))}
                </div>
              </div>
            );
          })}
        </div>

        {/* Footer */}
        <div className="px-4 py-2.5 bg-[var(--color-surface-raised)] border-t border-[var(--color-border)] flex justify-end">
          <button
            type="button"
            onClick={onClose}
            className="px-4 py-1.5 rounded-md text-xs font-medium bg-[var(--color-accent)] text-white hover:opacity-90 transition-opacity cursor-pointer"
          >
            关闭
          </button>
        </div>
      </div>
    </div>
  );
}
