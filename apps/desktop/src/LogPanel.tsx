import { invoke } from "@tauri-apps/api/core";
import { useEffect, useMemo, useRef, useState } from "react";
import type { LogTail } from "./types";
import { findMatches, formatLogText, getLogLevel } from "./logFormat";
import {
  type InputPayload,
  QUICK_INPUT_BUTTONS,
  type QuickButton,
  resolveKeyboardInput,
} from "./inputKeys";
import { ArrowDownIcon, ChevronDownIcon, ChevronUpIcon, FileIcon, PauseIcon, SearchIcon, XIcon } from "./icons";

export type LogPanelProps = {
  log: LogTail | null;
  logPath: string;
  canOpenLog: boolean;
  onOpenLog: () => void;
  selected: string;
  running?: boolean;
  onSendInput?: (params: InputPayload) => Promise<void>;
};

export function LogPanel({
  log,
  logPath,
  canOpenLog,
  onOpenLog,
  selected,
  running = false,
  onSendInput,
}: LogPanelProps) {
  const [query, setQuery] = useState("");
  const [activeMatchIndex, setActiveMatchIndex] = useState(0);
  const [autoScroll, setAutoScroll] = useState(true);
  const containerRef = useRef<HTMLPreElement>(null);
  const isScrollingRef = useRef(false);

  const [inputText, setInputText] = useState("");
  const [inputError, setInputError] = useState("");
  const [sending, setSending] = useState(false);
  const inputRef = useRef<HTMLInputElement>(null);

  useEffect(() => {
    setInputError("");
  }, [selected, running]);

  const lines = useMemo(() => {
    if (!log?.text) return [];
    const formatted = formatLogText(log.text);
    const list = formatted.split("\n");
    if (list.length > 0 && list[list.length - 1] === "") {
      list.pop();
    }
    return list;
  }, [log?.text]);

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

  async function handleSend(
    payload: InputPayload,
    clearOnSuccess = false
  ) {
    if (!selected || !running) return;
    setInputError("");
    setSending(true);
    try {
      if (onSendInput) {
        await onSendInput(payload);
      } else {
        await invoke("send_input", {
          id: selected,
          text: payload.text ?? null,
          enter: payload.enter ?? null,
          key: payload.key ?? null,
        });
      }
      if (clearOnSuccess) {
        setInputText("");
      }
    } catch (err: unknown) {
      const msg =
        typeof err === "string"
          ? err
          : err instanceof Error
          ? err.message
          : String(err);
      setInputError(msg || "发送输入失败");
    } finally {
      setSending(false);
    }
  }

  function handleInputKeyDown(e: React.KeyboardEvent<HTMLInputElement>) {
    if (!running) return;
    const payload = resolveKeyboardInput(
      { key: e.key, ctrlKey: e.ctrlKey, metaKey: e.metaKey },
      inputText
    );
    if (payload) {
      e.preventDefault();
      const clear = Boolean(payload.text);
      handleSend(payload, clear);
    }
  }

  function handleQuickButtonClick(btn: QuickButton) {
    if (!running || sending) return;
    handleSend(btn.payload, false);
    inputRef.current?.focus();
  }

  return (
    <section className="log" id="log-panel" aria-label="log panel">
      <div className="log-bar">
        <div className="log-bar-left">
          <span className="log-title">
            <span className={`log-live${autoScroll ? "" : " paused"}`} />
            实时日志
          </span>
          <span className="log-path" title={logPath}>
            {logPath}
          </span>
        </div>
        <div className="log-bar-actions">
          <label className={`log-search-box${query ? " has-query" : ""}${query && matches.length === 0 ? " no-match" : ""}`}>
            <SearchIcon className="log-search-icon" />
            <input
              type="text"
              placeholder="搜索日志"
              aria-label="搜索日志"
              value={query}
              onChange={(e) => setQuery(e.target.value)}
              onKeyDown={handleSearchKeyDown}
              title="Enter 下一个 · Shift+Enter 上一个 · Esc 清空"
            />
            {query && (
              <span className="match-counter" aria-live="polite">
                {matches.length > 0 ? `${activeMatchIndex + 1}/${matches.length}` : "无结果"}
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
            onClick={() => {
              if (autoScroll) {
                setAutoScroll(false);
              } else {
                scrollToBottom();
              }
            }}
            title={autoScroll ? "暂停自动滚动" : "恢复自动滚动并回到底部"}
          >
            {autoScroll ? <PauseIcon /> : <ArrowDownIcon />}
            {autoScroll ? "暂停滚动" : "自动滚动"}
          </button>
          <button
            type="button"
            className="log-btn"
            disabled={!canOpenLog}
            onClick={onOpenLog}
            title={canOpenLog ? "使用系统默认程序打开日志文件" : "日志文件不存在"}
          >
            <FileIcon />
            打开文件
          </button>
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
              {selected ? "日志是空的，启动后会实时显示在这里" : "未选择项目"}
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

      <div className="log-input-bar">
        <div className="log-input-main">
          <input
            ref={inputRef}
            type="text"
            className="log-terminal-input"
            value={inputText}
            onChange={(e) => {
              setInputText(e.target.value);
              if (inputError) setInputError("");
            }}
            onKeyDown={handleInputKeyDown}
            disabled={!running || sending}
            placeholder={
              running
                ? "输入内容，回车发送；为空时按 ↑ ↓ ← → Tab Esc 直接发键"
                : "项目运行中才能输入"
            }
            title={running ? "" : "项目运行中才能输入"}
            aria-label="终端输入框"
          />
          {inputError && (
            <span className="log-input-error" role="alert" title={inputError}>
              {inputError}
            </span>
          )}
        </div>
        <div className="log-quick-buttons" role="group" aria-label="快捷按键">
          {QUICK_INPUT_BUTTONS.map((btn) => (
            <button
              key={btn.label}
              type="button"
              className="log-input-btn"
              disabled={!running || sending}
              onClick={() => handleQuickButtonClick(btn)}
              title={running ? btn.title : "项目运行中才能输入"}
            >
              {btn.label}
            </button>
          ))}
        </div>
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
