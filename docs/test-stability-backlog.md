# tongyun-dev · 测试稳定性与待稳定用例清单 (Test Stability Backlog)

> 本文档由任务 `#1153023971825462563` 产出，承接前序 PR #36（`http_post_input_endpoint_and_security` 稳定化），对 `tongyun-dev` 仓库内的全量自动化测试（CLI 进程集成测试、Core 单元与状态机测试、Desktop 前端用例）进行全面扫描，梳理出 Flaky（偶发失败）、Skip（平台跳过 / 条件绕过）及带 Retry / 轮询竞态的测试清单，并给出具体根因与稳定化加固建议。

---

## 一、总览统计

| 序号 | 测试用例 / 模块 | 涉及文件路径 | 类别 | 现象与风险级别 |
| :--- | :--- | :--- | :--- | :--- |
| **01** | `delayed_listen_transitions_from_starting_to_ready` | `crates/tongyun-dev-cli/tests/lifecycle_integration.rs:672` | **Flaky / Timing Race** | **高**：Windows 环境实测直接失败；Python3 缺失时静默跳过 |
| **02** | `agent_contract.rs` 全量集成测试集 (共 15 个用例) | `crates/tongyun-dev-cli/tests/agent_contract.rs:1-696` | **Platform Skip** | **高**：整文件受 `#![cfg(unix)]` 门禁，Windows 下测试量为 0，Agent 接口契约缺乏跨平台保障 |
| **03** | PTY / 信号处理相关用例 (共 9 个用例) | `crates/tongyun-dev-cli/tests/lifecycle_integration.rs` | **Platform Skip** | **中**：受 `#[cfg(unix)]` 门禁，Windows 平台跳过率达 47% (9/19) |
| **04** | `free_port()` 瞬时释放引起的 TOCTOU 端口竞态 | `crates/tongyun-dev-cli/tests/lifecycle_integration.rs:22` | **Flaky / Port Race** | **中**：并发多线程测试下极易被 OS 端口分配池重分造成 `AddrInUse` 偶发红 |
| **05** | `group_restart_waits_and_skips_downstream_when_dependency_exits` | `crates/tongyun-dev-cli/tests/lifecycle_integration.rs:1592` | **Flaky / Hardcoded Port** | **中**：硬编码 8080 与 5173 常用端口；硬编码 `sh -c` 在无 Bash 的 Windows 上中断 |
| **06** | `fast_exit_transitions_to_failed_within_four_seconds` | `crates/tongyun-dev-cli/tests/lifecycle_integration.rs:620` | **Flaky / Timing Threshold** | **低**：4 秒轮询上限在重载 CI 或 Windows 杀毒扫描进程启动时偶发超时 |
| **07** | `pty_true_outputs_tty_and_cleans_up_on_stop` | `crates/tongyun-dev-cli/tests/lifecycle_integration.rs:1166` | **Platform Gap** | **低**：控制终端检查仅对 Linux 生效（`#[cfg(target_os = "linux")]`），macOS 下静默跳过断言 |

---

## 二、详细用例剖析与加固方案

### 01. `delayed_listen_transitions_from_starting_to_ready`
- **文件路径**: `crates/tongyun-dev-cli/tests/lifecycle_integration.rs:672-744`
- **类别**: Flaky (时序断言竞态) + Silent Skip (环境缺失静默跳过)
- **现象描述**:
  1. **断言即时崩溃**: 在 Windows 本地环境直接复现失败：
     ```text
     thread 'delayed_listen_transitions_from_starting_to_ready' panicked at crates/tongyun-dev-cli/tests/lifecycle_integration.rs:727:5:
     assertion `left == right` failed
       left: Ready
      right: Starting
     ```
  2. **根因剖析**:
     - 在 Unix 分支，测试使用 `sleep 2; exec python3 tests/fixtures/listen.py {port}`，保证了进程启动后有明确的 2 秒静默期，初次查询状态必然处于 `Starting`。
     - 但在 Windows 分支，测试调用的是：
       ```rust
       ("cmd", "/C", format!("python3 -m http.server {port} --bind 127.0.0.1"))
       ```
       这里完全没有加入延迟启动机制！当调用 `loaded.start_detached("py_srv")` 时，Python HTTP 服务启动极快并立即占用了端口。随后的状态检查获取到的生命周期状态已经是 `LifeState::Ready`，导致首行断言 `assert_eq!(st.state, LifeState::Starting)` 必然偶发或固定崩溃。
     3. **静默跳过缺陷**:
        测试在开头使用 `Command::new("python3").arg("--version")` 检查。在许多 Windows 机器上，Python 可执行文件名为 `python.exe` 而非 `python3.exe`。若检测失败，测试仅向 `eprintln!` 打印一段提示后直接 `return`，在 `cargo test` 报告中被当作 `ok` 统计，掩盖了未执行事实。
