use clap::{Parser, Subcommand};
use devstack_core::{
    config::Config,
    paths::Paths,
    site::{Site, SiteRegistry},
};

#[derive(Parser)]
#[command(
    name = "devctl",
    version,
    about = "DevStack — local dev environment manager"
)]
struct Cli {
    #[command(subcommand)]
    cmd: Cmd,
}

#[derive(Subcommand)]
enum Cmd {
    /// One-time system setup (DNS / hosts / config dirs).
    /// macOS: needs sudo for /etc/resolver. Windows: needs admin for hosts file.
    Setup,
    /// Start all services
    Up,
    /// Stop all services
    Down,
    /// Show service status
    Status,
    /// Manage sites
    Site {
        #[command(subcommand)]
        cmd: SiteCmd,
    },
    /// Manage PHP versions
    Php {
        #[command(subcommand)]
        cmd: PhpCmd,
    },
    /// Download a vendored binary (Windows) — httpd / mariadb / php-8.3
    Install {
        /// package: httpd | mariadb | php-8.3 | php-8.2 | …
        package: String,
    },
    /// Tail service logs
    Logs {
        service: String,
        #[arg(short, long)]
        follow: bool,
    },
}

#[derive(Subcommand)]
enum SiteCmd {
    /// Add a site: devctl site add demo --php 8.3 --root ~/code/demo/public
    Add {
        name: String,
        #[arg(long)]
        php: String,
        #[arg(long)]
        root: String,
        /// public port — default from config (80). Use a ≥1024 port for a
        /// dedicated bind (e.g. api.test:9000), no pf needed
        #[arg(long)]
        port: Option<u16>,
        #[arg(long)]
        https: bool,
    },
    /// Remove a site
    Remove { name: String },
    /// List sites
    List,
    /// Set PHP version for a site
    SetPhp { name: String, version: String },
    /// Set a public port override, or `default` to use the global port (80)
    SetPort {
        name: String,
        /// Port number or "default"
        port: Option<u16>,
    },
}

#[derive(Subcommand)]
enum PhpCmd {
    /// List detected/installed versions
    List,
}

fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::from_default_env()
                .add_directive("devstack_core=info".parse()?),
        )
        .init();

    let cli = Cli::parse();
    let paths = Paths::detect()?;

    match cli.cmd {
        Cmd::Setup => cmd_setup(&paths),
        Cmd::Up => cmd_up(&paths),
        Cmd::Down => cmd_down(&paths),
        Cmd::Status => cmd_status(&paths),
        Cmd::Site { cmd } => cmd_site(&paths, cmd),
        Cmd::Php { cmd } => cmd_php(&paths, cmd),
        Cmd::Install { package } => cmd_install(&paths, &package),
        Cmd::Logs { service, follow } => cmd_logs(&paths, &service, follow),
    }
}

// ── setup ────────────────────────────────────────────────────────────────────

