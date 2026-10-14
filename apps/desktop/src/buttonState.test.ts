import { describe, expect, it } from "vitest";
import { canStopProject, stopButtonTitle } from "./buttonState";

describe("canStopProject", () => {
  it("未选中返回 false", () => {
    expect(canStopProject({ selected: null, busy: "" })).toBe(false);
    expect(canStopProject({ selected: undefined, busy: "" })).toBe(false);
    expect(canStopProject({ selected: "", busy: "" })).toBe(false);
  });

  it("busy 非空返回 false", () => {
    expect(canStopProject({ selected: "demo-api", busy: "start" })).toBe(false);
    expect(canStopProject({ selected: "demo-api", busy: "stop" })).toBe(false);
    expect(canStopProject({ selected: "demo-api", busy: "restart" })).toBe(false);
    expect(canStopProject({ selected: "demo-api", busy: "install" })).toBe(false);
  });

  it("状态 stopped / failed / starting / ready / status 为 null / 项目 disabled 均返回 true", () => {
    // 状态 stopped
    expect(canStopProject({ selected: "demo-api", busy: "", status: { state: "stopped" } })).toBe(true);
    // 状态 failed
    expect(canStopProject({ selected: "demo-api", busy: "", status: { state: "failed" } })).toBe(true);
    // 状态 starting
    expect(canStopProject({ selected: "demo-api", busy: "", status: { state: "starting" } })).toBe(true);
    // 状态 ready
    expect(canStopProject({ selected: "demo-api", busy: "", status: { state: "ready" } })).toBe(true);
    // status 为 null
    expect(canStopProject({ selected: "demo-api", busy: "", status: null })).toBe(true);
    // 项目 disabled
    expect(canStopProject({ selected: "demo-api", busy: "", project: { enabled: false } })).toBe(true);
  });
});

describe("stopButtonTitle", () => {
  it("已停止时提示「清理残留进程和端口占用」", () => {
    expect(stopButtonTitle({ state: "stopped" })).toBe("清理残留进程和端口占用");
  });

  it("非 stopped 状态或无状态时返回 undefined", () => {
    expect(stopButtonTitle({ state: "ready" })).toBeUndefined();
    expect(stopButtonTitle({ state: "starting" })).toBeUndefined();
    expect(stopButtonTitle({ state: "failed" })).toBeUndefined();
    expect(stopButtonTitle(null)).toBeUndefined();
    expect(stopButtonTitle(undefined)).toBeUndefined();
  });

  it("支持传入包含 status 字段的对象形态", () => {
    expect(stopButtonTitle({ status: { state: "stopped" } })).toBe("清理残留进程和端口占用");
    expect(stopButtonTitle({ status: { state: "ready" } })).toBeUndefined();
    expect(stopButtonTitle({ status: null })).toBeUndefined();
    expect(stopButtonTitle({ status: undefined })).toBeUndefined();
  });
});

