//! Facts that come from the machine: live pid, localhost port, login HTTP code.

use std::net::{SocketAddr, TcpStream};
use std::process::{Command, Stdio};
use std::time::Duration;

/// Runtime checks. The binary uses [`SystemProbe`]; tests pass a fake.
pub trait Probe {
    fn pid_alive(&self, pid: u32) -> bool;
    fn port_open(&self, port: u16) -> bool;
    fn login_http_code(&self, url: &str) -> Option<u16>;
}

#[derive(Debug, Default)]
pub struct SystemProbe;

impl Probe for SystemProbe {
    fn pid_alive(&self, pid: u32) -> bool {
        // `kill -0 0` signals the process group and can succeed. Never treat 0 as live.
        if pid == 0 {
            return false;
        }
        Command::new("kill")
            .args(["-0", &pid.to_string()])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .map(|status| status.success())
            .unwrap_or(false)
    }

    fn port_open(&self, port: u16) -> bool {
        let addr = SocketAddr::from(([127, 0, 0, 1], port));
        TcpStream::connect_timeout(&addr, Duration::from_millis(400)).is_ok()
    }

    fn login_http_code(&self, url: &str) -> Option<u16> {
        // stderr is discarded so a probe failure cannot echo the URL or curl diagnostics.
        let output = Command::new("curl")
            .args([
                "-sS",
                "-o",
                "/dev/null",
                "-w",
                "%{http_code}",
                "--connect-timeout",
                "2",
                "--max-time",
                "5",
                url,
            ])
            .stderr(Stdio::null())
            .output()
            .ok()?;
        interpret_http_code(&String::from_utf8_lossy(&output.stdout))
    }
}

/// Parse curl's `%{http_code}`. `000` and non-numeric text mean "no response".
pub fn interpret_http_code(raw: &str) -> Option<u16> {
    let code = raw.trim().parse::<u16>().ok()?;
    if code == 0 {
        None
    } else {
        Some(code)
    }
}

pub fn http_code_ok(code: Option<u16>) -> bool {
    matches!(code, Some(200) | Some(301) | Some(302))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::TcpListener;

    #[test]
    fn http_code_parsing_and_ok_policy() {
        assert_eq!(interpret_http_code("200"), Some(200));
        assert_eq!(interpret_http_code(" 302 \n"), Some(302));
        assert_eq!(interpret_http_code("000"), None);
        assert_eq!(interpret_http_code(""), None);
        assert_eq!(interpret_http_code("abc"), None);
        assert_eq!(interpret_http_code("0"), None);

        assert!(http_code_ok(Some(200)));
        assert!(http_code_ok(Some(301)));
        assert!(http_code_ok(Some(302)));
        assert!(!http_code_ok(Some(404)));
        assert!(!http_code_ok(Some(500)));
        assert!(!http_code_ok(None));
    }

    #[test]
    fn system_probe_pid_and_port_without_tomcat() {
        let probe = SystemProbe;
        assert!(probe.pid_alive(std::process::id()));
        assert!(!probe.pid_alive(0));

        // A reaped child is not alive. Avoid `u32::MAX`: `kill` treats it as pid -1.
        let mut child = std::process::Command::new("true").spawn().unwrap();
        let dead = child.id();
        child.wait().unwrap();
        assert!(!probe.pid_alive(dead));

        let listener = TcpListener::bind(("127.0.0.1", 0)).unwrap();
        let port = listener.local_addr().unwrap().port();
        assert_ne!(port, 8080);
        assert!(probe.port_open(port));
        drop(listener);

        let closed = TcpListener::bind(("127.0.0.1", 0)).unwrap();
        let closed_port = closed.local_addr().unwrap().port();
        drop(closed);
        assert!(!probe.port_open(closed_port));
    }
}
