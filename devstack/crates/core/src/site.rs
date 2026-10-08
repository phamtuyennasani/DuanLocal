use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::PathBuf;

/// One registered site.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Site {
    /// Short name, e.g. "demo" → demo.test
    pub name: String,
    /// Document root (absolute path)
    pub root: PathBuf,
    /// PHP version, e.g. "8.3"
    pub php: String,
    /// Enable HTTPS via mkcert
    #[serde(default)]
    pub https: bool,
}

impl Site {
    pub fn domain(&self, tld: &str) -> String {
        format!("{}.{}", self.name, tld)
    }
}

/// Registry of all sites — sites.json
#[derive(Debug, Default, Serialize, Deserialize)]
pub struct SiteRegistry {
    #[serde(flatten)]
    pub sites: BTreeMap<String, Site>,
}

impl SiteRegistry {
    pub fn load(paths: &crate::paths::Paths) -> crate::Result<Self> {
        let file = paths.sites_file();
        if !file.exists() {
            return Ok(Self::default());
        }
        let text = std::fs::read_to_string(&file)?;
        Ok(serde_json::from_str(&text)?)
    }

    pub fn save(&self, paths: &crate::paths::Paths) -> crate::Result<()> {
        let text = serde_json::to_string_pretty(self)?;
        std::fs::write(paths.sites_file(), text)?;
        Ok(())
    }

    pub fn add(&mut self, site: Site) -> crate::Result<()> {
        if self.sites.contains_key(&site.name) {
            return Err(crate::Error::SiteExists(site.name.clone()));
        }
        if !site.root.exists() {
            return Err(crate::Error::Config(format!(
                "docroot does not exist: {}",
                site.root.display()
            )));
        }
        self.sites.insert(site.name.clone(), site);
        Ok(())
    }

    pub fn remove(&mut self, name: &str) -> crate::Result<Site> {
        self.sites
            .remove(name)
            .ok_or_else(|| crate::Error::SiteNotFound(name.to_string()))
    }

    pub fn get(&self, name: &str) -> Option<&Site> {
        self.sites.get(name)
    }
}
