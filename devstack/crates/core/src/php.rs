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

/// Detect installed PHP versions. Prefers vendored static binaries
/// (~/.devstack/vendor/php-X.Y/php-fpm) so a fresh Mac needs no brew;
/// falls back to brew taps if nothing is vendored.
#[cfg(target_os = "macos")]
pub fn detect_phps(paths: &crate::paths::Paths) -> crate::Result<Vec<PhpVersion>> {
    let mut out = Vec::new();

    // vendored static PHP (static-php-cli) — preferred, no brew needed
    for name in crate::vendor_mac::list_installed(paths) {
        if let Some(ver) = name.strip_prefix("php-") {
            if let Some(fpm) = crate::vendor_mac::find_vendored_bin(paths, &name, "php-fpm") {
                out.push(PhpVersion {
                    version: ver.to_string(),
                    fpm_bin: fpm,
                    ini_dir: paths.php_ini_dir(ver),
                });
            }
        }
    }

    // Also discover legacy Homebrew PHP versions when present. Vendored builds
    // remain preferred for a matching version, but all installed versions are
    // returned so the runtime can start and display every available PHP-FPM.
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
                if fpm.exists() && !out.iter().any(|php| php.version == ver) {
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

/// Back-compat alias used by the app — same thing, clearer name now.
#[cfg(target_os = "macos")]
pub fn detect_brew_phps(paths: &crate::paths::Paths) -> crate::Result<Vec<PhpVersion>> {
    detect_phps(paths)
}

/// Find the php-fpm binary for a specific version — vendored first, then brew.
#[cfg(target_os = "macos")]
pub fn find_php_fpm(version: &str) -> Option<PathBuf> {
    let paths = crate::paths::Paths::detect().ok()?;
    if let Some(p) =
        crate::vendor_mac::find_vendored_bin(&paths, &format!("php-{version}"), "php-fpm")
    {
        return Some(p);
    }
    for prefix in ["/opt/homebrew", "/usr/local"] {
        let fpm = PathBuf::from(format!("{prefix}/opt/php@{version}/sbin/php-fpm"));
        if fpm.exists() {
            return Some(fpm);
        }
    }
    which::which(format!("php-fpm{version}")).ok()
}

/// Find the php CLI binary for a version (used for `devctl php` + future tooling).
/// static-php-cli tarball only ships `php-fpm`; fall back to the same binary —
/// `php-fpm -v` reports the CLI version fine, and sites only need fpm anyway.
#[cfg(target_os = "macos")]
pub fn find_php_cli(version: &str) -> Option<PathBuf> {
    let paths = crate::paths::Paths::detect().ok()?;
    // vendored layout may or may not ship a separate `php` binary
    crate::vendor_mac::find_vendored_bin(&paths, &format!("php-{version}"), "php")
        .or_else(|| {
            crate::vendor_mac::find_vendored_bin(&paths, &format!("php-{version}"), "php-fpm")
        })
        .or_else(|| {
            for prefix in ["/opt/homebrew", "/usr/local"] {
                let p = PathBuf::from(format!("{prefix}/opt/php@{version}/bin/php"));
                if p.exists() {
                    return Some(p);
                }
            }
            None
        })
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
        user = std::env::var("SUDO_USER")
            .ok()
            .filter(|u| u != "root" && !u.is_empty())
            .or_else(|| std::env::var("USER").ok())
            .unwrap_or_else(|| "www".into()),
        group = if cfg!(target_os = "macos") {
            "staff"
        } else {
            "www-data"
        },
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
    let fpm_bin =
        find_php_fpm(version).ok_or_else(|| crate::Error::PhpNotInstalled(version.into()))?;
    let conf = paths.etc_dir().join(format!("php-fpm-{version}.conf"));

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
    crate::service::stop_daemon(&paths.pid_file(&format!("php-fpm-{version}")), 5)
}

/// `php -v` output first line for a version — vendored or brew.
#[cfg(target_os = "macos")]
pub fn php_cli_version(version: &str) -> Option<String> {
    let bin = find_php_cli(version)?;
    Command::new(bin)
        .arg("-v")
        .output()
        .ok()
        .and_then(|o| String::from_utf8(o.stdout).ok())
        .map(|s| s.lines().next().unwrap_or("").to_string())
}

/// List vendored PHP versions: ~/.devstack/vendor/php-*/
/// Works on both platforms — on macOS these are static-php-cli builds.
pub fn detect_vendored_phps(paths: &crate::paths::Paths) -> crate::Result<Vec<String>> {
    #[cfg(target_os = "windows")]
    {
        Ok(crate::vendor::list_vendored()
            .iter()
            .filter_map(|n| n.strip_prefix("php-").map(|v| v.to_string()))
            .collect())
    }
    #[cfg(target_os = "macos")]
    {
        Ok(crate::vendor_mac::list_installed(paths)
            .iter()
            .filter_map(|n| n.strip_prefix("php-").map(|v| v.to_string()))
            .collect())
    }
    #[cfg(not(any(target_os = "windows", target_os = "macos")))]
    {
        Ok(vec![])
    }
}