fn cmd_setup(paths: &Paths) -> anyhow::Result<()> {
    #[cfg(target_os = "macos")]
    if std::env::var_os("SUDO_USER").is_some()
        || std::env::var_os("USER").as_deref() == Some(std::ffi::OsStr::new("root"))
    {
        anyhow::bail!("do not run the full DevStack setup with sudo; open DevStack and approve its administrator prompt instead");
    }

    paths.ensure_dirs()?;
    let config = Config::load(paths)?;

    #[cfg(target_os = "macos")]
    {
        // User-owned runtime/configuration setup. Do not run this command through
        // sudo: the packaged application uses its own native admin prompt only
        // for /etc and pf changes.
        devstack_core::setup::prepare_user(paths, &config, |message| println!("  … {message}"))?;
        println!("  ✓ user-owned runtime files prepared");

        let redirects = devstack_core::setup::port_redirects(paths, &config)?;
        match devstack_core::elevate_macos::run_with_admin_dialog(&config.tld, &redirects)? {
            devstack_core::elevate_macos::AdminOutcome::Applied
            | devstack_core::elevate_macos::AdminOutcome::AlreadyConfigured => {
                println!("  ✓ local domains and public ports enabled");
            }
            devstack_core::elevate_macos::AdminOutcome::Cancelled => {
                println!("  ! administrator approval cancelled — retry from DevStack when ready");
            }
        }
    }

    #[cfg(target_os = "windows")]
    {
        // hosts file needs admin — elevate if not already
        if !elevate::is_elevated() {
            println!("  ! hosts file needs admin — re-run `devctl setup` as Administrator");
        } else {
            // seed the devstack block so add_site can append cleanly
            println!("  ✓ admin — hosts file writable");
        }

        httpd_win::write_httpd_conf(paths, config.http_port)?;
        println!("  ✓ httpd.conf (mod_proxy_fcgi)");

        mysql::write_my_cnf(paths, config.mysql_port)?;
        mysql::init_datadir(paths)?;
        println!("  ✓ my.cnf + datadir");

        for v in php::detect_vendored_phps(paths)? {
            php_win::ensure_php_ini(&v, paths)?;
            println!("  ✓ php.ini for {v}");
        }

        hosts::flush_dns();
    }

    println!("\nDone. Try: devctl site add demo --php <ver> --root ~/Sites/demo");
    Ok(())
}

// ── up / down / status ────────────────────────────────────────────────────────

fn cmd_up(paths: &Paths) -> anyhow::Result<()> {
    use devstack_core::*;
    paths.ensure_dirs()?;
    let config = Config::load(paths)?;
    let registry = SiteRegistry::load(paths)?;

    vhost::VhostRenderer::new().render_all(&registry, &config, paths)?;

    #[cfg(target_os = "macos")]
    sync_httpd_binds(paths, &registry, &config)?;

    #[cfg(target_os = "macos")]
    {
        if !service::is_running(&paths.pid_file("dnsmasq")) {
            dns::start_dnsmasq(paths)?;
            println!("✓ dnsmasq");
        }
        if !service::is_running(&paths.pid_file("mysqld")) {
            mysql::start_mysqld(paths)?;
            println!("✓ mysqld");
        }
        let versions: std::collections::BTreeSet<String> =
            registry.sites.values().map(|s| s.php.clone()).collect();
        for v in versions {
            if !service::is_running(&paths.pid_file(&format!("php-fpm-{v}"))) {
                php::start_fpm(&v, paths)?;
                println!("✓ php-fpm {v}");
            }
        }
        if !service::is_running(&paths.pid_file("httpd")) {
            httpd::start_httpd(paths)?;
            println!("✓ httpd");
        }
    }

    #[cfg(target_os = "windows")]
    {
        if !service::is_running(&paths.pid_file("mysqld")) {
            mysql::start_mysqld(paths)?;
            println!("✓ mysqld");
        }
        // No php-cgi daemon needed — PHP runs as classic CGI via Action/ScriptAlias
        // (mod_actions supplies the real SCRIPT_FILENAME mod_proxy_fcgi can't).
        if !service::is_running(&paths.pid_file("httpd")) {
            httpd_win::start_httpd(paths)?;
            println!("✓ httpd");
        }
    }

    let port_display = if config.public_port == 80 {
        String::new()
    } else {
        format!(":{}", config.public_port)
    };
    println!("\n→ http://*.{tld}{port_display}", tld = config.tld);
    Ok(())
}

