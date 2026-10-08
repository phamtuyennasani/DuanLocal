use std::path::PathBuf;

/// Write my.cnf with datadir under our data dir, small footprint.
pub fn write_my_cnf(paths: &crate::paths::Paths, port: u16) -> crate::Result<()> {
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
            .or_else(|| which::which(format!("{name}.exe")).ok());
    }
    #[cfg(not(target_os = "windows"))]
    {
        let prefix = crate::httpd::brew_prefix()?;
        for formula in ["mariadb", "mysql"] {
            let p = PathBuf::from(format!("{prefix}/opt/{formula}/bin/{name}"));
            if p.exists() {
                return Some(p);
            }
            // versioned keg e.g. mariadb@10.11
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
        which::which(name).ok()
    }
}

/// Public helper for provider trait — path to mariadbd binary.
pub fn mariadbd_path() -> Option<PathBuf> {
    mariadb_bin("mariadbd").or_else(|| mariadb_bin("mysqld"))
}

/// Initialize datadir if empty.
pub fn init_datadir(paths: &crate::paths::Paths) -> crate::Result<()> {
    let datadir = paths.mysql_data_dir();
    std::fs::create_dir_all(&datadir)?;
    // already initialized?
    if datadir.join("mysql").exists() {
        return Ok(());
    }
    let tool = mariadb_bin("mariadb-install-db")
        .or_else(|| mariadb_bin("mysql_install_db"))
        .ok_or_else(|| {
            crate::Error::Config("mariadb-install-db not found — brew install mariadb".into())
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
    Ok(())
}

pub fn start_mysqld(paths: &crate::paths::Paths) -> crate::Result<()> {
    let bin = mariadb_bin("mariadbd").or_else(|| mariadb_bin("mysqld"))
        .ok_or_else(|| crate::Error::Config("mariadbd not found — brew install mariadb".into()))?;
    crate::service::spawn_daemon(
        bin.to_str().unwrap(),
        &[&format!("--defaults-file={}", paths.my_cnf().display())],
        &paths.pid_file("mysqld"),
        &paths.log_file("mysqld"),
    )?;
    Ok(())
}

pub fn stop_mysqld(paths: &crate::paths::Paths) -> crate::Result<()> {
    // graceful: mysqladmin shutdown if socket exists, else SIGTERM
    let sock = paths.run_dir().join("mysql.sock");
    if sock.exists() {
        if let Some(admin) = mariadb_bin("mariadb-admin").or_else(|| mariadb_bin("mysqladmin")) {
            std::process::Command::new(admin)
                .args([
                    &format!("--socket={}", sock.display()),
                    "-u", "root",
                    "shutdown",
                ])
                .output()
                .ok();
            let _ = std::fs::remove_file(paths.pid_file("mysqld"));
            return Ok(());
        }
    }
    crate::service::stop_daemon(&paths.pid_file("mysqld"), 10)
}
