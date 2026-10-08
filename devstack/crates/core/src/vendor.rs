use std::path::PathBuf;

/// Vendor directory: ~/.devstack/vendor/ holds downloaded binaries on Windows.
/// On macOS this is unused — binaries come from brew.
pub fn vendor_dir(paths: &crate::paths::Paths) -> PathBuf {
    paths.root.join("vendor")
}

/// Find a binary inside a vendored package directory.
/// E.g. find_vendor_bin("php-8.3", "php-cgi.exe") →
///   ~/.devstack/vendor/php-8.3/php-cgi.exe
pub fn find_vendor_bin(package: &str, bin_rel: &str) -> Option<PathBuf> {
    let root = crate::paths::Paths::detect().ok()?.root.join("vendor").join(package);
    let p = root.join(bin_rel);
    p.exists().then_some(p)
}

/// List installed vendor packages (directories under vendor/).
pub fn list_vendored() -> Vec<String> {
    let Ok(paths) = crate::paths::Paths::detect() else { return vec![] };
    let dir = vendor_dir(&paths);
    if !dir.exists() {
        return vec![];
    }
    std::fs::read_dir(&dir)
        .map(|rd| {
            rd.flatten()
                .filter(|e| e.path().is_dir())
                .map(|e| e.file_name().to_string_lossy().to_string())
                .collect()
        })
        .unwrap_or_default()
}

/// Download URLs for Windows binaries.
pub mod urls {
    /// Apache Lounge — 64-bit httpd zip
    pub fn httpd() -> &'static str {
        // e.g. https://www.apachelounge.com/download/VS17/binaries/httpd-2.4.62-240904-win64-VS17.zip
        "https://www.apachelounge.com/download/VS17/binaries/httpd-2.4.62-240904-win64-VS17.zip"
    }
    /// PHP NTS zip from windows.php.net
    pub fn php(version: &str) -> String {
        // windows.php.net/downloads/releases/php-8.3.x-nts-Win32-vs16-x64.zip
        // Caller must resolve exact patch version — we try common patterns.
        format!("https://windows.php.net/downloads/releases/php-{version}-nts-Win32-vs16-x64.zip")
    }
    /// MariaDB zip
    pub fn mariadb() -> &'static str {
        "https://archive.mariadb.org/mariadb-11.4.2/winx64-packages/mariadb-11.4.2-winx64.zip"
    }
}