fn cmd_down(paths: &Paths) -> anyhow::Result<()> {
    use devstack_core::*;

    #[cfg(target_os = "macos")]
    {
        let _ = httpd::stop_httpd(paths);
        let _ = mysql::stop_mysqld(paths);
        let _ = dns::stop_dnsmasq(paths);
    }
    #[cfg(target_os = "windows")]
    {
        let _ = httpd_win::stop_httpd(paths);
        let _ = mysql::stop_mysqld(paths);
    }

    // stop all php-* pid files
    if let Ok(rd) = std::fs::read_dir(paths.run_dir()) {
        for e in rd.flatten() {
            let name = e.file_name().to_string_lossy().to_string();
            if (name.starts_with("php-fpm-") || name.starts_with("php-cgi-"))
                && name.ends_with(".pid")
            {
                let _ = service::stop_daemon(&e.path(), 5);
            }
        }
    }
    println!("✓ all stopped");
    Ok(())
}

fn cmd_status(paths: &Paths) -> anyhow::Result<()> {
    use devstack_core::*;
    let check = |name: &str| {
        let running = service::is_running(&paths.pid_file(name));
        println!("  {:<20} {}", name, if running { "●" } else { "○" });
    };
    println!("services:");

    #[cfg(target_os = "macos")]
    {
        check("dnsmasq");
        check("mysqld");
        check("httpd");
    }
    #[cfg(target_os = "windows")]
    {
        check("mysqld");
        check("httpd");
    }

    if let Ok(rd) = std::fs::read_dir(paths.run_dir()) {
        for e in rd.flatten() {
            let n = e.file_name().to_string_lossy().to_string();
            if (n.starts_with("php-fpm-") || n.starts_with("php-cgi-")) && n.ends_with(".pid") {
                let running = service::is_running(&e.path());
                println!(
                    "  {:<20} {}",
                    n.trim_end_matches(".pid"),
                    if running { "●" } else { "○" }
                );
            }
        }
    }
    Ok(())
}

// ── site ─────────────────────────────────────────────────────────────────────

fn cmd_site(paths: &Paths, cmd: SiteCmd) -> anyhow::Result<()> {
    use devstack_core::*;
    let mut registry = SiteRegistry::load(paths)?;
    let config = Config::load(paths)?;

    match cmd {
        SiteCmd::Add {
            name,
            php,
            root,
            port,
            https,
        } => {
            let root = std::path::PathBuf::from(shellexpand(&root));
            registry.add(Site {
                name: name.clone(),
                root,
                php,
                port,
                https,
            })?;
            registry.save(paths)?;
            vhost::VhostRenderer::new().render_all(&registry, &config, paths)?;
            #[cfg(target_os = "macos")]
            sync_httpd_binds(paths, &registry, &config)?;

            #[cfg(target_os = "windows")]
            {
                // write hosts entry — may need elevation
                if let Err(e) = hosts::add_site(&name, &config.tld) {
                    eprintln!("warning: {e}");
                    eprintln!("  → site added but DNS won't resolve until hosts is updated");
                } else {
                    hosts::flush_dns();
                }
            }

            println!("✓ {}.{}", name, config.tld);
        }
        SiteCmd::Remove { name } => {
            registry.remove(&name)?;
            registry.save(paths)?;
            let _ = std::fs::remove_file(paths.vhosts_dir().join(format!("{name}.conf")));

            // clean up the site's log files too — access/error/ssl variants
            let domain = format!("{}.{}", name, config.tld);
            for pat in [
                format!("{domain}-access.log"),
                format!("{domain}-error.log"),
                format!("{domain}-ssl-access.log"),
                format!("{domain}-ssl-error.log"),
            ] {
                let _ = std::fs::remove_file(paths.logs_dir().join(&pat));
            }

            #[cfg(target_os = "macos")]
            sync_httpd_binds(paths, &registry, &config)?;

            #[cfg(target_os = "windows")]
            {
                let _ = hosts::remove_site(&name, &config.tld);
                hosts::flush_dns();
            }

            println!("✓ {name} removed");
        }
        SiteCmd::List => {
            if registry.sites.is_empty() {
                println!("no sites — `devctl site add <name> --php <ver> --root <path>`");
            } else {
                println!(
                    "{:<16} {:<30} {:<6} {:<6} {}",
                    "NAME", "DOMAIN", "PORT", "PHP", "ROOT"
                );
                for s in registry.sites.values() {
                    let port = s
                        .port
                        .map(|p| p.to_string())
                        .unwrap_or_else(|| config.public_port.to_string());
                    println!(
                        "{:<16} {:<30} {:<6} {:<6} {}",
                        s.name,
                        s.domain(&config.tld),
                        port,
                        s.php,
                        s.root.display()
                    );
                }
            }
        }
        SiteCmd::SetPhp { name, version } => {
            let site = registry
                .sites
                .get_mut(&name)
                .ok_or_else(|| devstack_core::Error::SiteNotFound(name.clone()))?;
            site.php = version;
            registry.save(paths)?;
            vhost::VhostRenderer::new().render_all(&registry, &config, paths)?;
            println!("✓ {name} → php {}", registry.sites[&name].php);
        }
        SiteCmd::SetPort { name, port } => {
            if matches!(port, Some(value) if value < 1024) {
                anyhow::bail!(
                    "custom site ports must be 1024 or higher; use the default port for 80"
                );
            }
            let site = registry
                .sites
                .get_mut(&name)
                .ok_or_else(|| devstack_core::Error::SiteNotFound(name.clone()))?;
            site.port = port;
            registry.save(paths)?;
            vhost::VhostRenderer::new().render_all(&registry, &config, paths)?;
            #[cfg(target_os = "macos")]
            sync_httpd_binds(paths, &registry, &config)?;
            match port {
                Some(p) => println!("✓ {name} → :{p}"),
                None => println!("✓ {name} → default :{}", config.public_port),
            }
        }
    }
    Ok(())
}

