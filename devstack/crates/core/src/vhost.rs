use handlebars::Handlebars;
use serde_json::json;
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

    {{#if php_cgi}}
    # Windows: php-cgi via Action/ScriptAlias. mod_actions supplies a real
    # filesystem SCRIPT_FILENAME (mod_proxy_fcgi sends a proxy:-prefixed one
    # that php-cgi rejects with "No input file specified").
    AddType application/x-httpd-php-{{php_mime}} .php
    Action application/x-httpd-php-{{php_mime}} "/devstack-php-{{php_mime}}/php-cgi.exe"
    {{else}}
    <FilesMatch \.php$>
        SetHandler "proxy:{{php_upstream}}"
    </FilesMatch>
    {{/if}}

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

    {{#if php_cgi}}
    # Windows: php-cgi via Action/ScriptAlias. mod_actions supplies a real
    # filesystem SCRIPT_FILENAME (mod_proxy_fcgi sends a proxy:-prefixed one
    # that php-cgi rejects with "No input file specified").
    AddType application/x-httpd-php-{{php_mime}} .php
    Action application/x-httpd-php-{{php_mime}} "/devstack-php-{{php_mime}}/php-cgi.exe"
    {{else}}
    <FilesMatch \.php$>
        SetHandler "proxy:{{php_upstream}}"
    </FilesMatch>
    {{/if}}

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
    /// PHP wiring is platform-specific — resolved from `site.php` internally.
    pub fn render_site(
        &self,
        site: &crate::site::Site,
        config: &crate::config::Config,
        paths: &crate::paths::Paths,
    ) -> crate::Result<std::path::PathBuf> {
        let domain = site.domain(&config.tld);
        let cert_dir = paths.certs_dir().join(&domain);
        let docroot = site.root.display().to_string().replace('\\', "/");

        let mut ctx = json!({
            "domain": domain,
            "docroot": docroot,
            "http_port": config.http_port,
            "https": site.https,
            "https_port": config.https_port,
            "cert_path": cert_dir.join("cert.pem").display().to_string().replace('\\', "/"),
            "key_path": cert_dir.join("key.pem").display().to_string().replace('\\', "/"),
            "log_dir": paths.logs_dir().display().to_string().replace('\\', "/"),
        });

        #[cfg(target_os = "macos")]
        {
            ctx["php_upstream"] = json!(format!(
                "unix:{}|fcgi://localhost",
                paths.php_fpm_socket(&site.php).display()
            ));
        }
        #[cfg(not(target_os = "macos"))]
        {
            ctx["php_cgi"] = json!(true);
            // mime suffix "8-3" keeps types unique per version
            ctx["php_mime"] = json!(site.php.replace('.', "-"));
        }

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
            self.render_site(site, config, paths)?;
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
