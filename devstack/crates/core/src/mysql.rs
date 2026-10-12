use std::path::PathBuf;

/// Write my.cnf with datadir under our data dir, small footprint.
/// Windows has no unix socket — MariaDB uses a named pipe / TCP there, and all
/// paths must use forward slashes (display() would emit backslashes).
pub fn write_my_cnf(paths: &crate::paths::Paths, port: u16) -> crate::Result<()> {
    #[cfg(target_os = "windows")]
    let cnf = format!(
        r#"[mysqld]
datadir = {datadir}
port = {port}
bind-address = 127.0.0.1
pid-file = {pid}
log-error = {log}
enable-named-pipe
socket = MySQL

skip-name-resolve
innodb_buffer_pool_size = 128M
innodb_log_file_size = 64M
max_connections = 50
table_open_cache = 200
tmp_table_size = 32M
max_heap_table_size = 32M

[client]
port = {port}
"#,
        datadir = paths
            .mysql_data_dir()
            .display()
            .to_string()
            .replace('\\', "/"),
        port = port,
        pid = paths
            .pid_file("mysqld")
            .display()
            .to_string()
            .replace('\\', "/"),
        log = paths
            .log_file("mysqld")
            .display()
            .to_string()
            .replace('\\', "/"),
    );

    #[cfg(not(target_os = "windows"))]
    let cnf = format!(
        r#"[mysqld]
datadir = {datadir}
socket = {sock}
port = {port}
bind-address = 127.0.0.1
pid-file = {pid}
log-error = {log}

skip-name-resolve
innodb_buffer_pool_size = 128M
innodb_log_file_size = 64M
max_connections = 50
table_open_cache = 200
tmp_table_size = 32M
max_heap_table_size = 32M

[mysql]
socket = {sock}

[client]
socket = {sock}
port = {port}
"#,
        datadir = paths.mysql_data_dir().display(),
        sock = paths.run_dir().join("mysql.sock").display(),
        port = port,
        pid = paths.pid_file("mysqld").display(),
        log = paths.log_file("mysqld").display(),
    );

    std::fs::write(paths.my_cnf(), cnf)?;
    Ok(())
}

fn mariadb_bin(name: &str) -> Option<PathBuf> {
    #[cfg(target_os = "windows")]
    {
        // vendored: ~/.devstack/vendor/mariadb/bin/<name>.exe
        return crate::vendor::find_vendor_bin("mariadb", &format!("bin/{name}.exe"))
            .or_else(|| crate::vendor::find_vendor_bin("mariadb", &format!("bin/{name}")))
            .or_else(|| which::which(format!("{name}.exe")).ok())
            .or_else(|| which::which(name).ok());
    }
    #[cfg(target_os = "macos")]
    {
        let paths = crate::paths::Paths::detect().ok()?;
        // vendored MySQL 8.4 tarball: ~/.devstack/vendor/mysql/bin/<name>
        for pkg in ["mysql", "mariadb"] {
            if let Some(p) = crate::vendor_mac::find_vendored_bin(&paths, pkg, name) {
                return Some(p);
            }
        }
        // brew fallback for developer machines
        if let Some(prefix) = crate::httpd::brew_prefix() {
            for formula in ["mariadb", "mysql"] {
                let p = PathBuf::from(format!("{prefix}/opt/{formula}/bin/{name}"));
                if p.exists() {
                    return Some(p);
                }
                let opt = PathBuf::from(format!("{prefix}/opt"));
                if let Ok(rd) = std::fs::read_dir(&opt) {
                    for e in rd.flatten() {
                        let n = e.file_name().to_string_lossy().to_string();
                        if n.starts_with(&format!("{formula}@")) {
                            let p = e.path().join("bin").join(name);
                            if p.exists() {
                                return Some(p);
                            }
                        }
                    }
                }
            }
        }
        which::which(name).ok()
    }
    #[cfg(not(any(target_os = "windows", target_os = "macos")))]
    {
        which::which(name).ok()
    }
}

/// Public helper for provider trait — path to mariadbd binary.
pub fn mariadbd_path() -> Option<PathBuf> {
    #[cfg(target_os = "windows")]
    {
        return mariadb_bin("mariadbd").or_else(|| mariadb_bin("mysqld"));
    }
    #[cfg(not(target_os = "windows"))]
    {
        mariadb_bin("mariadbd").or_else(|| mariadb_bin("mysqld"))
    }
}

