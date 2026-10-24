import { invoke } from "@tauri-apps/api/core";
import { useState } from "react";
import type { ImportReport, ScanReport } from "./types";
import {
  CONFIDENCE_LABEL,
  pickedRows,
  rowProblems,
  toImportItems,
  toRows,
  type ScanRow,
} from "./scanImport";
import { FolderIcon, SearchIcon } from "./icons";

interface ScanDialogProps {
  onClose: () => void;
  /** Called after projects were written; gets the first added id. */
  onImported: (firstId: string | null, report: ImportReport) => Promise<void> | void;
}

export function ScanDialog({ onClose, onImported }: ScanDialogProps) {
  const [dir, setDir] = useState("");
  const [depth, setDepth] = useState(3);
  const [rows, setRows] = useState<ScanRow[] | null>(null);
  const [summary, setSummary] = useState("");
  const [error, setError] = useState("");
  const [scanning, setScanning] = useState(false);
  const [saving, setSaving] = useState(false);

  async function pickAndScan() {
    try {
      const picked = await invoke<string | null>("pick_folder");
      if (picked) {
        setDir(picked);
        await runScan(picked);
      }
    } catch (err: unknown) {
      setError(String(err));
    }
  }

  async function runScan(target = dir) {
    if (!target.trim()) {
      setError("先选一个目录");
      return;
    }
    setError("");
    setScanning(true);
    try {
      const report = await invoke<ScanReport>("scan_dir", { dir: target.trim(), depth });
      const next = toRows(report.candidates);
      setRows(next);
      const fresh = next.filter((r) => !r.existing_id).length;
      setSummary(
        `在 ${report.root} 下看了 ${report.dirs_visited} 个目录，找到 ${report.candidates.length} 个项目，其中 ${fresh} 个是新的` +
          (report.truncated ? "（目录太多已截断，换个小一点的目录试试）" : ""),
      );
    } catch (err: unknown) {
      setError(String(err));
      setRows(null);
    } finally {
      setScanning(false);
    }
  }

  function update(i: number, patch: Partial<ScanRow>) {
    setRows((prev) => prev && prev.map((r, k) => (k === i ? { ...r, ...patch } : r)));
  }

  const problems = rows ? rowProblems(rows) : {};
  const picked = rows ? pickedRows(rows) : [];
  const blocked = Object.keys(problems).length > 0;

  async function handleImport() {
    if (!rows || !picked.length || blocked) return;
    setSaving(true);
    setError("");
    try {
      const report = await invoke<ImportReport>("import_scanned", { items: toImportItems(rows) });
      if (report.skipped.length) {
        setError(
          `已添加 ${report.added.length} 个；跳过：` +
            report.skipped.map((s) => `${s.id}（${s.reason}）`).join("；"),
        );
      }
      await onImported(report.added[0]?.id ?? null, report);
      if (!report.skipped.length) onClose();
    } catch (err: unknown) {
      setError(String(err));
    } finally {
      setSaving(false);
    }
  }

  return (
    <div
      className="modal-backdrop"
      role="dialog"
      aria-modal="true"
      onClick={(e) => {
        if (e.target === e.currentTarget && !saving) onClose();
      }}
    >
      <div className="modal-container modal-wide">
        <div className="modal-header">
          <h2>扫描目录</h2>
          <button type="button" className="modal-close" disabled={saving} onClick={onClose} aria-label="关闭">
            ×
          </button>
        </div>
        <div className="modal-form">
          <div className="scan-bar">
            <div className="input-with-button scan-dir">
              <input
                type="text"
                value={dir}
                placeholder="放代码的目录，例如 ~/code"
                onChange={(e) => setDir(e.target.value)}
                onKeyDown={(e) => {
                  if (e.key === "Enter") runScan();
                }}
              />
              <button type="button" className="btn-pick" onClick={pickAndScan} disabled={scanning || saving}>
                <FolderIcon /> 选择…
              </button>
            </div>
            <label className="scan-depth">
              深度
              <input
                type="number"
                min={1}
                max={8}
                value={depth}
                onChange={(e) => setDepth(Math.max(1, Math.min(8, Number(e.target.value) || 3)))}
              />
            </label>
            <button type="button" className="btn btn-primary" onClick={() => runScan()} disabled={scanning || saving}>
              <SearchIcon /> {scanning ? "扫描中…" : "扫描"}
            </button>
          </div>
          <p className="field-hint">
            {summary ||
              "只读不改：自动认出 Spring Boot、Tomcat WAR、Vite / Vue / React、Vue CLI、Next、Nuxt、npm start、Django / FastAPI / Flask，勾选改好再添加。已经注册过的目录不会重复添加。"}
          </p>
          {error && <div className="form-error-banner">{error}</div>}

          {rows && rows.length === 0 && (
            <div className="scan-empty">没找到项目。试试上一级目录，或者把深度调大。</div>
          )}
          {rows && rows.length > 0 && (
            <ul className="scan-list">
              {rows.map((r, i) => {
                const done = !!r.existing_id;
                return (
                  <li key={`${r.root}-${r.kind}`} className={`scan-row${done ? " done" : ""}${problems[i] ? " invalid" : ""}`}>
                    <input
                      type="checkbox"
                      className="scan-check"
                      checked={r.picked && !done}
                      disabled={done}
                      onChange={(e) => update(i, { picked: e.target.checked })}
                      aria-label={`选择 ${r.name}`}
                    />
                    <div className="scan-main">
                      <div className="scan-title">
                        <strong>{r.label}</strong>
                        <span className={`scan-conf conf-${r.confidence}`}>{CONFIDENCE_LABEL[r.confidence]}</span>
                        {r.java && <span className="tool-tag">Java {r.java}</span>}
                        {r.node && <span className="tool-tag">Node {r.node}</span>}
                        {r.needs_tomcat && <span className="tool-tag">需要 Tomcat</span>}
                        {r.profiles && Object.keys(r.profiles).length > 0 && (
                          <span className="tool-tag">Profile: {Object.keys(r.profiles).join(", ")}</span>
                        )}
                        {done && <span className="tool-tag">已注册为 {r.existing_id}</span>}
                      </div>
                      <code className="scan-root">{r.root}</code>
                      {!done && (
                        <div className="scan-fields">
                          <label>
                            ID
                            <input type="text" value={r.id} onChange={(e) => update(i, { id: e.target.value })} />
                          </label>
                          <label>
                            名称
                            <input type="text" value={r.name} onChange={(e) => update(i, { name: e.target.value })} />
                          </label>
                          <label className="wide">
                            启动命令
                            <input type="text" value={r.start} onChange={(e) => update(i, { start: e.target.value })} />
                          </label>
                          <label className="narrow">
                            端口{r.port_guessed ? "（猜）" : ""}
                            <input
                              type="number"
                              value={r.port || ""}
                              placeholder="?"
                              onChange={(e) => update(i, { port: Number(e.target.value) || 0 })}
                            />
                          </label>
                          {r.needs_tomcat && (
                            <label className="full">
                              Tomcat 目录（不填就先加成停用）
                              <input
                                type="text"
                                value={r.tomcat}
                                placeholder="~/opt/tomcat-9"
                                onChange={(e) => update(i, { tomcat: e.target.value })}
                              />
                            </label>
                          )}
                          {r.profiles && Object.keys(r.profiles).length > 0 && (
                            <label className="full">
                              环境预设（Profile 候选：{Object.keys(r.profiles).join("、")}）· 默认生效 Profile
                              <select
                                value={r.default_profile || ""}
                                onChange={(e) => update(i, { default_profile: e.target.value.trim() || null })}
                              >
                                <option value="">（不设默认，直接使用基础启动命令）</option>
                                {Object.keys(r.profiles).map((pName) => {
                                  const pConf = r.profiles![pName];
                                  const portHint = pConf.port ? ` · 端口 :${pConf.port}` : "";
                                  const isRec = pName === "dev" || pName === r.default_profile;
                                  return (
                                    <option key={pName} value={pName}>
                                      {pName}{portHint}{isRec ? "（推断推荐）" : ""}
                                    </option>
                                  );
                                })}
                              </select>
                            </label>
                          )}
                        </div>
                      )}
                      {problems[i] && <span className="field-error">{problems[i]}</span>}
                      {r.why.length > 0 && (
                        <details className="scan-why">
                          <summary>为什么这么猜</summary>
                          <ul>
                            {r.why.map((w) => (
                              <li key={w}>{w}</li>
                            ))}
                          </ul>
                        </details>
                      )}
                    </div>
                  </li>
                );
              })}
            </ul>
          )}

          <div className="modal-actions">
            <button type="button" className="btn-cancel" disabled={saving} onClick={onClose}>
              取消
            </button>
            <button
              type="button"
              className="primary btn-submit"
              disabled={saving || !picked.length || blocked}
              onClick={handleImport}
            >
              {saving ? "添加中…" : picked.length ? `添加选中（${picked.length}）` : "添加选中"}
            </button>
          </div>
        </div>
      </div>
    </div>
  );
}
