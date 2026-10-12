import { describe, expect, it } from "vitest";
import {
  buildEditRequest,
  formatAutoAnswersLines,
  formatEnvLines,
  formatLinksLines,
  formatProfilesLines,
  parseAutoAnswersLines,
  parseEnvLines,
  parseLinksLines,
  parseProfilesLines,
  validateProjectForm,
  type EditBaseline,
  type ProjectFormData,
} from "./projectValidation";

describe("validateProjectForm (项目表单校验)", () => {
  const validData: ProjectFormData = {
    id: "test_proj",
    name: "测试项目",
    root: "/home/user/project",
    start: "pnpm dev",
    stop: "pnpm stop",
    port: 3000,
    login_url: "http://127.0.0.1:3000/login",
    health_url: "",
    log_file: "",
    node: "22",
    java: "",
    enabled: true,
    reason: "",
    start_timeout_secs: 180,
  };

  it("passes validation for complete valid project data in create mode", () => {
    const errors = validateProjectForm(validData, false);
    expect(Object.keys(errors)).toHaveLength(0);
  });

  it("passes validation in edit mode without checking ID", () => {
    const editData = { ...validData, id: "" };
    const errors = validateProjectForm(editData, true);
    expect(errors.id).toBeUndefined();
    expect(Object.keys(errors)).toHaveLength(0);
  });

  describe("必填项校验", () => {
    it("reports error when root is missing or whitespace only", () => {
      const errorsEmpty = validateProjectForm({ ...validData, root: "" });
      expect(errorsEmpty.root).toBe("根目录为必填项");

      const errorsSpaces = validateProjectForm({ ...validData, root: "   " });
      expect(errorsSpaces.root).toBe("根目录为必填项");
    });

    it("reports error when start command is missing or whitespace only", () => {
      const errorsEmpty = validateProjectForm({ ...validData, start: "" });
      expect(errorsEmpty.start).toBe("启动命令为必填项");

      const errorsSpaces = validateProjectForm({ ...validData, start: "   " });
      expect(errorsSpaces.start).toBe("启动命令为必填项");
    });

    it("reports error when id is missing or invalid in create mode", () => {
      const errorsEmpty = validateProjectForm({ ...validData, id: "" }, false);
      expect(errorsEmpty.id).toBe("项目 ID 为必填项");

      const errorsInvalid = validateProjectForm({ ...validData, id: "bad id with spaces!" }, false);
      expect(errorsInvalid.id).toContain("字母、数字、下划线");
    });

    it("reports error when enabled is false but reason is empty", () => {
      const errors = validateProjectForm({ ...validData, enabled: false, reason: "" });
      expect(errors.reason).toBe("停用项目时必须填写停用原因");

      const errorsOk = validateProjectForm({ ...validData, enabled: false, reason: "正在维护" });
      expect(errorsOk.reason).toBeUndefined();
    });
  });

  describe("端口范围校验 (端口>0 与 1-65535)", () => {
    it("rejects port <= 0", () => {
      expect(validateProjectForm({ ...validData, port: 0 }).port).toBeDefined();
      expect(validateProjectForm({ ...validData, port: -1 }).port).toBeDefined();
      expect(validateProjectForm({ ...validData, port: -8080 }).port).toBeDefined();
    });

    it("rejects port > 65535", () => {
      expect(validateProjectForm({ ...validData, port: 65536 }).port).toBeDefined();
      expect(validateProjectForm({ ...validData, port: 99999 }).port).toBeDefined();
    });

    it("rejects non-integer ports or NaN", () => {
      expect(validateProjectForm({ ...validData, port: 8080.5 }).port).toBeDefined();
      expect(validateProjectForm({ ...validData, port: "not-a-number" }).port).toBeDefined();
      expect(validateProjectForm({ ...validData, port: "" }).port).toBeDefined();
    });

    it("accepts valid port numbers and valid port strings", () => {
      expect(validateProjectForm({ ...validData, port: 1 }).port).toBeUndefined();
      expect(validateProjectForm({ ...validData, port: 8080 }).port).toBeUndefined();
      expect(validateProjectForm({ ...validData, port: 65535 }).port).toBeUndefined();
      expect(validateProjectForm({ ...validData, port: "5173" }).port).toBeUndefined();
    });
  });
});

