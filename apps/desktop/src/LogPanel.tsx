import { useEffect, useMemo, useRef, useState } from "react";
import type { LogTail } from "./types";
import { findMatches, formatLogText, getLogLevel } from "./logFormat";
import type { InputPayload } from "./inputKeys";
import {
  ArrowDownIcon,
  CheckIcon,
  ChevronDownIcon,
  ChevronUpIcon,
  CopyIcon,
  FileIcon,
  PauseIcon,
  SearchIcon,
  TerminalIcon,
  TrashIcon,
  XIcon,
} from "./icons";

export type LogPanelProps = {
  log: LogTail | null;
  logPath: string;
  canOpenLog: boolean;
  onOpenLog: () => void;
  selected: string;
  running?: boolean;
  onSendInput?: (params: InputPayload) => Promise<void>;
  onOpenTerminal?: () => void;
};

export function LogPanel({
  log,
  logPath,
  canOpenLog,
  onOpenLog,
  selected,
  running = false,
  onSendInput: _onSendInput,
  onOpenTerminal,
}: LogPanelProps) {
  const [query, setQuery] = useState("");
  const [activeMatchIndex, setActiveMatchIndex] = useState(0);
  const [autoScroll, setAutoScroll] = useState(true);
  const [clearedText, setClearedText] = useState<string | null>(null);
  const [copied, setCopied] = useState(false);
  const containerRef = useRef<HTMLPreElement>(null);
  const isScrollingRef = useRef(false);

  // 切换项目时重置清空状态
  useEffect(() => {
    setClearedText(null);
  }, [selected]);

  const lines = useMemo(() => {
    if (!log?.text) return [];
    if (clearedText !== null && log.text === clearedText) {
      return [];
    }
    const textToFormat =
      clearedText !== null && log.text.startsWith(clearedText)
        ? log.text.slice(clearedText.length)
        : log.text;
    const formatted = formatLogText(textToFormat);
    const list = formatted.split("\n");
    if (list.length > 0 && list[list.length - 1] === "") {
      list.pop();
    }
    return list;
  }, [log?.text, clearedText]);

  const matches = useMemo(() => findMatches(lines, query), [lines, query]);

  // Reset active match when search query changes; clamp when the log shrinks
  useEffect(() => {
    setActiveMatchIndex(0);
  }, [query]);
  useEffect(() => {
    if (activeMatchIndex >= matches.length && matches.length > 0) {
      setActiveMatchIndex(matches.length - 1);
    }
  }, [matches.length, activeMatchIndex]);
  // 搜索时暂停自动滚动，免得新日志把定位冲走
  useEffect(() => {
    if (query) setAutoScroll(false);
  }, [query]);

  // When active match changes, scroll to it
  useEffect(() => {
    if (matches.length === 0 || !query) return;
    const el = containerRef.current?.querySelector(
      `[data-match-id="${activeMatchIndex}"]`
    );
    if (el) {
      el.scrollIntoView({ block: "center", behavior: "smooth" });
    }
  }, [activeMatchIndex, matches.length, query]);

  // Auto scroll to bottom on new log text
  useEffect(() => {
    if (!autoScroll) return;
    const node = containerRef.current;
    if (node) {
      node.scrollTop = node.scrollHeight;
    }
  }, [lines, autoScroll]);

  // Detect user scroll up -> pause autoScroll; user scroll to bottom -> resume autoScroll
  const handleScroll = () => {
    if (isScrollingRef.current) return;
    const node = containerRef.current;
    if (!node) return;
    const atBottom = node.scrollHeight - node.scrollTop - node.clientHeight <= 10;
    if (atBottom && !autoScroll) {
      setAutoScroll(true);
    } else if (!atBottom && autoScroll) {
      setAutoScroll(false);
    }
  };

  const scrollToBottom = () => {
    isScrollingRef.current = true;
    setAutoScroll(true);
    const node = containerRef.current;
    if (node) {
      node.scrollTop = node.scrollHeight;
    }
    setTimeout(() => {
      isScrollingRef.current = false;
    }, 100);
  };

  const handleNextMatch = () => {
    if (matches.length === 0) return;
    setActiveMatchIndex((prev) => (prev + 1) % matches.length);
  };

  const handlePrevMatch = () => {
    if (matches.length === 0) return;
    setActiveMatchIndex((prev) => (prev - 1 + matches.length) % matches.length);
  };

  const handleSearchKeyDown = (e: React.KeyboardEvent<HTMLInputElement>) => {
    if (e.key === "Enter") {
      e.preventDefault();
      if (e.shiftKey) {
        handlePrevMatch();
      } else {
        handleNextMatch();
      }
    } else if (e.key === "Escape") {
      e.preventDefault();
      setQuery("");
    }
  };

  const handleClear = () => {
    setClearedText(log?.text ?? "");
  };

  const handleCopyAll = () => {
    if (!lines.length) return;
    navigator.clipboard.writeText(lines.join("\n"));
    setCopied(true);
    setTimeout(() => setCopied(false), 1500);
  };

  const matchesByLine = useMemo(() => {
    const map = new Map<
      number,
      { matchIndex: number; start: number; end: number }[]
    >();
    matches.forEach((m, matchIndex) => {
      const arr = map.get(m.lineIndex) || [];
      arr.push({ matchIndex, start: m.start, end: m.end });
      map.set(m.lineIndex, arr);
    });
    return map;
  }, [matches]);

  return (
    <section className="log" aria-label="实时日志">
      <div className="log-bar">
        <div className="log-bar-left">
          <span className="log-title">
            <span className={`log-dot ${running ? "pulse" : ""}`} />
            实时日志
          </span>
          {logPath ? (
            <code className="log-path font-mono text-[11px]" title={logPath}>
              {logPath}
            </code>
          ) : (
            <span className="log-path-empty">未配置日志路径</span>
          )}
        </div>

        <div className="log-bar-right">
          <label className={`log-search-box ${query ? "has-query" : ""} ${query && matches.length === 0 ? "no-match" : ""}`}>
            <SearchIcon className="log-search-icon" />
            <input
              type="text"
              value={query}
              onChange={(e) => setQuery(e.target.value)}
              onKeyDown={handleSearchKeyDown}
              placeholder="搜索日志 (Ctrl+F)"
              title="按 Enter 下一个，Shift+Enter 上一个，Esc 清空"
            />
            {query && (
              <span className="match-counter">
                {matches.length === 0 ? "无结果" : `${activeMatchIndex + 1}/${matches.length}`}
              </span>
            )}
            {query && (
              <span className="search-nav-buttons">
                <button
                  type="button"
                  title="上一个 (Shift+Enter)"
                  aria-label="上一个"
                  disabled={matches.length === 0}
                  onClick={handlePrevMatch}
                >
                  <ChevronUpIcon />
                </button>
                <button
                  type="button"
                  title="下一个 (Enter)"
                  aria-label="下一个"
                  disabled={matches.length === 0}
                  onClick={handleNextMatch}
                >
                  <ChevronDownIcon />
                </button>
                <button type="button" title="清空 (Esc)" aria-label="清空" onClick={() => setQuery("")}>
                  <XIcon />
                </button>
              </span>
            )}
          </label>

          <button
            type="button"
            className="log-btn"
            onClick={handleClear}
            title="清空当前控制台显示"
          >
            <TrashIcon width={13} height={13} />
            <span>清空</span>
          </button>

          <button
            type="button"
            className="log-btn"
            onClick={handleCopyAll}
            title={copied ? "已复制到剪贴板！" : "复制全部日志文本"}
            disabled={lines.length === 0}
          >
            {copied ? <CheckIcon width={13} height={13} /> : <CopyIcon width={13} height={13} />}
            <span>{copied ? "已复制" : "复制"}</span>
          </button>

          <button
            type="button"
            className="log-btn"
            onClick={() => {
              if (autoScroll) {
                setAutoScroll(false);
              } else {
                scrollToBottom();
              }
            }}
            title={autoScroll ? "暂停自动滚动" : "恢复自动滚动并回到底部"}
          >
            {autoScroll ? <PauseIcon width={13} height={13} /> : <ArrowDownIcon width={13} height={13} />}
            <span>{autoScroll ? "暂停滚动" : "自动滚动"}</span>
          </button>

          <button
            type="button"
            className="log-btn"
            disabled={!canOpenLog}
            onClick={onOpenLog}
            title={canOpenLog ? "使用系统默认程序打开日志文件" : "日志文件不存在"}
          >
            <FileIcon width={13} height={13} />
            <span>打开文件</span>
          </button>

          {onOpenTerminal && (
            <button
              type="button"
              className="log-btn log-btn-cmd"
              onClick={onOpenTerminal}
              title="在系统原生独立终端 (CMD) 中交互运行"
            >
              <TerminalIcon width={13} height={13} />
              <span>打开 CMD</span>
            </button>
          )}
        </div>
      </div>

      <div className="log-body">
        <pre ref={containerRef} onScroll={handleScroll} className="log-pre">
          {log?.missing ? (
            <div className="log-placeholder missing">
              还没有日志
              <small>{logPath || "未配置日志路径"}</small>
            </div>
          ) : lines.length === 0 ? (
            <div className="log-placeholder">
              {clearedText !== null
                ? "日志已清空（有新输出时会自动显示）"
                : selected
                ? "日志是空的，启动后会实时显示在这里"
                : "未选择项目"}
            </div>
          ) : (
            lines.map((line, lineIndex) => {
              const level = getLogLevel(line);
              const lineMatches = matchesByLine.get(lineIndex);

              return (
                <div key={lineIndex} className={`log-line ${level}`}>
                  {lineMatches
                    ? renderLineContent(
                        line,
                        lineIndex,
                        activeMatchIndex,
                        lineMatches
                      )
                    : line || " "}
                </div>
              );
            })
          )}
        </pre>

        {!autoScroll && (
          <button
            type="button"
            className="log-paused-btn"
            onClick={scrollToBottom}
          >
            已暂停 · 回到底部
          </button>
        )}
      </div>

      {/* 底部极简状态与快捷交互栏 */}
      <div className="log-footer-bar">
        <div className="log-footer-left">
          <span className="log-footer-hint">
            共 {lines.length} 行输出 · 按 <kbd className="font-mono text-[10px]">Ctrl+F</kbd> 搜索 · 交互请在独立 CMD 中操作
          </span>
        </div>
        {onOpenTerminal && (
          <button
            type="button"
            className="log-footer-cmd-btn"
            onClick={onOpenTerminal}
            title="在系统独立命令行窗口中运行"
          >
            <TerminalIcon width={12} height={12} />
            <span>打开 CMD 终端 ↗</span>
          </button>
        )}
      </div>
    </section>
  );
}

function renderLineContent(
  line: string,
  _lineIndex: number,
  activeMatchIndex: number,
  lineMatches: { matchIndex: number; start: number; end: number }[]
) {
  const parts: React.ReactNode[] = [];
  let last = 0;
  for (let i = 0; i < lineMatches.length; i++) {
    const m = lineMatches[i];
    if (m.start > last) {
      parts.push(line.slice(last, m.start));
    }
    const isCurrent = m.matchIndex === activeMatchIndex;
    parts.push(
      <mark
        key={`m-${i}`}
        data-match-id={m.matchIndex}
        className={`search-match ${isCurrent ? "current" : ""}`}
      >
        {line.slice(m.start, m.end)}
      </mark>
    );
    last = m.end;
  }
  if (last < line.length) {
    parts.push(line.slice(last));
  }
  return parts;
}
