// Windows implementations — php-cgi over TCP instead of php-fpm unix socket.

use std::path::PathBuf;

/// Locate vendored PHP CGI binary: ~/.devstack/vendor/php-{ver}/php-cgi.exe
pub fn find_php_cgi(version: &str) -> Option<PathBuf> {
    crate::vendor::find_vendor_bin(&format!("php-{version}"), "php-cgi.exe")
}

/// Deterministic TCP port for a PHP version: 9000 + major*100 + minor.
/// "8.3" → 9083, "8.2" → 9082.
pub fn php_cgi_port(version: &str) -> u16 {
    crate::provider::php_cgi_port(version)
}

/// Write a php.ini for a version under etc/php/{ver}/php.ini
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
extension_dir = "ext"

[opcache]
opcache.enable = 1
opcache.enable_cli = 1
opcache.memory_consumption = 128
"#,
        )?;
    }
    Ok(())
}

/// Spawn php-cgi.exe listening on its TCP port.
/// php-cgi -b 127.0.0.1:PORT -c <ini_dir>
/// Sets PHP_FCGI_CHILDREN=0 (single process, ondemand-ish) and
/// PHP_FCGI_MAX_REQUESTS=500.
pub fn start_php_cgi(version: &str, paths: &crate::paths::Paths) -> crate::Result<()> {
    let bin = find_php_cgi(version)
        .ok_or_else(|| crate::Error::PhpNotInstalled(version.into()))?;
    let port = php_cgi_port(version);
    let ini_dir = paths.php_ini_dir(version);

    let mut cmd = std::process::Command::new(&bin);
    cmd.args([
        "-b",
        &format!("127.0.0.1:{port}"),
        "-c",
        ini_dir.to_str().unwrap(),
    ])
    .env("PHP_FCGI_CHILDREN", "0")
    .env("PHP_FCGI_MAX_REQUESTS", "500")
    .env("PHPRC", ini_dir.as_os_str());

    // redirect output to log
    let log_file = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(paths.log_file(&format!("php-cgi-{version}")))?;
    cmd.stdout(log_file.try_clone()?).stderr(log_file);

    let child = cmd.spawn().map_err(|e| crate::Error::ServiceFailed {
        name: format!("php-cgi-{version}"),
        reason: e.to_string(),
    })?;

    std::fs::write(
        paths.pid_file(&format!("php-cgi-{version}")),
        child.id().to_string(),
    )?;
    tracing::info!("php-cgi {version} on 127.0.0.1:{port} pid={}", child.id());
    Ok(())
}

pub fn stop_php_cgi(version: &str, paths: &crate::paths::Paths) -> crate::Result<()> {
    crate::service::stop_daemon(&paths.pid_file(&format!("php-cgi-{version}")), 5)
}
