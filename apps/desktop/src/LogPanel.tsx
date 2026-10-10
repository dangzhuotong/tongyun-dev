import { useEffect, useMemo, useRef, useState } from "react";
import type { LogTail } from "./types";
import { findMatches, getLogLevel } from "./logFormat";

export type LogPanelProps = {
  log: LogTail | null;
  logPath: string;
  canOpenLog: boolean;
  onOpenLog: () => void;
  selected: string;
};

export function LogPanel({
  log,
  logPath,
  canOpenLog,
  onOpenLog,
  selected,
}: LogPanelProps) {
  const [query, setQuery] = useState("");
  const [activeMatchIndex, setActiveMatchIndex] = useState(0);
  const [autoScroll, setAutoScroll] = useState(true);
  const containerRef = useRef<HTMLPreElement>(null);
  const isScrollingRef = useRef(false);

  const lines = useMemo(() => {
    if (!log?.text) return [];
    const list = log.text.split("\n");
    if (list.length > 0 && list[list.length - 1] === "") {
      list.pop();
    }
    return list;
  }, [log?.text]);

  const matches = useMemo(() => findMatches(lines, query), [lines, query]);

  // Reset active match when search query or lines change
  useEffect(() => {
    setActiveMatchIndex(0);
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
  }, [log?.text, autoScroll]);

  function handleScroll() {
    if (isScrollingRef.current) return;
    const node = containerRef.current;
    if (!node) return;
    const distanceFromBottom =
      node.scrollHeight - node.scrollTop - node.clientHeight;
    if (distanceFromBottom > 35) {
      if (autoScroll) setAutoScroll(false);
    } else {
      if (!autoScroll) setAutoScroll(true);
    }
  }

  function scrollToBottom() {
    setAutoScroll(true);
    const node = containerRef.current;
    if (node) {
      node.scrollTop = node.scrollHeight;
    }
  }

  function handlePrevMatch() {
    if (matches.length === 0) return;
    setActiveMatchIndex((prev) => (prev - 1 + matches.length) % matches.length);
  }

  function handleNextMatch() {
    if (matches.length === 0) return;
    setActiveMatchIndex((prev) => (prev + 1) % matches.length);
  }

  function handleSearchKeyDown(e: React.KeyboardEvent<HTMLInputElement>) {
    if (e.key === "Enter") {
      e.preventDefault();
      if (e.shiftKey) {
        handlePrevMatch();
      } else {
        handleNextMatch();
      }
    } else if (e.key === "ArrowDown") {
      e.preventDefault();
      handleNextMatch();
    } else if (e.key === "ArrowUp") {
      e.preventDefault();
      handlePrevMatch();
    } else if (e.key === "Escape") {
      setQuery("");
    }
  }

  // Group matches by lineIndex for rendering
  const matchesByLine = useMemo(() => {
    const map = new Map<number, { matchIndex: number; start: number; end: number }[]>();
    for (let idx = 0; idx < matches.length; idx++) {
      const m = matches[idx];
      let list = map.get(m.lineIndex);
      if (!list) {
        list = [];
        map.set(m.lineIndex, list);
      }
      list.push({ matchIndex: idx, start: m.start, end: m.end });
    }
    return map;
  }, [matches]);

  return (
    <section className="log" id="log-panel" aria-label="log panel">
      <div className="log-bar">
        <div className="log-bar-left">
          <strong>实时日志</strong>
          <span className="log-path" title={logPath}>
            {logPath}
          </span>
        </div>
        <div className="log-bar-actions">
          <div className="log-search-box">
            <input
              type="text"
              placeholder="搜索日志 (回车/上下箭头切换)"
              value={query}
              onChange={(e) => setQuery(e.target.value)}
              onKeyDown={handleSearchKeyDown}
            />
            {query && (
              <span className="match-counter">
                {matches.length > 0 ? `${activeMatchIndex + 1}/${matches.length}` : "0/0"}
              </span>
            )}
            {query && (
              <div className="search-nav-buttons">
                <button
                  type="button"
                  title="上一个 (Shift+Enter, ↑)"
                  disabled={matches.length === 0}
                  onClick={handlePrevMatch}
                >
                  ↑
                </button>
                <button
                  type="button"
                  title="下一个 (Enter, ↓)"
                  disabled={matches.length === 0}
                  onClick={handleNextMatch}
                >
                  ↓
                </button>
              </div>
            )}
          </div>
          <button
            type="button"
            className="log-toggle-scroll"
            onClick={() => {
              if (autoScroll) {
                setAutoScroll(false);
              } else {
                scrollToBottom();
              }
            }}
          >
            {autoScroll ? "暂停滚动" : "自动滚动"}
          </button>
          <button
            type="button"
            disabled={!canOpenLog}
            onClick={onOpenLog}
            title={canOpenLog ? "使用系统默认程序打开日志文件" : "日志文件不存在"}
          >
            打开日志文件
          </button>
        </div>
      </div>

      <div className="log-body">
        <pre ref={containerRef} onScroll={handleScroll} className="log-pre">
          {log?.missing ? (
            <div className="log-placeholder missing">
              还没有日志：{logPath || "未配置日志路径"}
            </div>
          ) : lines.length === 0 ? (
            <div className="log-placeholder">
              {selected ? "日志是空的。" : "未选择项目"}
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