/// Keep httpd.conf's Listen set in sync with site ports.
/// Public :80 is forwarded to the unprivileged internal Apache bind.
#[cfg(target_os = "macos")]
fn sync_httpd_binds(
    paths: &Paths,
    registry: &devstack_core::site::SiteRegistry,
    config: &devstack_core::config::Config,
) -> anyhow::Result<()> {
    use devstack_core::*;
    let mut binds = vec![config.http_port];
    for s in registry.sites.values() {
        if let Some(p) = s.port {
            if p >= 1024 {
                binds.push(p);
            }
        }
    }
    // any https-enabled site → also Listen on the TLS bind port
    if registry.sites.values().any(|s| s.https) {
        binds.push(config.https_port);
    }
    httpd::write_httpd_conf(paths, &binds)?;
    // reload apache if running so binds take effect now
    if service::is_running(&paths.pid_file("httpd")) {
        let _ = httpd::stop_httpd(paths);
        httpd::start_httpd(paths)?;
    }
    Ok(())
}

// ── php ──────────────────────────────────────────────────────────────────────

fn cmd_php(_paths: &Paths, cmd: PhpCmd) -> anyhow::Result<()> {
    use devstack_core::*;
    match cmd {
        PhpCmd::List => {
            #[cfg(target_os = "windows")]
            {
                let vendored = vendor::list_vendored();
                let phps: Vec<_> = vendored.iter().filter(|n| n.starts_with("php-")).collect();
                if phps.is_empty() {
                    println!("no vendored PHP — `devctl install php-8.3`");
                } else {
                    for p in phps {
                        println!("  {p}");
                    }
                }
            }
            #[cfg(target_os = "macos")]
            {
                let vers = php::detect_phps(_paths)?;
                if vers.is_empty() {
                    println!("no PHP found — `devctl install php-8.4`");
                } else {
                    for v in vers {
                        println!("  php@{}  ({})", v.version, v.fpm_bin.display());
                    }
                }
            }
        }
    }
    Ok(())
}

// ── install (Windows vendor downloads) ───────────────────────────────────────

