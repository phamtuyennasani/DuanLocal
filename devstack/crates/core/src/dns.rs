use std::path::{Path, PathBuf};
use std::process::Command;

/// DNS port for DevStack's user-owned dnsmasq instance.
pub const DNS_PORT: u16 = 15353;

/// Validate a single-label local TLD before it is used in paths, configuration,
/// or an elevated macOS command.
pub fn validate_tld(tld: &str) -> crate::Result<()> {
    let valid = !tld.is_empty()
        && tld.len() <= 63
        && tld
            .bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-')
        && !tld.starts_with('-')
        && !tld.ends_with('-')
        && matches!(tld, "test" | "example" | "invalid");
    if valid {
        Ok(())
    } else {
        Err(crate::Error::Config(format!(
            "invalid local TLD {tld:?} — use a single lowercase label such as test"
        )))
    }
}

pub fn resolver_contents() -> String {
    format!("nameserver 127.0.0.1\nport {DNS_PORT}\n")
}

fn dnsmasq_contents(tld: &str) -> String {
    format!(
        "address=/.{tld}/127.0.0.1\nport={DNS_PORT}\nlisten-address=127.0.0.1\nbind-interfaces\n"
    )
}

/// Write the user-owned dnsmasq configuration. This must never run as root.
pub fn write_dnsmasq_conf(tld: &str, dnsmasq_conf: &Path) -> crate::Result<()> {
    validate_tld(tld)?;
    std::fs::write(dnsmasq_conf, dnsmasq_contents(tld))?;
    Ok(())
}

/// Check that the system resolver is DevStack's expected configuration.
pub fn check_resolver(tld: &str) -> bool {
    validate_tld(tld).is_ok()
        && std::fs::read_to_string(format!("/etc/resolver/{tld}"))
            .map(|contents| contents == resolver_contents())
            .unwrap_or(false)
}

/// Check the user-owned dnsmasq configuration without requiring root.
pub fn check_dnsmasq_conf(tld: &str, path: &Path) -> bool {
    validate_tld(tld).is_ok()
        && std::fs::read_to_string(path)
            .map(|contents| contents == dnsmasq_contents(tld))
            .unwrap_or(false)
}

/// Generate the user-owned dnsmasq conf path under our etc dir.
pub fn dnsmasq_conf_path(paths: &crate::paths::Paths) -> PathBuf {
    paths.etc_dir().join("dnsmasq.conf")
}

/// dnsmasq binary — vendored first, then common Homebrew/PATH locations.
#[cfg(target_os = "macos")]
fn dnsmasq_bin(paths: &crate::paths::Paths) -> Option<PathBuf> {
    crate::vendor_mac::find_vendored_bin(paths, "dnsmasq", "dnsmasq")
        .or_else(|| which::which("dnsmasq").ok())
        .or_else(|| {
            ["/opt/homebrew/bin/dnsmasq", "/usr/local/bin/dnsmasq"]
                .iter()
                .map(PathBuf::from)
                .find(|path| path.exists())
        })
}

#[cfg(not(target_os = "macos"))]
fn dnsmasq_bin(_paths: &crate::paths::Paths) -> Option<PathBuf> {
    None
}

/// Start dnsmasq as our managed service (non-root, on DNS_PORT).
pub fn start_dnsmasq(paths: &crate::paths::Paths) -> crate::Result<()> {
    let conf = dnsmasq_conf_path(paths);
    if !conf.exists() {
        return Err(crate::Error::DnsSetupRequired);
    }
    let dnsmasq = dnsmasq_bin(paths).ok_or_else(|| {
        crate::Error::Config("dnsmasq not found — install it from the Packages panel".into())
    })?;
    crate::service::spawn_daemon(
        dnsmasq.to_str().unwrap(),
        &[
            &format!("--conf-file={}", conf.display()),
            "--keep-in-foreground",
            &format!("--pid-file={}", paths.pid_file("dnsmasq").display()),
        ],
        &paths.pid_file("dnsmasq"),
        &paths.log_file("dnsmasq"),
    )?;
    Ok(())
}

pub fn stop_dnsmasq(paths: &crate::paths::Paths) -> crate::Result<()> {
    crate::service::stop_daemon(&paths.pid_file("dnsmasq"), 5)
}

/// Flush macOS DNS cache after changing resolver.
pub fn flush_dns_cache() -> crate::Result<()> {
    Command::new("/usr/bin/dscacheutil")
        .arg("-flushcache")
        .output()
        .ok();
    Command::new("/usr/bin/killall")
        .args(["-HUP", "mDNSResponder"])
        .output()
        .ok();
    Ok(())
}

/// Render the complete DevStack pf anchor from validated numeric port pairs.
pub fn render_pf_anchor(redirects: &[(u16, u16)]) -> String {
    redirects
        .iter()
        .map(|(public_port, bind_port)| {
            format!(
                "rdr pass on lo0 inet proto tcp from any to any port {public_port} -> 127.0.0.1 port {bind_port}\n"
            )
        })
        .collect()
}

/// Check the anchor file's exact content without invoking pfctl as a user.
pub fn check_pf_anchor(redirects: &[(u16, u16)]) -> bool {
    if redirects.is_empty() {
        return true;
    }
    std::fs::read_to_string("/etc/pf.anchors/devstack")
        .map(|contents| contents == render_pf_anchor(redirects))
        .unwrap_or(false)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tld_validation_rejects_paths_and_shell_syntax() {
        for value in [
            "", "Test", ".test", "test.dev", "-test", "test-", "../test", "test;id", "local",
        ] {
            assert!(validate_tld(value).is_err(), "{value:?} should fail");
        }
        assert!(validate_tld("example").is_ok());
    }

    #[test]
    fn pf_anchor_contains_each_redirect() {
        assert_eq!(
            render_pf_anchor(&[(80, 8081), (443, 8443)]),
            "rdr pass on lo0 inet proto tcp from any to any port 80 -> 127.0.0.1 port 8081\n\
rdr pass on lo0 inet proto tcp from any to any port 443 -> 127.0.0.1 port 8443\n"
        );
    }
}
