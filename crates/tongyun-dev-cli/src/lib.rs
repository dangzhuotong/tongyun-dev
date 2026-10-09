//! Local project runner core.
//!
//! The `tongyun-dev` binary is a thin CLI over this library so a later UI shell
//! can reuse config loading, path expansion, probes, and the status JSON.

mod config;
mod ops;
mod probe;

#[cfg(test)]
mod testutil;

pub use config::{
    apply_env_overrides, load_config_file, parse_config, resolve_config_path, resolve_project,
    FileConfig, Project, ResolvedProject,
};
pub use ops::{
    logs_project, read_pid, start_project, status_of, stop_project, ListEntry, LoginProbe,
    StatusJson, StopOptions,
};
pub use probe::{http_code_ok, interpret_http_code, Probe, SystemProbe};