describe("buildEditRequest (只提交改过的字段)", () => {
  const base: EditBaseline = {
    name: "my_api",
    root: "~/code/my-api",
    start: "set -e\n./start.sh\n",
    start_editable: true,
    stop: "",
    stop_editable: true,
    port: 8080,
    login_url: "http://127.0.0.1:8080/login",
    health_url: "",
    log_file: "",
    node: null,
    java: "8",
    enabled: true,
    reason: "",
    start_timeout_secs: null,
  };
  const form = (over: Partial<ProjectFormData> = {}): ProjectFormData => ({
    id: "my_api",
    name: base.name,
    root: base.root,
    start: base.start,
    stop: "",
    port: "8080",
    login_url: base.login_url,
    health_url: "",
    log_file: "",
    node: "",
    java: "8",
    enabled: true,
    reason: "",
    start_timeout_secs: "",
    ...over,
  });

  it("未改动时为空，不碰多行 start", () => {
    expect(buildEditRequest(base, form())).toEqual({});
  });
  it("只改端口只发 port", () => {
    expect(buildEditRequest(base, form({ port: "9090" }))).toEqual({ port: 9090 });
  });
  it("清空登录页发空串（删键）", () => {
    expect(buildEditRequest(base, form({ login_url: "" }))).toEqual({ login_url: "" });
  });
  it("start 不可编辑时永不提交 start", () => {
    expect(
      buildEditRequest({ ...base, start_editable: false }, form({ start: "x" }))
    ).toEqual({});
  });
  it("停用时带原因，重新启用时清掉原因", () => {
    expect(buildEditRequest(base, form({ enabled: false, reason: "迁移中" }))).toEqual({
      enabled: false,
      reason: "迁移中",
    });
    expect(
      buildEditRequest({ ...base, enabled: false, reason: "未配置", port: 0 }, form({ port: "8302" }))
    ).toEqual({ enabled: true, reason: "", port: 8302 });
  });
  it("修改分组时提交 group，清空分组时发空串", () => {
    expect(buildEditRequest(base, form({ group: "demo" }))).toEqual({ group: "demo" });
    const baseWithGroup = { ...base, group: "demo" };
    expect(buildEditRequest(baseWithGroup, form({ group: "" }))).toEqual({ group: "" });
  });
});

describe("buildEditRequest auto_install（缺 node_modules 自动装依赖）", () => {
  const base: EditBaseline = {
    name: "web",
    root: "~/code/web",
    start: "pnpm dev --port 5173",
    start_editable: true,
    stop: "",
    stop_editable: true,
    port: 5173,
    login_url: "",
    health_url: "",
    log_file: "",
    node: "22",
    java: null,
    enabled: true,
    reason: "",
  };
  const form: ProjectFormData = {
    id: "web",
    name: "web",
    root: "~/code/web",
    start: "pnpm dev --port 5173",
    stop: "",
    port: 5173,
    login_url: "",
    health_url: "",
    log_file: "",
    node: "22",
    java: "",
    enabled: true,
    reason: "",
    start_timeout_secs: "",
  };

  it("缺省（开着）不提交", () => {
    expect(buildEditRequest(base, { ...form, auto_install: true })).toEqual({});
    expect(buildEditRequest(base, form)).toEqual({});
  });

  it("关掉时提交 false，再打开提交 true", () => {
    expect(buildEditRequest(base, { ...form, auto_install: false })).toEqual({ auto_install: false });
    expect(buildEditRequest({ ...base, auto_install: false }, { ...form, auto_install: true })).toEqual({
      auto_install: true,
    });
  });

  it("修改分组提交新分组，清空分组提交空字符串", () => {
    expect(buildEditRequest(base, { ...form, group: "Frontend" })).toEqual({ group: "Frontend" });
    expect(buildEditRequest({ ...base, group: "Frontend" }, { ...form, group: "" })).toEqual({
      group: "",
    });
    expect(buildEditRequest({ ...base, group: "Frontend" }, { ...form, group: "Frontend" })).toEqual({});
  });
});

