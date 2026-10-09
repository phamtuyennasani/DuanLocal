use std::process::{Child, Command, Stdio};
#[cfg(unix)]
use std::thread;
#[cfg(unix)]
use std::time::Duration;

/// Kind of managed service.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ServiceKind {
    Httpd,
    Mysql,
    Dnsmasq,
    PhpFpm, // versioned — handle per version
}

impl ServiceKind {
    pub fn name(&self) -> &'static str {
        match self {
            Self::Httpd => "httpd",
            Self::Mysql => "mysqld",
            Self::Dnsmasq => "dnsmasq",
            Self::PhpFpm => "php-fpm",
        }
    }
}

/// Spawn a daemon process detached from the terminal.
/// Writes child PID to `pid_file`, stdout/stderr to `log_file`.
pub fn spawn_daemon(
    program: &str,
    args: &[&str],
    pid_file: &std::path::Path,
    log_file: &std::path::Path,
) -> crate::Result<Child> {
    let log = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(log_file)?;
    let log_err = log.try_clone()?;

    let mut cmd = Command::new(program);
    cmd.args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::from(log))
        .stderr(Stdio::from(log_err));

    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        // DETACHED_PROCESS | CREATE_NO_WINDOW | CREATE_NEW_PROCESS_GROUP —
        // daemon survives our exit and never shows a console window.
        const DETACHED: u32 = 0x0000_0008;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        const NEW_GROUP: u32 = 0x0000_0200;
        cmd.creation_flags(DETACHED | CREATE_NO_WINDOW | NEW_GROUP);
    }

    let child = cmd.spawn().map_err(|e| crate::Error::ServiceFailed {
        name: program.into(),
        reason: e.to_string(),
    })?;

    let pid = child.id();
    std::fs::write(pid_file, pid.to_string())?;
    tracing::info!("spawned {program} pid={pid}");

    Ok(child)
}

/// Read PID from file and check if process is alive.
pub fn is_running(pid_file: &std::path::Path) -> bool {
    let Ok(text) = std::fs::read_to_string(pid_file) else {
        return false;
    };
    let Ok(pid) = text.trim().parse::<i32>() else {
        return false;
    };
    pid_alive(pid)
}

#[cfg(unix)]
fn pid_alive(pid: i32) -> bool {
    use nix::sys::signal::kill;
    use nix::unistd::Pid;
    kill(Pid::from_raw(pid), None).is_ok()
}

#[cfg(windows)]
fn pid_alive(pid: i32) -> bool {
    use std::os::windows::process::CommandExt;
    const CREATE_NO_WINDOW: u32 = 0x0800_0000;
    Command::new("tasklist")
        .args(["/FI", &format!("PID eq {pid}")])
        .creation_flags(CREATE_NO_WINDOW)
        .output()
        .map(|o| String::from_utf8_lossy(&o.stdout).contains(&pid.to_string()))
        .unwrap_or(false)
}

/// Send SIGTERM and wait for exit (up to timeout). SIGKILL on timeout.
#[cfg(unix)]
pub fn stop_daemon(pid_file: &std::path::Path, timeout_secs: u64) -> crate::Result<()> {
    use nix::sys::signal::{kill, Signal};
    use nix::unistd::Pid;

    let text = std::fs::read_to_string(pid_file)
        .map_err(|_| crate::Error::ServiceNotRunning(pid_file.display().to_string()))?;
    let pid: i32 = text.trim().parse().map_err(|_| {
        crate::Error::Config(format!("bad pid in {}", pid_file.display()))
    })?;
    let pid = Pid::from_raw(pid);

    kill(pid, Signal::SIGTERM).ok();

    for _ in 0..(timeout_secs * 10) {
        if kill(pid, None).is_err() {
            let _ = std::fs::remove_file(pid_file);
            return Ok(());
        }
        thread::sleep(Duration::from_millis(100));
    }

    // force kill
    kill(pid, Signal::SIGKILL).ok();
    let _ = std::fs::remove_file(pid_file);
    Ok(())
}

#[cfg(windows)]
pub fn stop_daemon(pid_file: &std::path::Path, _timeout_secs: u64) -> crate::Result<()> {
    use std::os::windows::process::CommandExt;
    const CREATE_NO_WINDOW: u32 = 0x0800_0000;
    let text = std::fs::read_to_string(pid_file)
        .map_err(|_| crate::Error::ServiceNotRunning(pid_file.display().to_string()))?;
    let pid = text.trim();
    Command::new("taskkill")
        .args(["/PID", pid, "/F"])
        .creation_flags(CREATE_NO_WINDOW)
        .output()
        .ok();
    let _ = std::fs::remove_file(pid_file);
    Ok(())
}
