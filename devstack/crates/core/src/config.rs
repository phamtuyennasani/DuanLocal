use serde::{Deserialize, Serialize};
use std::path::PathBuf;

/// Global devstack config — devstack.toml
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct Config {
    /// TLD for virtual domains, e.g. "test" → foo.test
    pub tld: String,

    /// Apache's unprivileged internal bind port; public HTTP is forwarded here.
    pub http_port: u16,
    /// Port users type in the browser. It matches http_port by default.
    pub public_port: u16,
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
        #[cfg(target_os = "macos")]
        let (http_port, public_port) = (8081, 80);
        #[cfg(not(target_os = "macos"))]
        let (http_port, public_port) = (8080, 8080);
        Self {
            tld: "test".into(),
            // macOS forwards the public default :80 to an unprivileged Apache
            // listener. Other platforms bind their configured port directly.
            http_port,
            public_port,
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
        let mut cfg: Self = toml::from_str(&text)?;
        #[cfg(target_os = "macos")]
        {
            // Migrate the former internal Apache listener to the new unprivileged
            // listener. Public URLs remain on :80.
            let legacy_http_port = 8_000 + 80;
            if cfg.http_port == legacy_http_port || cfg.public_port == legacy_http_port {
                cfg.http_port = 8081;
                cfg.public_port = 80;
                cfg.save(paths)?;
            }
        }
        crate::dns::validate_tld(&cfg.tld)?;
        cfg.validate_ports()?;
        Ok(cfg)
    }

    fn validate_ports(&self) -> crate::Result<()> {
        if self.http_port == 0
            || self.public_port == 0
            || self.https_port == 0
            || self.mysql_port == 0
        {
            return Err(crate::Error::Config(
                "ports must be between 1 and 65535".into(),
            ));
        }

        #[cfg(target_os = "macos")]
        {
            if self.public_port != 80 {
                return Err(crate::Error::Config(
                    "macOS public HTTP port must be 80; use the internal bind port for Apache"
                        .into(),
                ));
            }
            if self.http_port < 1024
                || self.https_port < 1024
                || self.https_port == 443
                || self.http_port == self.https_port
            {
                return Err(crate::Error::Config(
                    "macOS Apache HTTP and HTTPS bind ports must be distinct and 1024 or higher"
                        .into(),
                ));
            }
        }

        Ok(())
    }

    pub fn save(&self, paths: &crate::paths::Paths) -> crate::Result<()> {
        crate::dns::validate_tld(&self.tld)?;
        self.validate_ports()?;
        let text = toml::to_string_pretty(self)?;
        std::fs::write(paths.config_file(), text)?;
        Ok(())
    }
}
