//! macOS vendored binaries — download from public sources into ~/.devstack/vendor/.
//! Removes the Homebrew dependency: a user on a fresh Mac only needs `devctl setup`.
//!
//! Sources:
//!   php-X.Y   → dl.static-php.dev (static-fpm, zero deps)           ~13 MB
//!   mysql     → dev.mysql.com official tarball (arm64 / x86_64)    ~164 MB
//!   dnsmasq   → Homebrew bottle via GHCR anonymous token            ~0.3 MB
//!   httpd     → system /usr/sbin/httpd (no download needed)

use std::io::Read;
use std::path::{Path, PathBuf};

/// Progress callback phases for install — mirrors the Windows vendor::Progress
/// but lives here so macOS doesn't depend on the Windows-only module.
#[derive(Debug, Clone)]
pub enum Progress {
    Resolving,
    Downloading { got: u64, total: Option<u64> },
    Extracting,
    Done(PathBuf),
}

fn vendor_dir(paths: &crate::paths::Paths) -> PathBuf {
    paths.root.join("vendor")
}

/// Map a package spec to a vendor dir name + fetch strategy.
fn resolve(package: &str) -> crate::Result<Resolved> {
    let arch = macos_arch();
    if let Some(ver) = package.strip_prefix("php-") {
        // static-php-cli: php-{full}-fpm-macos-{arch}.tar.gz
        // We fetch the latest patch for the requested minor by listing the dir index.
        return resolve_php(ver, arch);
    }
    match package {
        "mysql" | "mariadb" => resolve_mysql(arch),
        "dnsmasq" => resolve_dnsmasq(arch),
        "mkcert" => resolve_mkcert(arch),
        _ => Err(crate::Error::Config(format!(
            "unknown package: {package} — expected php-X.Y | mysql | dnsmasq | mkcert"
        ))),
    }
}

pub fn macos_arch() -> &'static str {
    if cfg!(target_arch = "aarch64") {
        "aarch64"
    } else {
        "x86_64"
    }
}

struct Resolved {
    /// vendor subdir name
    package: String,
    url: String,
    /// sha256 hex, if we have one to verify against
    sha256: Option<String>,
}

// ── PHP (static-php-cli) ──────────────────────────────────────────────────────

fn resolve_php(minor: &str, arch: &str) -> crate::Result<Resolved> {
    let patch = latest_php_patch(minor, arch)?;
    let filename = format!("php-{patch}-fpm-macos-{arch}.tar.gz");
    Ok(Resolved {
        package: format!("php-{minor}"),
        url: format!("https://dl.static-php.dev/static-php-cli/common/{filename}"),
        sha256: None,
    })
}

/// Scrape the dl.static-php.dev index for the newest patch of `minor` on `arch`.
fn latest_php_patch(minor: &str, arch: &str) -> crate::Result<String> {
    let url = "https://dl.static-php.dev/static-php-cli/common/";
    let body = reqwest::blocking::get(url)
        .and_then(|r| r.error_for_status())
        .and_then(|r| r.text())
        .map_err(|e| crate::Error::Config(format!("fetch php index: {e}")))?;

    // entries look like: php-8.3.32-fpm-macos-aarch64.tar.gz
    let needle = format!("-fpm-macos-{arch}.tar.gz");
    let mut best: Option<String> = None;
    for line in body.split(|c: char| c == '"' || c == '>' || c == '<' || c.is_whitespace()) {
        let l = line.strip_suffix(&needle);
        if let Some(prefix) = l {
            if let Some(ver) = prefix.strip_prefix("php-") {
                // ver = "8.3.32" — must match requested minor
                if ver.starts_with(&format!("{minor}.")) || ver == minor {
                    let take = match &best {
                        None => true,
                        Some(b) => version_key(ver) > version_key(b),
                    };
                    if take {
                        best = Some(ver.to_string());
                    }
                }
            }
        }
    }
    best.ok_or_else(|| {
        crate::Error::Config(format!("no static PHP build for {minor} on macos-{arch}"))
    })
}

fn version_key(v: &str) -> (u64, u64, u64) {
    let mut it = v.split('.').map(|s| s.parse::<u64>().unwrap_or(0));
    (
        it.next().unwrap_or(0),
        it.next().unwrap_or(0),
        it.next().unwrap_or(0),
    )
}

// ── MySQL (dev.mysql.com) ────────────────────────────────────────────────────