- **加固建议**:
  - 弃用 Windows 下的 `python3 -m http.server`（此脚本在 macOS 15+ 也会因为 `socket.getfqdn` 发生卡顿，故仓库此前专门引入了 `tests/fixtures/listen.py`）。
  - 为 `listen.py` 增加启动延迟命令行参数（例如 `python listen.py <port> --delay-secs 2`），无论 Unix 还是 Windows 统一复用同一个 Python 脚本。
  - 在环境探测中兼容 `python` 与 `python3`；若均不可用，应显式使用标准条件或说明。

---

### 02. `agent_contract.rs` 全量集成测试集 (15 个用例)
- **文件路径**: `crates/tongyun-dev-cli/tests/agent_contract.rs:1-696`
- **类别**: Platform Skip (硬编码 Unix 门禁导致平台全量跳过)
- **涵盖测试清单**:
  1. `failed_script_and_unknown_project_are_structured_and_nonzero`
  2. `missing_config_and_missing_logs_have_predictable_results`
  3. `wait_is_bounded_read_only_and_start_timeout_keeps_service_tracked`
  4. `ready_start_wait_restart_and_stop_return_consistent_json`
  5. `conflicting_flags_and_zero_timeout_are_usage_errors`
  6. `automatic_installer_is_cancelled_when_start_wait_times_out`
  7. `foreign_port_blocks_start_without_creating_a_process`
  8. `list_json_with_groups_contract`
  9. `group_cli_resolves_existing_label_and_suggests_group_add`
  10. `status_without_id_continues_past_bad_log_dir`
  11. `status_json_single_object_has_stable_shape`
  12. `status_json_reports_probe_url_and_unknown_project_error`
  13. `group_add_rejects_invalid_keys_and_keeps_config_untouched`
  14. `group_command_validates_key_and_targets_existing_or_new_groups`
  15. `group_start_skips_downstream_without_retrying_failed_dependency`
- **现象描述**:
  - 文件第 2 行使用了整文件级 `#![cfg(unix)]` 属性宏。
  - 在 Windows 环境下执行 `cargo test -p tongyun-dev-cli --test agent_contract`，输出显示：
    ```text
    running 0 tests
    test result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s
    ```
  - 这意味着面向 AI Agent 的核心接口契约（JSON 格式响应、状态码转换、超时保护、分组管理等）在 Windows 平台毫无自动化测试覆盖。
- **根因剖析**:
  - `Fixture::new` 中硬编码了 Unix shell (`sh -c`、`exec sleep 30` 等)。
  - `Fixture::new` 中的端口获取使用 `TcpListener::bind("127.0.0.1:0")` 绑定后即刻 drop，存在并发执行时的端口争抢风险。
- **加固建议**:
  - 移除文件顶部的 `#![cfg(unix)]`，引入跨平台的命令派发辅助工具函数（如 `test_shell_cmd()`，在 Unix 返回 `sh -c`，在 Windows 返回 `cmd /C` 或调用无外部依赖的内置测试二进制 / Python 探测脚本）。
  - 使 JSON 结构与 Agent 契约验证成为全平台通用的基线测试。

---

### 03. PTY 与终端信号测试集 (9 个用例)
- **文件路径**: `crates/tongyun-dev-cli/tests/lifecycle_integration.rs`
- **类别**: Platform Skip
- **涵盖测试清单**:
  1. `process_group_stop_cleans_children_and_markers` (L462)
  2. `trap_term_killed_after_grace_period` (L529)
  3. `pty_stdin_fifo_and_env_lifecycle` (L815)
  4. `http_post_input_endpoint_and_security` (L897)
  5. `auto_answers_automatically_answers_prompt` (L1025)
  6. `pty_true_outputs_tty_and_cleans_up_on_stop` (L1098)
  7. `pty_password_prompt_does_not_echo_secret_into_log` (L1242)
  8. `test_send_input_secret_pass_shows_placeholder_not_plaintext` (L1312)
  9. `test_send_input_limit_4096_bytes_error` (L1377)
  10. `test_auto_answers_skips_echo_lines_once_false` (L1427)
- **现象描述**:
  - 上述 10 个测试用例均标注了 `#[cfg(unix)]`。在非 Unix 系统中被直接排除在编译单元之外，`lifecycle_integration` 在 Windows 仅运行 10 个用例（占总数 19 个的 52%）。
- **根因剖析**:
  - 该批测试重度依赖 POSIX 特有机制：`mkfifo` 命名管道、`/dev/pts` 虚拟终端、`SIGTERM` / `SIGKILL` 信号组传递，以及 `/proc/<pid>/stat` 进程树检测。
- **加固建议**:
  - 对于与 PTY 机制无强绑定的 HTTP API 测试（如 `test_send_input_limit_4096_bytes_error` 4096 字节超限校验），应将 HTTP 层的错误码与报文校验拆出为平台无关测试。
  - 对于交互式输入，后续可基于 Windows ConPTY 或无管道回显的 stdin 模拟提供 Windows 对应适配。

---

### 04. `free_port()` 瞬时释放导致的 TOCTOU 端口竞态
- **文件路径**: `crates/tongyun-dev-cli/tests/lifecycle_integration.rs:22-25` 及多处调用点
- **类别**: Flaky / Port Race (Time-of-check to time-of-use)
- **影响用例**:
  - `status_follows_pid_file_port_and_log` (L54)
  - `web_console_lists_adds_and_removes_through_core` (L118)
  - `fast_exit_transitions_to_failed_within_four_seconds` (L620)
  - `repeated_start_keeps_pid_and_restart_replaces_it` (L746)
  - `foreign_listener_blocks_restart_without_running_start_command` (L791)