describe("validateProjectForm port_mode 校验", () => {
  const baseForm: ProjectFormData = {
    id: "web",
    name: "web",
    root: "~/code/web",
    start: "pnpm dev",
    stop: "",
    port: 5173,
    login_url: "",
    health_url: "",
    log_file: "",
    node: "22",
    java: "",
    enabled: true,
    reason: "",
    start_timeout_secs: "",
  };

  it("接受有效的 port_mode (none, arg, env, config)", () => {
    for (const mode of ["none", "arg", "env", "config"]) {
      const errors = validateProjectForm({ ...baseForm, port_mode: mode });
      expect(errors.port_mode).toBeUndefined();
    }
  });

  it("拒绝未知的 port_mode", () => {
    const errors = validateProjectForm({ ...baseForm, port_mode: "custom_unknown" });
    expect(errors.port_mode).toBeDefined();
    expect(errors.port_mode).toContain("none、arg、env 或 config");
  });

  it("当 port_mode 为 arg 时，port_arg 不能为空白", () => {
    const errors = validateProjectForm({ ...baseForm, port_mode: "arg", port_arg: "   " });
    expect(errors.port_arg).toBeDefined();
    expect(errors.port_arg).toContain("参数名不能为空");

    const errorsOk = validateProjectForm({ ...baseForm, port_mode: "arg", port_arg: "-p" });
    expect(errorsOk.port_arg).toBeUndefined();
  });

  it("当 port_mode 为 env 时，port_env 不能为空白", () => {
    const errors = validateProjectForm({ ...baseForm, port_mode: "env", port_env: "   " });
    expect(errors.port_env).toBeDefined();
    expect(errors.port_env).toContain("环境变量名不能为空");

    const errorsOk = validateProjectForm({ ...baseForm, port_mode: "env", port_env: "VITE_PORT" });
    expect(errorsOk.port_env).toBeUndefined();
  });
});

describe("buildEditRequest port_mode / port_arg / port_env", () => {
  const base: EditBaseline = {
    name: "web",
    root: "~/code/web",
    start: "pnpm dev",
    start_editable: true,
    stop: "",
    stop_editable: true,
    port: 5173,
    login_url: "",
    health_url: "",
    log_file: "",
    node: "22",
    java: null,
    enabled: true,
    reason: "",
    port_mode: "none",
  };

  const form: ProjectFormData = {
    id: "web",
    name: "web",
    root: "~/code/web",
    start: "pnpm dev",
    stop: "",
    port: 5173,
    login_url: "",
    health_url: "",
    log_file: "",
    node: "22",
    java: "",
    enabled: true,
    reason: "",
    start_timeout_secs: "",
    port_mode: "none",
  };

  it("未改动 port_mode 时不提交", () => {
    expect(buildEditRequest(base, form)).toEqual({});
  });

  it("改动为 arg 时提交 port_mode 与自定义 port_arg", () => {
    expect(
      buildEditRequest(base, { ...form, port_mode: "arg", port_arg: "-p" })
    ).toEqual({ port_mode: "arg", port_arg: "-p" });
  });

  it("改动为 env 时提交 port_mode 与自定义 port_env", () => {
    expect(
      buildEditRequest(base, { ...form, port_mode: "env", port_env: "VITE_PORT" })
    ).toEqual({ port_mode: "env", port_env: "VITE_PORT" });
  });

  it("从 arg 切换为 none 时清空旧 port_arg", () => {
    const baseWithArg: EditBaseline = {
      ...base,
      port_mode: "arg",
      port_arg: "--port",
    };
    expect(
      buildEditRequest(baseWithArg, { ...form, port_mode: "none" })
    ).toEqual({ port_mode: "none", port_arg: "" });
  });

  it("改为 config 时提交 port_mode", () => {
    expect(
      buildEditRequest(base, { ...form, port_mode: "config" })
    ).toEqual({ port_mode: "config" });
  });

  describe("pty / env / auto_answers 变更提交", () => {
    it("pty 发生变更时提交 pty", () => {
      expect(buildEditRequest(base, { ...form, pty: true })).toEqual({
        pty: true,
      });

      const baseWithPty: EditBaseline = { ...base, pty: true };
      expect(buildEditRequest(baseWithPty, { ...form, pty: false })).toEqual({
        pty: false,
      });
      expect(buildEditRequest(baseWithPty, { ...form, pty: true })).toEqual({});
    });

    it("env 发生变更时提交解析后的键值对", () => {
      expect(
        buildEditRequest(base, { ...form, env_text: "CI=1\nAPP_ENV=staging" })
      ).toEqual({
        env: { CI: "1", APP_ENV: "staging" },
      });

      const baseWithEnv: EditBaseline = {
        ...base,
        env: { CI: "1", APP_ENV: "staging" },
      };
      // 未变动不提交
      expect(
        buildEditRequest(baseWithEnv, {
          ...form,
          env_text: "CI=1\nAPP_ENV=staging",
        })
      ).toEqual({});

      // 清空环境变量
      expect(
        buildEditRequest(baseWithEnv, { ...form, env_text: "" })
      ).toEqual({
        env: {},
      });
    });

    it("auto_answers 发生变更时提交解析后的应答数组", () => {
      expect(
        buildEditRequest(base, {
          ...form,
          auto_answers_text: "Select env => dev\n(y/N) => y",
        })
      ).toEqual({
        auto_answers: [
          { expect: "Select env", send: "dev", once: true, regex: false },
          { expect: "(y/N)", send: "y", once: true, regex: false },
        ],
      });

      const baseWithAnswers: EditBaseline = {
        ...base,
        auto_answers: [
          { expect: "Select env", send: "dev", once: true, regex: false },
        ],
      };
      // 未变动不提交
      expect(
        buildEditRequest(baseWithAnswers, {
          ...form,
          auto_answers_text: "Select env => dev",
        })
      ).toEqual({});

      // 清空自动应答
      expect(
        buildEditRequest(baseWithAnswers, { ...form, auto_answers_text: "" })
      ).toEqual({
        auto_answers: [],
      });
    });
  });
});