fn resolve_mysql(arch: &str) -> crate::Result<Resolved> {
    // macOS 15 tarball covers 14.x hosts too (arm64); x86_64 build for Intel.
    // Pinned to a known-good LTS; bump to update. cdn.mysql.com/archives hosts
    // the same payload as dev.mysql.com/get but without the 403 bot-check.
    let (version, file_arch) = if arch == "aarch64" {
        ("8.4.5", "macos15-arm64")
    } else {
        ("8.4.5", "macos15-x86_64")
    };
    let filename = format!("mysql-{version}-{file_arch}.tar.gz");
    Ok(Resolved {
        package: "mysql".into(),
        url: format!("https://cdn.mysql.com/archives/mysql-8.4/{filename}"),
        sha256: None,
    })
}

// ── dnsmasq (Homebrew bottle via GHCR) ────────────────────────────────────────

fn resolve_dnsmasq(arch: &str) -> crate::Result<Resolved> {
    if arch != "aarch64" {
        return Err(crate::Error::Config(
            "dnsmasq vendored binary only available for arm64 — install via brew on Intel".into(),
        ));
    }
    // arm64_sequoia bottle — runs on any recent macOS arm64.
    // When bumping dnsmasq version, fetch the new sha from
    //   https://formulae.brew.sh/api/formula/dnsmasq.json → bottle.stable.files.arm64_sequoia.url
    Ok(Resolved {
        package: "dnsmasq".into(),
        url: "ghcr-blob:homebrew/core/dnsmasq@sha256:e64cf295c597ad396d43ad626758e842259df423ff4040e718a7ad2aede7f1f1".into(),
        sha256: None,
    })
}

// ── mkcert (FiloSottile/mkcert github release) ───────────────────────────────

fn resolve_mkcert(arch: &str) -> crate::Result<Resolved> {
    // static single-binary release — latest pinned, bump to update
    let version = "1.4.4";
    let arch_suffix = if arch == "aarch64" { "arm64" } else { "amd64" };
    let filename = format!("mkcert-v{version}-darwin-{arch_suffix}");
    Ok(Resolved {
        package: "mkcert".into(),
        url: format!(
            "https://github.com/FiloSottile/mkcert/releases/download/v{version}/{filename}"
        ),
        sha256: None,
    })
}

// ── download + extract ───────────────────────────────────────────────────────

pub fn install(
    paths: &crate::paths::Paths,
    package: &str,
    on_progress: impl Fn(Progress),
) -> crate::Result<PathBuf> {
    on_progress(Progress::Resolving);
    let r = resolve(package)?;
    let dest_dir = vendor_dir(paths).join(&r.package);
    if dest_dir.exists() {
        on_progress(Progress::Done(dest_dir.clone()));
        return Ok(dest_dir);
    }

    let tmp = paths.root.join("tmp");
    std::fs::create_dir_all(&tmp)?;

    // mkcert ships as a bare binary, not a tarball
    if r.package == "mkcert" {
        let raw = tmp.join(&r.package);
        download(&r, &raw, &on_progress)?;
        std::fs::create_dir_all(&dest_dir)?;
        let target = dest_dir.join("mkcert");
        std::fs::copy(&raw, &target)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&target, std::fs::Permissions::from_mode(0o755))?;
        }
        let _ = std::fs::remove_file(&raw);
        on_progress(Progress::Done(dest_dir.clone()));
        return Ok(dest_dir);
    }

    let tarball = tmp.join(format!("{}.tar.gz", r.package));
    download(&r, &tarball, &on_progress)?;

    if let Some(want) = &r.sha256 {
        verify_sha256(&tarball, want)?;
    }

    on_progress(Progress::Extracting);
    let staging = dest_dir.with_file_name(format!(
        "{}.staging",
        dest_dir
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or("package"),
    ));
    if staging.exists() {
        std::fs::remove_dir_all(&staging)?;
    }
    std::fs::create_dir_all(&staging)?;
    extract_tar_gz(&tarball, &staging)?;

    if dest_dir.exists() {
        std::fs::remove_dir_all(&dest_dir)?;
    }
    std::fs::rename(&staging, &dest_dir)?;
    let _ = std::fs::remove_file(&tarball);

    // normalize package layouts so downstream find_vendor_bin paths are stable
    normalize_layout(&r.package, &dest_dir)?;

    on_progress(Progress::Done(dest_dir.clone()));
    Ok(dest_dir)
}

