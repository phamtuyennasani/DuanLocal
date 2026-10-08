use std::path::PathBuf;
use std::process::Command;

/// A detected PHP installation.
#[derive(Debug, Clone)]
pub struct PhpVersion {
    /// e.g. "8.3"
    pub version: String,
    /// Path to php-fpm binary
    pub fpm_bin: PathBuf,
    /// Path to php.ini dir (ours, under etc/php/<ver>/)
    pub ini_dir: PathBuf,
}

/// Detect installed PHP versions via Homebrew tap shivammathur/php.
/// Looks for /opt/homebrew/opt/php@X.Y/sbin/php-fpm and /usr/local/opt/php@X.Y/...
pub fn detect_brew_phps(paths: &crate::paths::Paths) -> crate::Result<Vec<PhpVersion>> {
    let mut out = Vec::new();

    for prefix in ["/opt/homebrew", "/usr/local"] {
        let opt = PathBuf::from(prefix).join("opt");
        if !opt.exists() {
            continue;
        }
        for entry in std::fs::read_dir(&opt)? {
            let entry = entry?;
            let name = entry.file_name().to_string_lossy().to_string();
            if let Some(ver) = name.strip_prefix("php@") {
                let fpm = entry.path().join("sbin/php-fpm");
                if fpm.exists() {
                    out.push(PhpVersion {
                        version: ver.to_string(),
                        fpm_bin: fpm,
                        ini_dir: paths.php_ini_dir(ver),
                    });
                }
            }
        }
    }
    Ok(out)
}

/// Find the php-fpm binary for a specific version.
pub fn find_php_fpm(version: &str) -> Option<PathBuf> {
    for prefix in ["/opt/homebrew", "/usr/local"] {
        let fpm = PathBuf::from(format!("{prefix}/opt/php@{version}/sbin/php-fpm"));
        if fpm.exists() {
            return Some(fpm);
        }
    }
    // also check PATH
    if let Ok(p) = which::which(format!("php-fpm{version}")) {
        return Some(p);
    }
    None
}

/// Generate php-fpm pool config for a version, ondemand for low RAM.
pub fn write_fpm_pool_conf(
    version: &str,
    paths: &crate::paths::Paths,
    profile: crate::config::RamProfile,
) -> crate::Result<()> {
    let sock = paths.php_fpm_socket(version);
    let pid = paths.pid_file(&format!("php-fpm-{version}"));
    let log = paths.log_file(&format!("php-fpm-{version}"));
    let ini_dir = paths.php_ini_dir(version);

    std::fs::create_dir_all(&ini_dir)?;

    let pm = match profile {
        crate::config::RamProfile::Low => "ondemand",
        crate::config::RamProfile::Balanced => "dynamic",
    };

    let conf = format!(
        r#"[global]
pid = {pid}
error_log = {log}
daemonize = no

[www]
listen = {sock}
listen.owner = {user}
listen.mode = 0666
user = {user}
group = {group}
pm = {pm}
pm.max_children = 5
pm.start_servers = 2
pm.min_spare_servers = 1
pm.max_spare_servers = 3
pm.process_idle_timeout = 10s
pm.max_requests = 500
php_admin_value[error_log] = {log}.php_errors
"#,
        pid = pid.display(),
        log = log.display(),
        sock = sock.display(),
        user = std::env::var("USER").unwrap_or_else(|_| "www".into()),
        group = if cfg!(target_os = "macos") { "staff" } else { "www-data" },
        pm = pm,
    );

    let fpm_conf = paths.etc_dir().join(format!("php-fpm-{version}.conf"));
    std::fs::write(fpm_conf, conf)?;
    Ok(())
}

/// Write a minimal php.ini for a version if missing.
pub fn ensure_php_ini(version: &str, paths: &crate::paths::Paths) -> crate::Result<()> {
    let dir = paths.php_ini_dir(version);
    std::fs::create_dir_all(&dir)?;
    let ini = dir.join("php.ini");
    if !ini.exists() {
        std::fs::write(
            ini,
            r#"[PHP]
memory_limit = 256M
error_reporting = E_ALL
display_errors = On
display_startup_errors = On
log_errors = On
date.timezone = UTC

[opcache]
opcache.enable = 1
opcache.enable_cli = 1
opcache.memory_consumption = 128
"#,
        )?;
    }
    Ok(())
}

/// Start php-fpm for a given version.
pub fn start_fpm(version: &str, paths: &crate::paths::Paths) -> crate::Result<()> {
    let fpm_bin = find_php_fpm(version)
        .ok_or_else(|| crate::Error::PhpNotInstalled(version.into()))?;
    let conf = paths
        .etc_dir()
        .join(format!("php-fpm-{version}.conf"));

    crate::service::spawn_daemon(
        fpm_bin.to_str().unwrap(),
        &[
            "--fpm-config",
            conf.to_str().unwrap(),
            "--pid",
            paths
                .pid_file(&format!("php-fpm-{version}"))
                .to_str()
                .unwrap(),
        ],
        &paths.pid_file(&format!("php-fpm-{version}")),
        &paths.log_file(&format!("php-fpm-{version}")),
    )?;
    Ok(())
}

pub fn stop_fpm(version: &str, paths: &crate::paths::Paths) -> crate::Result<()> {
    crate::service::stop_daemon(
        &paths.pid_file(&format!("php-fpm-{version}")),
        5,
    )
}

/// php -v for a version
pub fn php_cli_version(version: &str) -> Option<String> {
    let bin = PathBuf::from(format!("/opt/homebrew/opt/php@{version}/bin/php"));
    if !bin.exists() {
        return None;
    }
    Command::new(bin)
        .arg("-v")
        .output()
        .ok()
        .and_then(|o| String::from_utf8(o.stdout).ok())
        .map(|s| s.lines().next().unwrap_or("").to_string())
}

/// List vendored PHP versions on Windows: ~/.devstack/vendor/php-*/
pub fn detect_vendored_phps(_paths: &crate::paths::Paths) -> crate::Result<Vec<String>> {
    Ok(crate::vendor::list_vendored()
        .iter()
        .filter_map(|n| n.strip_prefix("php-").map(|v| v.to_string()))
        .collect())
}
