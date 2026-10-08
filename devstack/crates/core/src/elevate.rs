use std::process::Command;

/// Elevate the current process via UAC.
/// Returns true if the process is already elevated.
pub fn is_elevated() -> bool {
    // Try to open the hosts file for append — if we can, we have write access
    crate::hosts::check_access()
}

/// Re-launch the current executable with admin rights via `runas` or PowerShell.
/// This is a helper for `devctl setup` and `devctl site add` when hosts needs writing.
pub fn elevate_and_run(args: &[&str]) -> crate::Result<()> {
    if is_elevated() {
        return Ok(());
    }
    // Use PowerShell Start-Process -Verb RunAs to trigger UAC prompt
    let exe = std::env::current_exe()?;
    let args_str = args.join(" ");
    let status = Command::new("powershell")
        .args([
            "-NoProfile",
            "-Command",
            &format!(
                "Start-Process -FilePath '{}' -ArgumentList '{}' -Verb RunAs -Wait",
                exe.display(),
                args_str.replace('\'', "''")
            ),
        ])
        .status()?;
    if !status.success() {
        return Err(crate::Error::Config(
            "elevation denied — hosts file not updated".into(),
        ));
    }
    Ok(())
}