#[cfg(target_os = "windows")]
fn cmd_install(paths: &Paths, package: &str) -> anyhow::Result<()> {
    use devstack_core::vendor::{self, Progress};
    use std::io::Write;

    let dest = vendor::install(paths, package, |p| match p {
        Progress::Resolving => {
            print!("resolving {package}…\r");
            let _ = std::io::stdout().flush();
        }
        Progress::Downloading { got, total } => {
            let mb = got as f64 / 1_048_576.0;
            match total {
                Some(t) => {
                    let pct = (got as f64 / t as f64 * 100.0).min(100.0);
                    print!(
                        "  {mb:.1} / {:.1} MB  ({pct:.0}%)\r",
                        t as f64 / 1_048_576.0
                    );
                }
                None => print!("  {mb:.1} MB\r"),
            }
            let _ = std::io::stdout().flush();
        }
        Progress::Extracting => println!("\nextracting…"),
        Progress::Done(d) => println!("✓ {} → {}", package, d.display()),
    })
    .map_err(|e| anyhow::anyhow!("{e}"))?;

    // If a PHP was just installed, drop a php.ini for it.
    if package.starts_with("php-") {
        let ver = package.trim_start_matches("php-");
        devstack_core::php_win::ensure_php_ini(ver, paths).ok();
    }
    let _ = dest;
    Ok(())
}

#[cfg(target_os = "macos")]
fn cmd_install(paths: &Paths, package: &str) -> anyhow::Result<()> {
    use devstack_core::vendor_mac::Progress;
    use std::io::Write;

    let pkg = package.to_string();
    let dest = devstack_core::vendor_mac::install(paths, package, move |p| match p {
        Progress::Resolving => {
            print!("resolving {pkg}…\r");
            let _ = std::io::stdout().flush();
        }
        Progress::Downloading { got, total } => {
            let mb = got as f64 / 1_048_576.0;
            match total {
                Some(t) => {
                    let pct = (got as f64 / t as f64 * 100.0).min(100.0);
                    print!(
                        "  {mb:.1} / {:.1} MB  ({pct:.0}%)   \r",
                        t as f64 / 1_048_576.0
                    );
                }
                None => print!("  {mb:.1} MB   \r"),
            }
            let _ = std::io::stdout().flush();
        }
        Progress::Extracting => println!("\nextracting…"),
        Progress::Done(d) => println!("✓ {pkg} → {}", d.display()),
    })
    .map_err(|e| anyhow::anyhow!("{e}"))?;
    let _ = dest;
    Ok(())
}

#[cfg(not(any(target_os = "windows", target_os = "macos")))]
fn cmd_install(_paths: &Paths, _package: &str) -> anyhow::Result<()> {
    anyhow::bail!("`devctl install` is only supported on macOS and Windows")
}

// ── logs ─────────────────────────────────────────────────────────────────────

fn cmd_logs(paths: &Paths, service: &str, follow: bool) -> anyhow::Result<()> {
    let file = paths.log_file(service);
    if !file.exists() {
        anyhow::bail!("no log for {service}");
    }
    #[cfg(target_os = "windows")]
    {
        // tail not native on Windows — use PowerShell Get-Content -Wait
        let mut cmd = std::process::Command::new("powershell");
        cmd.args([
            "-NoProfile",
            "-Command",
            &format!(
                "Get-Content -Path '{}' -Tail 100{}",
                file.display(),
                if follow { " -Wait" } else { "" }
            ),
        ]);
        let status = cmd.status()?;
        std::process::exit(status.code().unwrap_or(0));
    }
    #[cfg(not(target_os = "windows"))]
    {
        let mut cmd = std::process::Command::new("tail");
        cmd.arg(&file);
        if follow {
            cmd.arg("-f");
        } else {
            cmd.args(["-n", "50"]);
        }
        let status = cmd.status()?;
        std::process::exit(status.code().unwrap_or(0));
    }
}

fn shellexpand(s: &str) -> String {
    if let Some(rest) = s.strip_prefix("~/") {
        if let Some(home) = dirs::home_dir() {
            return home.join(rest).display().to_string();
        }
    }
    s.to_string()
}
