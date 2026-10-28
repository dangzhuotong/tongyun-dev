import { describe, expect, it } from "vitest";
import { renderToStaticMarkup } from "react-dom/server";
import { StatusBar } from "./StatusBar";
import { AboutModal } from "./AboutModal";

describe("agentHttpUi", () => {
  it("renders agent port when running in StatusBar", () => {
    const html = renderToStaticMarkup(
      <StatusBar
        status={null}
        totalProjects={1}
        runningCount={0}
        agentPort={9099}
        agentRunning={true}
        onOpenConfigFile={() => {}}
        onOpenShortcuts={() => {}}
        onOpenAbout={() => {}}
      />,
    );
    expect(html).toContain(":9099");
    expect(html).not.toContain("未运行");
  });

  it("renders 未运行 when stopped in StatusBar", () => {
    const html = renderToStaticMarkup(
      <StatusBar
        status={null}
        totalProjects={1}
        runningCount={0}
        agentPort={null}
        agentRunning={false}
        onOpenConfigFile={() => {}}
        onOpenShortcuts={() => {}}
        onOpenAbout={() => {}}
      />,
    );
    expect(html).toContain("未运行");
    expect(html).toContain("Agent HTTP 服务未运行");
  });

  it("renders agent URL when running in AboutModal", () => {
    const html = renderToStaticMarkup(
      <AboutModal
        isOpen={true}
        onClose={() => {}}
        agentPort={9099}
        agentRunning={true}
      />,
    );
    expect(html).toContain("http://127.0.0.1:9099");
    expect(html).not.toContain("未运行");
  });

  it("renders 未运行 when stopped in AboutModal", () => {
    const html = renderToStaticMarkup(
      <AboutModal
        isOpen={true}
        onClose={() => {}}
        agentPort={null}
        agentRunning={false}
      />,
    );
    expect(html).toContain("未运行");
    expect(html).not.toContain("http://127.0.0.1:");
  });
});