- **现象描述**:
  - 在并行运行多个集成测试（特别是多测试套件多线程并发）时，测试偶发报出端口已被占用（`AddrInUse`）或意外检测到 `port_conflict: true`。
- **根因剖析**:
  - `free_port()` 的实现为：
    ```rust
    fn free_port() -> u16 {
        let l = TcpListener::bind("127.0.0.1:0").unwrap();
        l.local_addr().unwrap().port()
    }
    ```
    绑定后获取端口并随即 `drop(l)`。在当前测试进程实际启动子服务之前，操作系统动态端口池极可能在微秒级别将该端口重新分配给其他并发线程或外部进程。
- **加固建议**:
  - 参考此前 PR #36 中引入的 `free_service_port(exclude)` 方案，在专属用户态端口段（如 24000..32000）内采用原子自增步进 + 短暂探测分配；或者在启动被测服务前保持 Listener 开启，仅在交付进程前释放。

---

### 05. `group_restart_waits_and_skips_downstream_when_dependency_exits`
- **文件路径**: `crates/tongyun-dev-cli/tests/lifecycle_integration.rs:1592-1649`
- **类别**: Flaky / Hardcoded Host Port Collision
- **现象描述**:
  - 测试中写死了两个通用端口：
    ```toml
    [projects.demo_api]
    port = 8080

    [projects.demo_web]
    port = 5173
    ```
  - `8080` 是最常见的 HTTP 代理 / 容器端口，`5173` 是 Vite 默认前端端口。只要开发者本机或 CI 宿主机后台运行着相关服务，该测试就会因端口冲突导致意外结果。
  - 此外，配置项内直接书写 `start = ["sh", "-c", "exit 1"]`，在未安装 Git Bash 的 Windows 宿主机上无法拉起子进程。
- **加固建议**:
  - 使用动态分配的测试隔离端口替代 8080 和 5173。
  - 脚本执行使用 `cfg!(windows)` 选择 `cmd /C`。

---

### 06. `fast_exit_transitions_to_failed_within_four_seconds`
- **文件路径**: `crates/tongyun-dev-cli/tests/lifecycle_integration.rs:620-670`
- **类别**: Flaky / Timing Threshold
- **现象描述**:
  - 测试启动一个立即退出的子进程，并在 4 秒内每 200ms 轮询一次状态。若 4 秒内未变为 `Failed`，测试触发 panic：`must transition to Failed within 4s`。
- **根因剖析**:
  - 在虚拟机、重载 CI 机器或启用了实时防病毒检测的 Windows 机器上，`cmd.exe` 进程拉起、创建日志管道到内核回写进程退出码的整套开销可能偶尔逼近或超过 4 秒。
- **加固建议**:
  - 将轮询最大阈值放宽至 8~10 秒（因只要状态就绪就会立即 `break`，放宽阈值不会增加测试正常跑通的时间）。

---

### 07. `pty_true_outputs_tty_and_cleans_up_on_stop`
- **文件路径**: `crates/tongyun-dev-cli/tests/lifecycle_integration.rs:1166-1177`
- **类别**: Platform Gap (断言静默规避)
- **现象描述**:
  - 用例中的核心断言——检查进程是否真正拥有控制终端——被限定在 `#[cfg(target_os = "linux")]`：
    ```rust
    #[cfg(target_os = "linux")]
    {
        let child = descendant_pids(pid).into_iter().next().unwrap();
        let tty_nr = controlling_tty_nr(child).unwrap_or(0);
        assert!(tty_nr > 0, "child {child} must have a controlling tty");
    }
    ```
  - macOS 运行时此段校验直接被跳过，导致 macOS 下对控制终端的核心有效性未被真正断言。
- **根因剖析**:
  - `controlling_tty_nr` 依赖读取 Linux 特有的 `/proc/<pid>/stat`。
- **加固建议**:
  - 为 macOS 引入基于 `tcgetpgrp` 或 `isatty` 的辅助探测，消除 macOS 上的测试盲区。

---

## 三、治理优先级建议

1. **P1 (立即修复)**:
   - 修复 `delayed_listen_transitions_from_starting_to_ready`：消除 Windows 环境下的必现/高发断言红灯，统一复用 `listen.py` 并增加启动延时机制。
2. **P2 (测试环境与并发健壮性)**:
   - 治理 `lifecycle_integration.rs` 中的端口竞态与硬编码端口（8080/5173 改为隔离端口；`free_port()` 引入端口池或保留释放逻辑）。
   - 将 `fast_exit` 的超时判定上限放宽至 8 秒，预防 CI 偶发超时。
3. **P3 (跨平台契约补齐)**:
   - 改造 `agent_contract.rs`，消除 `#![cfg(unix)]` 门禁，让 Agent JSON 自动化契约在全平台持续受护。
