use serde::{Deserialize, Serialize};
use std::path::PathBuf;

/// Global devstack config — devstack.toml
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct Config {
    /// TLD for virtual domains, e.g. "test" → foo.test
    pub tld: String,

    /// Apache listen port (default 8080 — non-root)
    pub http_port: u16,
    pub https_port: u16,

    /// MariaDB port
    pub mysql_port: u16,

    /// Default PHP version for new sites
    pub default_php: Option<String>,

    /// Default docroot parent, e.g. ~/Sites
    pub sites_root: PathBuf,

    /// Auto-start services when `devctl` runs and they are down
    pub autostart: bool,

    /// RAM profile: "low" (ondemand, small pools) or "balanced"
    pub ram_profile: RamProfile,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum RamProfile {
    Low,
    Balanced,
}

impl Default for Config {
    fn default() -> Self {
        let home = dirs::home_dir().unwrap_or_else(|| PathBuf::from("/tmp"));
        Self {
            tld: "test".into(),
            http_port: 8080,
            https_port: 8443,
            mysql_port: 3306,
            default_php: None,
            sites_root: home.join("Sites"),
            autostart: false,
            ram_profile: RamProfile::Low,
        }
    }
}

impl Config {
    pub fn load(paths: &crate::paths::Paths) -> crate::Result<Self> {
        let file = paths.config_file();
        if !file.exists() {
            let cfg = Self::default();
            cfg.save(paths)?;
            return Ok(cfg);
        }
        let text = std::fs::read_to_string(&file)?;
        Ok(toml::from_str(&text)?)
    }

    pub fn save(&self, paths: &crate::paths::Paths) -> crate::Result<()> {
        let text = toml::to_string_pretty(self)?;
        std::fs::write(paths.config_file(), text)?;
        Ok(())
    }
}
