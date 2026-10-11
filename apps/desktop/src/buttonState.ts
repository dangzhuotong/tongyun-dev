import type { LifeState, Project, Status } from "./types";

export interface CanStopProjectOptions {
  /** 选中的项目 ID 或对象；未选中时为 null / undefined / 空字符串 */
  selected?: unknown;
  /** 当前正在执行的全局动作（如 "start" | "stop" | "restart" 等），空字符串表示空闲 */
  busy: string;
  /** 项目信息（即使 disabled 也不阻止停止） */
  project?: Pick<Project, "enabled"> | null;
  /** 状态信息（即使 stopped / null 也不阻止停止） */
  status?: Pick<Status, "state"> | { state?: LifeState | string } | null;
}

/**
 * 判断桌面端「停止」按钮是否可用。
 *
 * 只要选中了项目、且当前没有别的操作在进行（busy === ""）就可点击，
 * 不再因为状态是 stopped / 项目 disabled / status 还没取到而变灰。
 * 后端 stop 本身是幂等的，已停止时执行可清理残留进程与端口占用。
 */
export function canStopProject(options: CanStopProjectOptions): boolean {
  if (!options.selected) {
    return false;
  }
  if (options.busy !== "") {
    return false;
  }
  return true;
}

/**
 * 停止按钮悬停提示。
 * 已停止时提示「清理残留进程和端口占用」。
 */
export function stopButtonTitle(
  statusOrOptions?:
    | Pick<Status, "state">
    | { state?: LifeState | string }
    | { status?: Pick<Status, "state"> | { state?: LifeState | string } | null }
    | null,
): string | undefined {
  if (!statusOrOptions) return undefined;
  const status =
    "status" in statusOrOptions && statusOrOptions.status !== undefined
      ? statusOrOptions.status
      : (statusOrOptions as Pick<Status, "state"> | { state?: LifeState | string });
  if (status?.state === "stopped") {
    return "清理残留进程和端口占用";
  }
  return undefined;
}
