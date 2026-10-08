use std::path::PathBuf;
use std::process::Command;

/// Windows hosts file management.
/// Path: %SystemRoot%\System32\drivers\etc\hosts
/// Writing requires admin — we check once and surface errors clearly.

pub fn hosts_path() -> PathBuf {
    let system_root = std::env::var("SystemRoot").unwrap_or_else(|_| "C:\\Windows".into());
    PathBuf::from(format!("{system_root}\\System32\\drivers\\etc\\hosts"))
}

const MARKER_START: &str = "# devstack start";
const MARKER_END: &str = "# devstack end";

/// Check whether we can write to hosts (i.e. running elevated).
pub fn check_access() -> bool {
    let path = hosts_path();
    // Try opening in append mode — doesn't write yet
    std::fs::OpenOptions::new().append(true).open(&path).is_ok()
}

/// Add or update a site entry inside the devstack block.
pub fn add_site(name: &str, tld: &str) -> crate::Result<()> {
    let path = hosts_path();
    let content = std::fs::read_to_string(&path).unwrap_or_default();
    let domain = format!("{name}.{tld}");
    let line = format!("127.0.0.1 {domain} www.{domain}\n");

    let new = if let (Some(start), Some(end)) =
        (content.find(MARKER_START), content.find(MARKER_END))
    {
        // block exists — insert or replace inside it
        let block_start = start + MARKER_START.len();
        let block_end = end;
        let block = &content[block_start..block_end];

        // dedupe: remove any existing line for this domain
        let mut new_block = String::new();
        for l in block.lines() {
            if !l.contains(&domain) {
                new_block.push_str(l);
                new_block.push('\n');
            }
        }
        new_block.push_str(&line);
        format!(
            "{}{}{}{}",
            &content[..start],
            MARKER_START,
            new_block,
            &content[block_end..]
        )
    } else {
        // no block yet — append
        format!("{content}\n{MARKER_START}\n{line}{MARKER_END}\n")
    };

    std::fs::write(&path, new).map_err(|e| {
        crate::Error::Config(format!(
            "cannot write {}: {e} — run as administrator once, or edit hosts manually",
            path.display()
        ))
    })?;
    Ok(())
}

/// Remove a site's hosts entry.
pub fn remove_site(name: &str, tld: &str) -> crate::Result<()> {
    let path = hosts_path();
    let content = std::fs::read_to_string(&path).unwrap_or_default();
    let domain = format!("{name}.{tld}");

    let new: String = content
        .lines()
        .filter(|l| !l.contains(&domain))
        .collect::<Vec<_>>()
        .join("\n");
    std::fs::write(&path, new)?;
    Ok(())
}

/// Flush Windows DNS cache after hosts changes.
pub fn flush_dns() {
    Command::new("ipconfig")
        .args(["/flushdns"])
        .output()
        .ok();
}
