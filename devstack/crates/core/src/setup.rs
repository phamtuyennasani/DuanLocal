#![cfg(target_os = "macos")]

use serde::Serialize;

use crate::{config::Config, dns, httpd, mysql, paths::Paths, php, site::SiteRegistry, vendor_mac};

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum SetupState {
    Ready,
    NeedsAdmin,
    NeedsUserSetup,
}

#[derive(Debug, Clone, Serialize)]
pub struct SetupStatus {
    pub state: SetupState,
    pub tld: String,
    pub resolver_ready: bool,
    pub pf_ready: bool,
    pub user_setup_ready: bool,
    pub detail: Option<String>,
}

/// Prepare all state that must remain owned by the normal user.
pub fn prepare_user(
    paths: &Paths,
    config: &Config,
    mut progress: impl FnMut(&str),
) -> crate::Result<()> {
    paths.ensure_dirs()?;
    dns::write_dnsmasq_conf(&config.tld, &dns::dnsmasq_conf_path(paths))?;

    for package in ["mysql", "dnsmasq", "mkcert"] {
        if !vendor_mac::is_installed(paths, package) {
            progress(&format!("Downloading {package}…"));
            vendor_mac::install(paths, package, |_| {})?;
        }
    }

    let registry = SiteRegistry::load(paths)?;
    let mut bind_ports = vec![config.http_port];
    for site in registry.sites.values() {
        if let Some(port) = site.port {
            if port >= 1024 {
                bind_ports.push(port);
            }
        }
    }
    if registry.sites.values().any(|site| site.https) {
        bind_ports.push(config.https_port);
    }
    httpd::write_httpd_conf(paths, &bind_ports)?;
    mysql::write_my_cnf(paths, config.mysql_port)?;
    mysql::init_datadir(paths)?;

    for php_version in php::detect_phps(paths)? {
        php::write_fpm_pool_conf(&php_version.version, paths, config.ram_profile)?;
        php::ensure_php_ini(&php_version.version, paths)?;
    }

    Ok(())
}

pub fn port_redirects(paths: &Paths, config: &Config) -> crate::Result<Vec<(u16, u16)>> {
    let registry = SiteRegistry::load(paths)?;
    let mut redirects = Vec::new();
    if config.public_port != 80 {
        return Err(crate::Error::Config(
            "only public HTTP port 80 can be redirected by DevStack on macOS".into(),
        ));
    }
    if config.http_port < 1024 {
        return Err(crate::Error::Config(
            "the internal Apache bind port must be 1024 or higher".into(),
        ));
    }
    if config.public_port != config.http_port {
        redirects.push((config.public_port, config.http_port));
    }
    if registry.sites.values().any(|site| site.https) && config.https_port != 443 {
        if config.https_port < 1024 {
            return Err(crate::Error::Config(
                "the internal HTTPS bind port must be 1024 or higher".into(),
            ));
        }
        redirects.push((443, config.https_port));
    }
    Ok(redirects)
}

pub fn status(paths: &Paths, config: &Config) -> SetupStatus {
    let resolver_ready = dns::check_resolver(&config.tld);
    let user_setup_ready = dns::check_dnsmasq_conf(&config.tld, &dns::dnsmasq_conf_path(paths))
        && paths.httpd_conf().exists()
        && paths.my_cnf().exists();
    let pf_redirects = port_redirects(paths, config).unwrap_or_default();
    let pf_ready = crate::elevate_macos::check_pf_setup(&pf_redirects);
    let state = if !user_setup_ready {
        SetupState::NeedsUserSetup
    } else if resolver_ready && pf_ready {
        SetupState::Ready
    } else {
        SetupState::NeedsAdmin
    };
    SetupStatus {
        state,
        tld: config.tld.clone(),
        resolver_ready,
        pf_ready,
        user_setup_ready,
        detail: None,
    }
}