describe("parseEnvLines & formatEnvLines", () => {
  it("parses valid KEY=VALUE lines and ignores empty lines", () => {
    const raw = "  CI=1 \n\n  PORT=3000\nHELLO=WORLD=TEST\n";
    const res = parseEnvLines(raw);
    expect(res.error).toBeUndefined();
    expect(res.env).toEqual({
      CI: "1",
      PORT: "3000",
      HELLO: "WORLD=TEST",
    });
  });

  it("reports error when line is missing equal sign", () => {
    const raw = "CI=1\nINVALID_LINE\nPORT=3000";
    const res = parseEnvLines(raw);
    expect(res.error).toContain("第 2 行");
    expect(res.error).toContain("KEY=VALUE");
  });

  it("reports error when key is empty", () => {
    const raw = "=value";
    const res = parseEnvLines(raw);
    expect(res.error).toContain("第 1 行");
    expect(res.error).toContain("键名不能为空");
  });

  it("formats env map back to string lines", () => {
    expect(formatEnvLines({ A: "1", B: "2" })).toBe("A=1\nB=2");
    expect(formatEnvLines({})).toBe("");
    expect(formatEnvLines(null)).toBe("");
  });
});

describe("parseAutoAnswersLines & formatAutoAnswersLines", () => {
  it("parses valid MATCH=>SEND lines and ignores empty lines", () => {
    const raw = "  Select env => dev \n\n (y/N) => y\n";
    const res = parseAutoAnswersLines(raw);
    expect(res.error).toBeUndefined();
    expect(res.answers).toEqual([
      { expect: "Select env", send: "dev", once: true, regex: false },
      { expect: "(y/N)", send: "y", once: true, regex: false },
    ]);
  });

  it("reports error when line is missing =>", () => {
    const raw = "Select env -> dev";
    const res = parseAutoAnswersLines(raw);
    expect(res.error).toContain("第 1 行");
    expect(res.error).toContain("匹配文字 => 发送内容");
  });

  it("reports error when expect pattern is empty", () => {
    const raw = " => dev";
    const res = parseAutoAnswersLines(raw);
    expect(res.error).toContain("第 1 行");
    expect(res.error).toContain("匹配文字不能为空");
  });

  it("formats auto answers back to string lines", () => {
    expect(
      formatAutoAnswersLines([
        { expect: "foo", send: "bar" },
        { expect: "baz", send: "qux" },
      ])
    ).toBe("foo => bar\nbaz => qux");
    expect(formatAutoAnswersLines([])).toBe("");
    expect(formatAutoAnswersLines(null)).toBe("");
  });
});

describe("validateProjectForm env & auto_answers validation", () => {
  const baseValid: ProjectFormData = {
    id: "demo",
    name: "Demo",
    root: "/tmp",
    start: "echo 1",
    stop: "",
    port: 3000,
    login_url: "",
    health_url: "",
    log_file: "",
    node: "",
    java: "",
    enabled: true,
    reason: "",
    start_timeout_secs: "",
  };

  it("reports error when env_text has invalid line", () => {
    const errors = validateProjectForm({ ...baseValid, env_text: "ILLEGAL" });
    expect(errors.env_text).toBeDefined();
    expect(errors.env_text).toContain("KEY=VALUE");
  });

  it("reports error when auto_answers_text has invalid line", () => {
    const errors = validateProjectForm({
      ...baseValid,
      auto_answers_text: "NO_ARROW",
    });
    expect(errors.auto_answers_text).toBeDefined();
    expect(errors.auto_answers_text).toContain("匹配文字 => 发送内容");
  });

  it("passes when env_text and auto_answers_text are valid", () => {
    const errors = validateProjectForm({
      ...baseValid,
      env_text: "CI=1\nFOO=bar",
      auto_answers_text: "(y/N) => y",
    });
    expect(errors.env_text).toBeUndefined();
    expect(errors.auto_answers_text).toBeUndefined();
  });

  it("validates links_text format and reports error on empty url", () => {
    const errors = validateProjectForm({
      ...baseValid,
      links_text: "前台 =>",
    });
    expect(errors.links_text).toBeDefined();
    expect(errors.links_text).toContain("URL 不能为空");

    const validLinks = validateProjectForm({
      ...baseValid,
      links_text: "前台 => http://localhost:3000\n后台 => http://localhost:8080/admin\nhttp://localhost:9099",
    });
    expect(validLinks.links_text).toBeUndefined();
  });

  it("validates profiles_text format and reports error on invalid profile key", () => {
    const errors = validateProjectForm({
      ...baseValid,
      profiles_text: "invalid key!: pnpm dev",
    });
    expect(errors.profiles_text).toBeDefined();
    expect(errors.profiles_text).toContain("环境配置标识只能包含字母、数字");

    const validProfiles = validateProjectForm({
      ...baseValid,
      profiles_text: "dev: pnpm dev\nprod: 生产预览 | pnpm preview | 4173",
    });
    expect(validProfiles.profiles_text).toBeUndefined();
  });
});

