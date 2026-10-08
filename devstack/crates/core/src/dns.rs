use std::path::PathBuf;
use std::process::Command;

/// Setup macOS DNS resolution: /etc/resolver/<tld> → 127.0.0.1 (dnsmasq port 53),
/// plus a dnsmasq.conf wildcard entry.
/// This is the ONLY step needing sudo — run via `sudo devctl setup`.
pub fn setup_resolver(tld: &str, dnsmasq_conf: &PathBuf) -> crate::Result<()> {
    let resolver_dir = PathBuf::from("/etc/resolver");
    if !resolver_dir.exists() {
        std::fs::create_dir_all(&resolver_dir)?;
    }
    std::fs::write(
        resolver_dir.join(tld),
        "nameserver 127.0.0.1\nport 5353\n",
    )?;

    // dnsmasq: address=/.tld/127.0.0.1, listen on 5353 to avoid clashing with mDNS
    let conf = format!(
        "address=/.{tld}/127.0.0.1\nport=5353\nlisten-address=127.0.0.1\nbind-interfaces\n"
    );
    std::fs::write(dnsmasq_conf, conf)?;
    Ok(())
}

/// Check if resolver is configured (non-root).
pub fn check_resolver(tld: &str) -> bool {
    PathBuf::from(format!("/etc/resolver/{tld}")).exists()
}

/// Generate dnsmasq conf path under our etc dir.
pub fn dnsmasq_conf_path(paths: &crate::paths::Paths) -> PathBuf {
    paths.etc_dir().join("dnsmasq.conf")
}

/// Start dnsmasq as our managed service (non-root, port 5353).
pub fn start_dnsmasq(paths: &crate::paths::Paths) -> crate::Result<()> {
    let conf = dnsmasq_conf_path(paths);
    if !conf.exists() {
        return Err(crate::Error::DnsSetupRequired);
    }
    let dnsmasq = which::which("dnsmasq").map_err(|_| {
        crate::Error::Config("dnsmasq not found — brew install dnsmasq".into())
    })?;
    crate::service::spawn_daemon(
        dnsmasq.to_str().unwrap(),
        &[
            "--conf-file",
            conf.to_str().unwrap(),
            "--keep-in-foreground",
            "--pid-file",
            paths.pid_file("dnsmasq").to_str().unwrap(),
        ],
        &paths.pid_file("dnsmasq"),
        &paths.log_file("dnsmasq"),
    )?;
    Ok(())
}

pub fn stop_dnsmasq(paths: &crate::paths::Paths) -> crate::Result<()> {
    crate::service::stop_daemon(&paths.pid_file("dnsmasq"), 5)
}

/// Flush macOS DNS cache after changing resolver (needs sudo).
pub fn flush_dns_cache() -> crate::Result<()> {
    Command::new("dscacheutil").arg("-flushcache").output().ok();
    Command::new("killall")
        .args(["-HUP", "mDNSResponder"])
        .output()
        .ok();
    Ok(())
}
