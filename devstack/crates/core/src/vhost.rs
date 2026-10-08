use handlebars::Handlebars;
use serde_json::{json, Value};
use std::path::Path;

/// Render an Apache vhost config for a site.
pub struct VhostRenderer {
    hb: Handlebars<'static>,
}

const VHOST_TEMPLATE: &str = r#"<VirtualHost *:{{http_port}}>
    ServerName {{domain}}
    ServerAlias www.{{domain}}
    DocumentRoot "{{docroot}}"

    <Directory "{{docroot}}">
        Options Indexes FollowSymLinks
        AllowOverride All
        Require all granted
    </Directory>

    <FilesMatch \.php$>
        SetHandler "proxy:{{php_upstream}}"
    </FilesMatch>

    ErrorLog "{{log_dir}}/{{domain}}-error.log"
    CustomLog "{{log_dir}}/{{domain}}-access.log" combined
</VirtualHost>
{{#if https}}
<VirtualHost *:{{https_port}}>
    ServerName {{domain}}
    ServerAlias www.{{domain}}
    DocumentRoot "{{docroot}}"

    SSLEngine on
    SSLCertificateFile "{{cert_path}}"
    SSLCertificateKeyFile "{{key_path}}"

    <Directory "{{docroot}}">
        Options Indexes FollowSymLinks
        AllowOverride All
        Require all granted
    </Directory>

    <FilesMatch \.php$>
        SetHandler "proxy:{{php_upstream}}"
    </FilesMatch>

    ErrorLog "{{log_dir}}/{{domain}}-ssl-error.log"
    CustomLog "{{log_dir}}/{{domain}}-ssl-access.log" combined
</VirtualHost>
{{/if}}
"#;

impl Default for VhostRenderer {
    fn default() -> Self {
        Self::new()
    }
}

impl VhostRenderer {
    pub fn new() -> Self {
        let mut hb = Handlebars::new();
        hb.set_strict_mode(true);
        hb.register_template_string("vhost", VHOST_TEMPLATE)
            .expect("vhost template is valid");
        Self { hb }
    }

    /// Render + write a vhost conf file for a site.
    /// `php_upstream` is the socket path (macOS) or "127.0.0.1:PORT" (Windows).
    pub fn render_site(
        &self,
        site: &crate::site::Site,
        config: &crate::config::Config,
        paths: &crate::paths::Paths,
        php_upstream: &str,
    ) -> crate::Result<std::path::PathBuf> {
        let domain = site.domain(&config.tld);
        let cert_dir = paths.certs_dir().join(&domain);

        let ctx = json!({
            "domain": domain,
            "docroot": site.root.display().to_string().replace('\\', "/"),
            "php_upstream": php_upstream,
            "http_port": config.http_port,
            "https": site.https,
            "https_port": config.https_port,
            "cert_path": cert_dir.join("cert.pem").display().to_string().replace('\\', "/"),
            "key_path": cert_dir.join("key.pem").display().to_string().replace('\\', "/"),
            "log_dir": paths.logs_dir().display().to_string().replace('\\', "/"),
        });

        let rendered = self.hb.render("vhost", &ctx)?;
        let out = paths.vhosts_dir().join(format!("{}.conf", site.name));
        std::fs::write(&out, rendered)?;
        Ok(out)
    }

    /// Regenerate all vhost confs. Uses provider to resolve per-site PHP upstream.
    pub fn render_all(
        &self,
        registry: &crate::site::SiteRegistry,
        config: &crate::config::Config,
        paths: &crate::paths::Paths,
    ) -> crate::Result<()> {
        for entry in std::fs::read_dir(paths.vhosts_dir())? {
            let entry = entry?;
            if entry.path().extension().map(|e| e == "conf").unwrap_or(false) {
                std::fs::remove_file(entry.path())?;
            }
        }
        for site in registry.sites.values() {
            let upstream = crate::provider::Provider::default().php_upstream(&site.php, paths);
            let upstream_str = match upstream {
                crate::provider::PhpUpstream::Socket(p) => {
                    format!("unix:{}", p.display())
                }
                crate::provider::PhpUpstream::Tcp(port) => {
                    format!("fcgi://127.0.0.1:{port}")
                }
            };
            self.render_site(site, config, paths, &upstream_str)?;
        }
        Ok(())
    }
}

/// Ensure the file `path` exists and contains `Include <dir>/*.conf`.
/// Used to wire our vhosts.d into the main httpd.conf.
pub fn ensure_include_directive(httpd_conf: &Path, vhosts_dir: &Path) -> std::io::Result<()> {
    let include_line = format!("Include \"{}/*.conf\"", vhosts_dir.display());
    let existing = std::fs::read_to_string(httpd_conf).unwrap_or_default();
    if !existing.contains(&include_line) {
        let mut new = existing;
        new.push_str(&format!("\n# devstack vhosts\n{include_line}\n"));
        std::fs::write(httpd_conf, new)?;
    }
    Ok(())
}