fn download(r: &Resolved, out_path: &Path, on_progress: &impl Fn(Progress)) -> crate::Result<()> {
    if let Some(blob) = r.url.strip_prefix("ghcr-blob:") {
        return download_ghcr_blob(blob, out_path, on_progress);
    }
    let client = reqwest::blocking::Client::builder()
        .user_agent("Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/128.0 Safari/537.36")
        .build()
        .map_err(|e| crate::Error::Config(format!("http client: {e}")))?;
    let mut resp = client
        .get(&r.url)
        .send()
        .and_then(|r| r.error_for_status())
        .map_err(|e| crate::Error::Config(format!("download {}: {e}", r.url)))?;
    stream_body(&mut resp, out_path, on_progress)
}

/// GHCR blobs need an anonymous pull token, then the blob URL.
/// `spec` = "homebrew/core/dnsmasq@sha256:…"
fn download_ghcr_blob(
    spec: &str,
    out_path: &Path,
    on_progress: &impl Fn(Progress),
) -> crate::Result<()> {
    let (repo, digest) = spec
        .split_once('@')
        .ok_or_else(|| crate::Error::Config(format!("bad ghcr-blob spec: {spec}")))?;
    let token_url = format!("https://ghcr.io/token?scope=repository:{repo}:pull");
    let token: serde_json::Value = reqwest::blocking::get(&token_url)
        .and_then(|r| r.error_for_status())
        .and_then(|r| r.json())
        .map_err(|e| crate::Error::Config(format!("ghcr token: {e}")))?;
    let token = token["token"]
        .as_str()
        .ok_or_else(|| crate::Error::Config("ghcr token response missing `token`".into()))?;

    let url = format!("https://ghcr.io/v2/{repo}/blobs/{digest}");
    let client = reqwest::blocking::Client::new();
    let mut resp = client
        .get(&url)
        .header("Authorization", format!("Bearer {token}"))
        .send()
        .and_then(|r| r.error_for_status())
        .map_err(|e| crate::Error::Config(format!("ghcr blob: {e}")))?;
    stream_body(&mut resp, out_path, on_progress)
}

fn stream_body(
    resp: &mut reqwest::blocking::Response,
    out_path: &Path,
    on_progress: &impl Fn(Progress),
) -> crate::Result<()> {
    let total = resp.content_length();
    let mut out = std::fs::File::create(out_path)?;
    let mut got: u64 = 0;
    let mut buf = [0u8; 128 * 1024];
    loop {
        let n = resp.read(&mut buf)?;
        if n == 0 {
            break;
        }
        use std::io::Write;
        out.write_all(&buf[..n])?;
        got += n as u64;
        on_progress(Progress::Downloading { got, total });
    }
    Ok(())
}

fn verify_sha256(path: &Path, want: &str) -> crate::Result<()> {
    let out = std::process::Command::new("shasum")
        .args(["-a", "256"])
        .arg(path)
        .output()?;
    let got = String::from_utf8_lossy(&out.stdout)
        .split_whitespace()
        .next()
        .unwrap_or("")
        .to_string();
    if !got.eq_ignore_ascii_case(want) {
        return Err(crate::Error::Config(format!(
            "sha256 mismatch — got {got}, expected {want}"
        )));
    }
    Ok(())
}

/// Extract .tar.gz, stripping the single top-level directory every entry shares
/// (e.g. `dnsmasq/2.93/sbin/dnsmasq` → `sbin/dnsmasq`; `mysql-8.4.5-…/bin/mysqld` → `bin/mysqld`).
fn extract_tar_gz(tarball: &Path, dest_dir: &Path) -> crate::Result<()> {
    let file = std::fs::File::open(tarball)?;
    let gz = flate2::read::GzDecoder::new(file);
    let mut archive = tar::Archive::new(gz);

    // Find the prefix to strip by locating the dir that holds the payload.
    // We look for a directory containing `sbin/dnsmasq`, `bin/mysqld`, or a
    // bare `php-fpm` — and strip everything up to and including that dir's
    // parent components. Flat archives (single root-level file) strip nothing.
    let strip_prefix = detect_payload_root(tarball)?;

    for e in archive.entries()? {
        let mut e = e?;
        let path = e.path()?.to_path_buf();
        let rel = match &strip_prefix {
            Some(sp) => {
                let stripped = path.strip_prefix(sp);
                match stripped {
                    Ok(r) => r.to_path_buf(),
                    // root-level files (e.g. a bare `php-fpm` in flat archives)
                    // pass through unchanged
                    Err(_) => path.clone(),
                }
            }
            None => path.clone(),
        };
        if rel.as_os_str().is_empty() {
            continue;
        }
        let out = dest_dir.join(&rel);
        if let Some(parent) = out.parent() {
            std::fs::create_dir_all(parent)?;
        }
        // preserve executable bit
        let mode = e.header().mode().unwrap_or(0o644);
        e.unpack(&out)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let _ = std::fs::set_permissions(&out, std::fs::Permissions::from_mode(mode));
        }
    }
    Ok(())
}

