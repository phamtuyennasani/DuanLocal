use clap::{Parser, Subcommand};
use devstack_core::{config::Config, paths::Paths, site::{Site, SiteRegistry}};

#[derive(Parser)]
#[command(name = "devctl", version, about = "DevStack — local dev environment manager")]
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
        #[arg(long)]
        https: bool,
    },
    /// Remove a site
    Remove { name: String },
    /// List sites
    List,
    /// Set PHP version for a site
    SetPhp { name: String, version: String },
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
    use devstack_core::*;

    paths.ensure_dirs()?;
    let config = Config::load(paths)?;

    println!("Setting up devstack in {}", paths.root.display());

    #[cfg(target_os = "macos")]
    {
        // DNS resolver + dnsmasq conf (needs sudo)
        let dnsmasq_conf = dns::dnsmasq_conf_path(paths);
        dns::setup_resolver(&config.tld, &dnsmasq_conf)?;
        println!("  ✓ /etc/resolver/{} + dnsmasq conf", config.tld);

        httpd::write_httpd_conf(paths, config.http_port)?;
        println!("  ✓ httpd.conf");

        mysql::write_my_cnf(paths, config.mysql_port)?;
        mysql::init_datadir(paths)?;
        println!("  ✓ my.cnf + datadir");

        for pv in php::detect_brew_phps(paths)? {
            php::write_fpm_pool_conf(&pv.version, paths, config.ram_profile)?;
            php::ensure_php_ini(&pv.version, paths)?;
            println!("  ✓ php-fpm pool for {}", pv.version);
        }

        dns::flush_dns_cache()?;
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
        println!("  ✓ httpd.conf (mod_fcgid)");

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
        // start php-cgi for each version used
        let versions: std::collections::BTreeSet<String> =
            registry.sites.values().map(|s| s.php.clone()).collect();
        for v in versions {
            if !service::is_running(&paths.pid_file(&format!("php-cgi-{v}"))) {
                php_win::start_php_cgi(&v, paths)?;
                println!("✓ php-cgi {v} :{}", provider::php_cgi_port(&v));
            }
        }
        if !service::is_running(&paths.pid_file("httpd")) {
            httpd_win::start_httpd(paths)?;
            println!("✓ httpd");
        }
    }

    println!("\n→ http://*.{tld}:{port}", tld = config.tld, port = config.http_port);
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
            let name = e.file_name().to_string_lossy();
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
                println!("  {:<20} {}", n.trim_end_matches(".pid"),
                    if running { "●" } else { "○" });
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
        SiteCmd::Add { name, php, root, https } => {
            let root = std::path::PathBuf::from(shellexpand(&root));
            registry.add(Site { name: name.clone(), root, php, https })?;
            registry.save(paths)?;
            vhost::VhostRenderer::new().render_all(&registry, &config, paths)?;

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
                println!("{:<16} {:<30} {:<6} {}", "NAME", "DOMAIN", "PHP", "ROOT");
                for s in registry.sites.values() {
                    println!("{:<16} {:<30} {:<6} {}",
                        s.name, s.domain(&config.tld), s.php, s.root.display());
                }
            }
        }
        SiteCmd::SetPhp { name, version } => {
            let site = registry.sites.get_mut(&name)
                .ok_or_else(|| devstack_core::Error::SiteNotFound(name.clone()))?;
            site.php = version;
            registry.save(paths)?;
            vhost::VhostRenderer::new().render_all(&registry, &config, paths)?;
            println!("✓ {name} → php {}", registry.sites[&name].php);
        }
    }
    Ok(())
}

// ── php ──────────────────────────────────────────────────────────────────────

fn cmd_php(paths: &Paths, cmd: PhpCmd) -> anyhow::Result<()> {
    use devstack_core::*;
    match cmd {
        PhpCmd::List => {
            #[cfg(target_os = "macos")]
            {
                let vers = php::detect_brew_phps(paths)?;
                if vers.is_empty() {
                    println!("no brew PHP found — brew tap shivammathur/php");
                } else {
                    for v in vers {
                        println!("  php@{}  ({})", v.version, v.fpm_bin.display());
                    }
                }
            }
            #[cfg(target_os = "windows")]
            {
                let vendored = vendor::list_vendored();
                let phps: Vec<_> = vendored.iter()
                    .filter(|n| n.starts_with("php-"))
                    .collect();
                if phps.is_empty() {
                    println!("no vendored PHP — `devctl install php-8.3`");
                } else {
                    for p in phps {
                        println!("  {p}");
                    }
                }
            }
        }
    }
    Ok(())
}

// ── install (Windows vendor downloads) ───────────────────────────────────────

fn cmd_install(paths: &Paths, package: &str) -> anyhow::Result<()> {
    use devstack_core::*;

    #[cfg(not(target_os = "windows"))]
    {
        anyhow::bail!("`devctl install` is only supported on Windows — use brew on macOS");
    }

    #[cfg(target_os = "windows")]
    {
        let url = match package {
            "httpd" => vendor::urls::httpd().to_string(),
            "mariadb" => vendor::urls::mariadb().to_string(),
            p if p.starts_with("php-") => {
                let ver = p.trim_start_matches("php-");
                vendor::urls::php(ver)
            }
            _ => anyhow::bail!("unknown package: {package} — httpd | mariadb | php-X.Y"),
        };
        println!("downloading {package} …");
        println!("  {url}");
        println!("\n(todo: download + extract to {} — not yet implemented)",
            vendor::vendor_dir(paths).display());
    }
    Ok(())
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
        cmd.args(["-NoProfile", "-Command",
            &format!("Get-Content -Path '{}' -Tail 100{}",
                file.display(),
                if follow { " -Wait" } else { "" }
            )]);
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
