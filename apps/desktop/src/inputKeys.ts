export type KeyName =
  | "up"
  | "down"
  | "left"
  | "right"
  | "enter"
  | "tab"
  | "esc"
  | "ctrl-c";

export type InputPayload = {
  text?: string;
  enter?: boolean;
  key?: string;
};

export type QuickButton = {
  label: string;
  title: string;
  payload: InputPayload;
};

export const QUICK_INPUT_BUTTONS: QuickButton[] = [
  { label: "↑", title: "发送 ↑ 方向键", payload: { key: "up" } },
  { label: "↓", title: "发送 ↓ 方向键", payload: { key: "down" } },
  { label: "回车", title: "发送回车换行", payload: { key: "enter" } },
  { label: "y", title: "发送 y 并回车", payload: { text: "y", enter: true } },
  { label: "n", title: "发送 n 并回车", payload: { text: "n", enter: true } },
  { label: "Ctrl-C", title: "发送中断信号 (Ctrl-C)", payload: { key: "ctrl-c" } },
];

/**
 * 当输入框为空时，把键盘按键映射为终端按键名称：
 * ↑ -> up
 * ↓ -> down
 * ← -> left
 * → -> right
 * Tab -> tab
 * Esc -> esc
 * Enter -> enter
 * Ctrl-C -> ctrl-c
 */
export function mapEmptyInputKey(
  eventKey: string,
  modifiers?: { ctrlKey?: boolean; metaKey?: boolean }
): KeyName | null {
  if (modifiers?.ctrlKey || modifiers?.metaKey) {
    if (eventKey.toLowerCase() === "c") {
      return "ctrl-c";
    }
  }

  switch (eventKey) {
    case "ArrowUp":
      return "up";
    case "ArrowDown":
      return "down";
    case "ArrowLeft":
      return "left";
    case "ArrowRight":
      return "right";
    case "Tab":
      return "tab";
    case "Escape":
    case "Esc":
      return "esc";
    case "Enter":
      return "enter";
    default:
      return null;
  }
}

/**
 * 根据按键事件和当前输入框内容计算应发送的载荷：
 * 1. 若输入框为空，按 ↑ ↓ ← → Tab Esc / Enter / Ctrl-C 返回对应按键载荷。
 * 2. 若输入框有内容，按 Enter 返回 { text: currentText, enter: true }。
 * 3. 任何时候按 Ctrl-C 返回 { key: "ctrl-c" }。
 * 4. 其余情况返回 null。
 */
export function resolveKeyboardInput(
  e: { key: string; ctrlKey?: boolean; metaKey?: boolean },
  currentText: string
): InputPayload | null {
  const isCtrlC = (e.ctrlKey || e.metaKey) && e.key.toLowerCase() === "c";
  if (isCtrlC) {
    return { key: "ctrl-c" };
  }

  if (currentText.length === 0) {
    const key = mapEmptyInputKey(e.key, {
      ctrlKey: e.ctrlKey,
      metaKey: e.metaKey,
    });
    if (key) {
      return { key };
    }
    return null;
  }

  if (e.key === "Enter") {
    return { text: currentText, enter: true };
  }

  return null;
}
