use std::path::PathBuf;

/// Platform-abstracted operations.
#[cfg(target_os = "macos")]
pub type Provider = MacProvider;
#[cfg(target_os = "windows")]
pub type Provider = WinProvider;

pub trait PlatformProvider {
    /// Where Apache httpd binary lives.
    fn httpd_bin(&self) -> Option<PathBuf>;
    /// Where PHP binaries live for a version.
    fn php_fpm_bin(&self, version: &str) -> Option<PathBuf>;
    fn php_cgi_bin(&self, version: &str) -> Option<PathBuf>;
    /// Where mariadbd lives.
    fn mariadbd_bin(&self) -> Option<PathBuf>;

    /// Write the wildcard-DNS or hosts-file plumbing.
    /// May require elevation — caller decides.
    fn setup_dns(&self, tld: &str, config_path: &PathBuf) -> crate::Result<()>;
    /// Check whether DNS routing is in place.
    fn check_dns(&self, tld: &str) -> bool;

    /// Ports used to talk to php-fpm/cgi for a version.
    /// macOS: unix socket path. Windows: TCP port number.
    fn php_upstream(&self, version: &str, paths: &crate::paths::Paths) -> PhpUpstream;
}

pub enum PhpUpstream {
    /// Unix socket — Apache SetHandler "proxy:unix:…|fcgi://localhost"
    Socket(PathBuf),
    /// TCP loopback — Apache SetHandler "proxy:fcgi://127.0.0.1:PORT"
    Tcp(u16),
}

// ── macOS ────────────────────────────────────────────────────────────────────

#[cfg(target_os = "macos")]
#[derive(Default)]
pub struct MacProvider;

#[cfg(target_os = "macos")]
impl PlatformProvider for MacProvider {
    fn httpd_bin(&self) -> Option<PathBuf> {
        crate::httpd::httpd_bin()
    }
    fn php_fpm_bin(&self, version: &str) -> Option<PathBuf> {
        crate::php::find_php_fpm(version)
    }
    fn php_cgi_bin(&self, _version: &str) -> Option<PathBuf> {
        None // unused on macOS
    }
    fn mariadbd_bin(&self) -> Option<PathBuf> {
        crate::mysql::mariadbd_path()
    }
    fn setup_dns(&self, tld: &str, config_path: &PathBuf) -> crate::Result<()> {
        crate::dns::setup_resolver(tld, config_path)
    }
    fn check_dns(&self, tld: &str) -> bool {
        crate::dns::check_resolver(tld)
    }
    fn php_upstream(&self, version: &str, paths: &crate::paths::Paths) -> PhpUpstream {
        PhpUpstream::Socket(paths.php_fpm_socket(version))
    }
}

// ── Windows ──────────────────────────────────────────────────────────────────

#[cfg(target_os = "windows")]
#[derive(Default)]
pub struct WinProvider;

#[cfg(target_os = "windows")]
impl PlatformProvider for WinProvider {
    fn httpd_bin(&self) -> Option<PathBuf> {
        // ~/.devstack/vendor/httpd/bin/httpd.exe
        crate::vendor::find_vendor_bin("httpd", "bin/httpd.exe")
    }
    fn php_fpm_bin(&self, _version: &str) -> Option<PathBuf> {
        None // php-fpm doesn't exist on Windows
    }
    fn php_cgi_bin(&self, version: &str) -> Option<PathBuf> {
        // ~/.devstack/vendor/php-8.3/php-cgi.exe
        crate::vendor::find_vendor_bin(&format!("php-{version}"), "php-cgi.exe")
    }
    fn mariadbd_bin(&self) -> Option<PathBuf> {
        // ~/.devstack/vendor/mariadb/bin/mariadbd.exe
        crate::vendor::find_vendor_bin("mariadb", "bin/mariadbd.exe")
    }
    fn setup_dns(&self, tld: &str, _config_path: &PathBuf) -> crate::Result<()> {
        // Windows uses hosts file — handled separately via hosts module
        let _ = tld;
        Ok(()) // no-op; hosts entries written per-site by site add
    }
    fn check_dns(&self, _tld: &str) -> bool {
        // Check if hosts file is writable
        crate::hosts::check_access()
    }
    fn php_upstream(&self, version: &str, _paths: &crate::paths::Paths) -> PhpUpstream {
        // 9000 + (major*100 + minor) — 8.3→9083, 7.4→9074
        let port = php_cgi_port(version);
        PhpUpstream::Tcp(port)
    }
}

/// Deterministic port for php-cgi of a version: 9000 + major*100 + minor.
/// "8.3" → 9083, "8.4" → 9084, "7.4" → 9074.
pub fn php_cgi_port(version: &str) -> u16 {
    let parts: Vec<u16> = version
        .split('.')
        .filter_map(|s| s.parse().ok())
        .collect();
    match parts.as_slice() {
        [major, minor, ..] => 9000 + major * 100 + minor,
        [major] => 9000 + major * 100,
        _ => 9000,
    }
}
