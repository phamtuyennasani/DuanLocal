//! mkcert-based HTTPS for per-site SSL.
//! CA lives in ~/.devstack/certs/ca/, each site gets a cert under certs/<domain>/.
//! Requires `devctl install mkcert` (vendored static binary from github releases).

use std::path::PathBuf;

/// mkcert binary — vendored first, then PATH (brew).
fn mkcert_bin(paths: &crate::paths::Paths) -> Option<PathBuf> {
    crate::vendor_mac::find_vendored_bin(paths, "mkcert", "mkcert")
        .or_else(|| which::which("mkcert").ok())
}

/// Is mkcert available?
pub fn is_available(paths: &crate::paths::Paths) -> bool {
    mkcert_bin(paths).is_some()
}

/// Ensure the local CA exists and is trusted (mkcert -install).
/// Called once during `devctl setup` — mkcert asks for sudo to add the CA
/// to the system keychain on macOS.
pub fn ensure_ca(paths: &crate::paths::Paths) -> crate::Result<()> {
    let ca_dir = paths.certs_dir().join("ca");
    if ca_dir.join("rootCA.pem").exists() {
        return Ok(());
    }
    let bin = mkcert_bin(paths)
        .ok_or_else(|| crate::Error::Config("mkcert not found — `devctl install mkcert`".into()))?;

    std::fs::create_dir_all(&ca_dir)?;
    let status = std::process::Command::new(bin)
        .env("CAROOT", &ca_dir)
        .arg("-install")
        .status()?;
    if !status.success() {
        return Err(crate::Error::ServiceFailed {
            name: "mkcert -install".into(),
            reason: format!("exit {status}"),
        });
    }
    Ok(())
}

/// Generate a cert for a site's domain (and wildcard alt names).
/// Writes <certs_dir>/<domain>/cert.pem + key.pem — same paths the vhost
/// template expects.
pub fn generate_site_cert(paths: &crate::paths::Paths, domain: &str) -> crate::Result<()> {
    let bin = mkcert_bin(paths)
        .ok_or_else(|| crate::Error::Config("mkcert not found — `devctl install mkcert`".into()))?;
    let ca_dir = paths.certs_dir().join("ca");
    let out_dir = paths.certs_dir().join(domain);
    std::fs::create_dir_all(&out_dir)?;

    let cert = out_dir.join("cert.pem");
    let key = out_dir.join("key.pem");
    if cert.exists() && key.exists() {
        return Ok(());
    }

    let status = std::process::Command::new(bin)
        .env("CAROOT", &ca_dir)
        .arg("-cert-file")
        .arg(&cert)
        .arg("-key-file")
        .arg(&key)
        .arg(domain)
        .arg(format!("*.{domain}"))
        .status()?;
    if !status.success() {
        return Err(crate::Error::ServiceFailed {
            name: format!("mkcert {domain}"),
            reason: format!("exit {status}"),
        });
    }
    Ok(())
}

/// Remove a site's cert on remove_site.
pub fn remove_site_cert(paths: &crate::paths::Paths, domain: &str) {
    let _ = std::fs::remove_dir_all(paths.certs_dir().join(domain));
}
