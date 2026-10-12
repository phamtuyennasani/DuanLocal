#![cfg(target_os = "macos")]

use std::process::Command;

use crate::{dns, Error, Result};

const PF_ANCHOR: &str = "/etc/pf.anchors/devstack";
const PF_ANCHOR_NAME: &str = "com.apple/devstack";
const PF_LAUNCH_DAEMON: &str = "/Library/LaunchDaemons/local.devstack.pf.plist";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AdminOutcome {
    Applied,
    AlreadyConfigured,
    Cancelled,
}

fn shell_quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\\''"))
}

fn launch_daemon_contents() -> String {
    format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0"><dict>
<key>Label</key><string>local.devstack.pf</string>
<key>ProgramArguments</key><array>
<string>/sbin/pfctl</string><string>-e</string><string>-a</string><string>{PF_ANCHOR_NAME}</string><string>-f</string><string>{PF_ANCHOR}</string>
</array>
<key>RunAtLoad</key><true/>
</dict></plist>
"#
    )
}

fn validate_redirects(redirects: &[(u16, u16)]) -> Result<()> {
    if redirects.len() > 2 {
        return Err(Error::Config("too many privileged port redirects".into()));
    }

    let mut http_seen = false;
    let mut https_seen = false;
    for &(public, bind) in redirects {
        let seen = match public {
            80 => &mut http_seen,
            443 => &mut https_seen,
            _ => {
                return Err(Error::Config(format!(
                    "unsupported privileged redirect {public} -> {bind}"
                )));
            }
        };
        if *seen || bind < 1024 || public == bind {
            return Err(Error::Config(format!(
                "unsupported privileged redirect {public} -> {bind}"
            )));
        }
        *seen = true;
    }
    Ok(())
}

/// Build the complete root-only setup script from a validated TLD and a fixed
/// redirect list. The script never reads ~/.devstack or executes user files.
///
/// Redirects are deliberately allowlisted instead of accepting arbitrary
/// (public, bind) pairs: the webview-side config can be edited by the user, so
/// the elevation path re-validates that only the two supported privileged
/// forwards (80→internal HTTP, 443→internal HTTPS) can ever reach root.
pub fn build_root_script(tld: &str, redirects: &[(u16, u16)]) -> Result<String> {
    dns::validate_tld(tld)?;
    validate_redirects(redirects)?;

    let resolver = format!("/etc/resolver/{tld}");
    let resolver_q = shell_quote(&resolver);
    let resolver_contents_q = shell_quote(&dns::resolver_contents());
    let anchor_q = shell_quote(PF_ANCHOR);
    let daemon_q = shell_quote(PF_LAUNCH_DAEMON);
    let pf_commands = if redirects.is_empty() {
        format!(
            r#"(/bin/launchctl bootout system/local.devstack.pf >/dev/null 2>&1 || true) && \
(/sbin/pfctl -a {PF_ANCHOR_NAME} -F all >/dev/null 2>&1 || true) && \
/bin/rm -f {anchor_q} {daemon_q}"#
        )
    } else {
        let anchor_contents_q = shell_quote(&dns::render_pf_anchor(redirects));
        let daemon_contents_q = shell_quote(&launch_daemon_contents());
        format!(
            r#"/usr/bin/printf '%s' {anchor_contents_q} > {anchor_q} && \
/usr/sbin/chown root:wheel {anchor_q} && /bin/chmod 0644 {anchor_q} && \
/usr/bin/printf '%s' {daemon_contents_q} > {daemon_q} && \
/usr/sbin/chown root:wheel {daemon_q} && /bin/chmod 0644 {daemon_q} && \
(/sbin/pfctl -s info | /usr/bin/grep -q 'Status: Enabled' || /sbin/pfctl -e) && \
/sbin/pfctl -a {PF_ANCHOR_NAME} -f {anchor_q} && \
(/bin/launchctl bootout system/local.devstack.pf >/dev/null 2>&1 || true) && \
/bin/launchctl bootstrap system {daemon_q}"#
        )
    };

    Ok(format!(
        r#"/bin/mkdir -p /etc/resolver /etc/pf.anchors /Library/LaunchDaemons && \
/usr/bin/printf '%s' {resolver_contents_q} > {resolver_q} && \
/usr/sbin/chown root:wheel {resolver_q} && /bin/chmod 0644 {resolver_q} && \
{pf_commands} && \
/usr/bin/dscacheutil -flushcache && /usr/bin/killall -HUP mDNSResponder"#
    ))
}

/// Check root-owned persistence inputs. A live pf status is verified by the
/// elevated command; a non-root app cannot read pf's active rules reliably.
pub fn check_pf_setup(redirects: &[(u16, u16)]) -> bool {
    if validate_redirects(redirects).is_err() {
        return false;
    }
    if redirects.is_empty() {
        return !std::path::Path::new(PF_ANCHOR).exists()
            && !std::path::Path::new(PF_LAUNCH_DAEMON).exists();
    }
    let anchor_ok = std::fs::read_to_string(PF_ANCHOR)
        .map(|contents| contents == dns::render_pf_anchor(redirects))
        .unwrap_or(false);
    let daemon_ok = std::fs::read_to_string(PF_LAUNCH_DAEMON)
        .map(|contents| contents == launch_daemon_contents())
        .unwrap_or(false);
    anchor_ok && daemon_ok
}

/// Ask macOS for administrator privileges without re-launching DevStack.
pub fn run_with_admin_dialog(tld: &str, redirects: &[(u16, u16)]) -> Result<AdminOutcome> {
    let command = build_root_script(tld, redirects)?;
    let program = r#"
on run argv
    do shell script (item 1 of argv) with administrator privileges with prompt "DevStack needs administrator access to enable local domains and public ports."
end run
"#;
    let output = Command::new("/usr/bin/osascript")
        .arg("-e")
        .arg(program)
        .arg("--")
        .arg(command)
        .output()?;
    if output.status.success() {
        return Ok(AdminOutcome::Applied);
    }
    let stderr = String::from_utf8_lossy(&output.stderr);
    if stderr.contains("-128") || stderr.to_ascii_lowercase().contains("canceled") {
        return Ok(AdminOutcome::Cancelled);
    }
    Err(Error::ElevationFailed(stderr.trim().to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn root_script_contains_only_expected_system_paths() {
        let script = build_root_script("test", &[(80, 8081), (443, 8443)]).unwrap();
        assert!(script.contains("/etc/resolver/test"));
        assert!(script.contains("/etc/pf.anchors/devstack"));
        assert!(script.contains("com.apple/devstack"));
        assert!(script.contains("/Library/LaunchDaemons/local.devstack.pf.plist"));
        assert!(!script.contains("/.devstack"));
        assert!(!script.contains("pfctl -f"));
    }

    #[test]
    fn root_script_rejects_untrusted_tld() {
        assert!(build_root_script("../test", &[(80, 8081)]).is_err());
    }

    #[test]
    fn root_script_rejects_unsupported_or_duplicate_redirects() {
        for redirects in [
            vec![(8080, 8081)],
            vec![(80, 80)],
            vec![(80, 443)],
            vec![(80, 8081), (80, 8082)],
            vec![(80, 8081), (443, 8443), (443, 9443)],
        ] {
            assert!(
                build_root_script("test", &redirects).is_err(),
                "{redirects:?}"
            );
        }
    }

    #[test]
    fn empty_redirects_remove_persisted_pf_integration() {
        let script = build_root_script("test", &[]).unwrap();
        assert!(script.contains("pfctl -a com.apple/devstack -F all"));
        assert!(script.contains("/bin/rm -f '/etc/pf.anchors/devstack'"));
    }
}