/// Initialize datadir if empty.
pub fn init_datadir(paths: &crate::paths::Paths) -> crate::Result<()> {
    let datadir = paths.mysql_data_dir();
    std::fs::create_dir_all(&datadir)?;
    if datadir.join("mysql").exists() {
        return Ok(());
    }

    #[cfg(target_os = "windows")]
    {
        let tool = mariadb_bin("mariadb-install-db")
            .or_else(|| mariadb_bin("mysql_install_db"))
            .ok_or_else(|| {
                crate::Error::Config(
                    "mariadb-install-db.exe not found — `devctl install mariadb`".into(),
                )
            })?;
        let status = std::process::Command::new(tool)
            .args([
                &format!(
                    "--datadir={}",
                    datadir.display().to_string().replace('/', "\\")
                ),
                "--password=",
            ])
            .status()?;
        if !status.success() {
            return Err(crate::Error::ServiceFailed {
                name: "mariadb-install-db".into(),
                reason: format!("exit {}", status),
            });
        }
    }

    #[cfg(target_os = "macos")]
    {
        // vendored MySQL 8.4: mysqld --initialize-insecure (root, no password)
        // brew MariaDB:           mariadb-install-db script
        if let Some(mysqld) = mariadb_bin("mysqld") {
            let status = std::process::Command::new(mysqld)
                .args([
                    &format!("--datadir={}", datadir.display()),
                    "--initialize-insecure",
                ])
                .status()?;
            if !status.success() {
                return Err(crate::Error::ServiceFailed {
                    name: "mysqld --initialize-insecure".into(),
                    reason: format!("exit {}", status),
                });
            }
            return Ok(());
        }
        let tool = mariadb_bin("mariadb-install-db")
            .or_else(|| mariadb_bin("mysql_install_db"))
            .ok_or_else(|| {
                crate::Error::Config(
                    "no MySQL/MariaDB server found — `devctl install mysql` or brew install mariadb"
                        .into(),
                )
            })?;
        let status = std::process::Command::new(tool)
            .args([
                &format!("--datadir={}", datadir.display()),
                "--auth-root-authentication-method=socket",
            ])
            .status()?;
        if !status.success() {
            return Err(crate::Error::ServiceFailed {
                name: "mariadb-install-db".into(),
                reason: format!("exit {}", status),
            });
        }
    }
    Ok(())
}

pub fn start_mysqld(paths: &crate::paths::Paths) -> crate::Result<()> {
    let bin = mariadb_bin("mariadbd")
        .or_else(|| mariadb_bin("mysqld"))
        .ok_or_else(|| {
            crate::Error::Config(
                "mysqld not found — `devctl install mysql` or brew install mariadb".into(),
            )
        })?;

    let mut args: Vec<String> = vec![format!("--defaults-file={}", paths.my_cnf().display())];
    // vendored MySQL 8.4 needs --basedir so it finds share/ + lib/plugin/
    #[cfg(target_os = "macos")]
    if let Ok(p) = crate::paths::Paths::detect() {
        let vendored = crate::vendor_mac::find_vendored_bin(&p, "mysql", "mysqld");
        if Some(&bin) == vendored.as_ref() {
            if let Some(basedir) = bin.parent().and_then(|b| b.parent()) {
                args.push(format!("--basedir={}", basedir.display()));
            }
        }
    }

    let arg_refs: Vec<&str> = args.iter().map(String::as_str).collect();
    crate::service::spawn_daemon(
        bin.to_str().unwrap(),
        &arg_refs,
        &paths.pid_file("mysqld"),
        &paths.log_file("mysqld"),
    )?;
    Ok(())
}

pub fn stop_mysqld(paths: &crate::paths::Paths) -> crate::Result<()> {
    let pid_file = paths.pid_file("mysqld");
    if !crate::service::is_running(&pid_file) {
        let _ = std::fs::remove_file(&pid_file);
        return Ok(());
    }

    #[cfg(target_os = "windows")]
    {
        // Windows: mysqladmin over TCP or named pipe. If the graceful command
        // fails or hangs, fall back to the managed PID so Stop All is reliable.
        if let Some(admin) = mariadb_bin("mariadb-admin").or_else(|| mariadb_bin("mysqladmin")) {
            let port = 3306; // TODO: read from config
            if let Ok(mut child) = std::process::Command::new(admin)
                .args([
                    &format!("--port={port}"),
                    "--host=127.0.0.1",
                    "-u",
                    "root",
                    "shutdown",
                ])
                .spawn()
            {
                let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
                loop {
                    if !crate::service::is_running(&pid_file) {
                        let _ = child.wait();
                        let _ = std::fs::remove_file(&pid_file);
                        return Ok(());
                    }
                    if child.try_wait().ok().flatten().is_some()
                        || std::time::Instant::now() >= deadline
                    {
                        let _ = child.kill();
                        let _ = child.wait();
                        break;
                    }
                    std::thread::sleep(std::time::Duration::from_millis(100));
                }
            }
        }
        return crate::service::stop_daemon(&pid_file, 10);
    }

    #[cfg(not(target_os = "windows"))]
    {
        let sock = paths.run_dir().join("mysql.sock");
        if sock.exists() {
            if let Some(admin) = mariadb_bin("mariadb-admin").or_else(|| mariadb_bin("mysqladmin"))
            {
                if let Ok(mut child) = std::process::Command::new(admin)
                    .args([
                        &format!("--socket={}", sock.display()),
                        "-u",
                        "root",
                        "shutdown",
                    ])
                    .spawn()
                {
                    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
                    loop {
                        if !crate::service::is_running(&pid_file) {
                            let _ = child.wait();
                            let _ = std::fs::remove_file(&pid_file);
                            return Ok(());
                        }
                        if child.try_wait().ok().flatten().is_some()
                            || std::time::Instant::now() >= deadline
                        {
                            let _ = child.kill();
                            let _ = child.wait();
                            break;
                        }
                        std::thread::sleep(std::time::Duration::from_millis(100));
                    }
                }
            }
        }
        crate::service::stop_daemon(&pid_file, 10)
    }
}