describe("parseLinksLines & formatLinksLines", () => {
  it("parses and formats web links correctly", () => {
    const raw = "主页 => http://localhost:3000\nhttp://localhost:8080\n文档 => http://localhost:3000/docs";
    const { links, error } = parseLinksLines(raw);
    expect(error).toBeUndefined();
    expect(links).toHaveLength(3);
    expect(links[0]).toEqual({ title: "主页", url: "http://localhost:3000" });
    expect(links[1]).toEqual({ title: "http://localhost:8080", url: "http://localhost:8080" });
    expect(links[2]).toEqual({ title: "文档", url: "http://localhost:3000/docs" });

    const formatted = formatLinksLines(links);
    expect(formatted).toBe("主页 => http://localhost:3000\nhttp://localhost:8080\n文档 => http://localhost:3000/docs");
  });
});

describe("parseProfilesLines & formatProfilesLines", () => {
  it("parses single command, label+cmd, label+cmd+port, cmd+port profiles", () => {
    const raw = [
      "dev: pnpm dev",
      "staging: 预发环境 | pnpm dev:stage",
      "prod: 生产预览 | pnpm preview | 4173",
      "test: pnpm test | 3001",
    ].join("\n");
    const { profiles, error } = parseProfilesLines(raw);
    expect(error).toBeUndefined();
    expect(profiles.dev).toEqual({ start: "pnpm dev" });
    expect(profiles.staging).toEqual({ label: "预发环境", start: "pnpm dev:stage" });
    expect(profiles.prod).toEqual({ label: "生产预览", start: "pnpm preview", port: 4173 });
    expect(profiles.test).toEqual({ start: "pnpm test", port: 3001 });

    const formatted = formatProfilesLines(profiles);
    expect(formatted).toContain("dev: pnpm dev");
    expect(formatted).toContain("staging: 预发环境 | pnpm dev:stage");
    expect(formatted).toContain("prod: 生产预览 | pnpm preview | 4173");
    expect(formatted).toContain("test: pnpm test | 3001");
  });
});

describe("buildEditRequest with links, profiles and default_profile", () => {
  const base: EditBaseline = {
    name: "Demo",
    root: "/demo",
    start: "pnpm dev",
    start_editable: true,
    stop: "",
    stop_editable: true,
    port: 3000,
    login_url: "",
    health_url: "",
    log_file: "",
    enabled: true,
    reason: "",
    links: [{ title: "旧链接", url: "http://old" }],
    default_profile: "dev",
    profiles: { dev: { start: "pnpm dev" } },
  };

  const form: ProjectFormData = {
    id: "demo",
    name: "Demo",
    root: "/demo",
    start: "pnpm dev",
    stop: "",
    port: 3000,
    login_url: "",
    health_url: "",
    log_file: "",
    node: "",
    java: "",
    enabled: true,
    reason: "",
    start_timeout_secs: "",
    links_text: "新链接 => http://new",
    default_profile: "prod",
    profiles_text: "prod: 生产 | pnpm preview | 4173",
  };

  it("detects changes in links, default_profile, profiles", () => {
    const req = buildEditRequest(base, form);
    expect(req.default_profile).toBe("prod");
    expect(req.links).toEqual([{ title: "新链接", url: "http://new" }]);
    expect(req.profiles).toEqual({
      prod: { label: "生产", start: "pnpm preview", port: 4173 },
    });
  });
});