/// Scan the archive for the deepest path prefix that directly contains a
/// payload marker (sbin/, bin/, or a top-level binary). Returns the prefix to
/// strip — e.g. `dnsmasq/2.93` for brew bottles, `mysql-8.4.5-macos15-arm64`
/// for MySQL tarballs, None for flat archives.
fn detect_payload_root(tarball: &Path) -> crate::Result<Option<String>> {
    let file = std::fs::File::open(tarball)?;
    let gz = flate2::read::GzDecoder::new(file);
    let mut archive = tar::Archive::new(gz);

    // payload markers: find the path *before* them and strip that.
    // entries look like: dnsmasq/2.93/sbin/dnsmasq  →  strip "dnsmasq/2.93"
    //                    mysql-x.y.z/bin/mysqld      →  strip "mysql-x.y.z"
    const MARKERS: &[&str] = &[
        "/sbin/dnsmasq",
        "/bin/mysqld",
        "/bin/mariadbd",
        "/sbin/php-fpm",
        "/bin/php-fpm",
    ];

    let mut best: Option<String> = None;
    for e in archive.entries()? {
        let e = e?;
        let path = e.path()?;
        let norm = path.to_string_lossy().replace('\\', "/");
        for m in MARKERS {
            if let Some(idx) = norm.rfind(m) {
                // candidate prefix is everything before the marker
                let cand = &norm[..idx];
                // strip it if it's the longest prefix (most specific payload root)
                let take = match &best {
                    None => true,
                    Some(b) => cand.len() > b.len(),
                };
                if take {
                    best = Some(cand.to_string());
                }
            }
        }
        // flat archive: a bare executable at root → no strip
        if best.is_none() && !norm.contains('/') {
            if let Some(fname) = norm.rsplit('/').next() {
                if fname == "php-fpm" || fname == "php" {
                    return Ok(None);
                }
            }
        }
    }
    Ok(best)
}

/// After extraction, make sure the layout matches what find_vendor_bin expects.
/// PHP ships as a bare `php-fpm` file at archive root — no dir. We keep that.
/// dnsmasq bottle has `sbin/dnsmasq` — kept as-is.
/// mysql tarball has `bin/` + `lib/` — kept as-is.
fn normalize_layout(_package: &str, _dest_dir: &Path) -> crate::Result<()> {
    Ok(())
}

// ── lookups used by start/stop code ──────────────────────────────────────────

/// Find a binary inside a vendored package dir.
/// Searches the whole tree (tarballs nest differently) but checks common spots first.
pub fn find_vendored_bin(
    paths: &crate::paths::Paths,
    package: &str,
    bin_name: &str,
) -> Option<PathBuf> {
    let root = vendor_dir(paths).join(package);
    if !root.exists() {
        return None;
    }
    // fast paths
    for rel in [
        format!("bin/{bin_name}"),
        format!("sbin/{bin_name}"),
        bin_name.to_string(),
    ] {
        let p = root.join(&rel);
        if p.exists() {
            return Some(p);
        }
    }
    // walk the tree — bounded depth 4, first match wins
    walk_for_bin(&root, bin_name, 4)
}

fn walk_for_bin(dir: &Path, name: &str, depth: u8) -> Option<PathBuf> {
    if depth == 0 {
        return None;
    }
    let rd = std::fs::read_dir(dir).ok()?;
    for e in rd.flatten() {
        let p = e.path();
        if p.is_file() && p.file_name().map(|n| n == name).unwrap_or(false) {
            return Some(p);
        }
        if p.is_dir() {
            if let Some(found) = walk_for_bin(&p, name, depth - 1) {
                return Some(found);
            }
        }
    }
    None
}

/// Is a package installed (vendor dir exists)?
pub fn is_installed(paths: &crate::paths::Paths, package: &str) -> bool {
    vendor_dir(paths).join(package).exists()
}

/// List installed vendored packages on macOS.
pub fn list_installed(paths: &crate::paths::Paths) -> Vec<String> {
    let dir = vendor_dir(paths);
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
