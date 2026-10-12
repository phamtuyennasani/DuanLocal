// Prevents a console window on release Windows builds.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod metrics;

use devstack_core::{
    config::Config,
    paths::Paths,
    service,
    site::{Site, SiteRegistry},
    vhost::VhostRenderer,
};
#[cfg(target_os = "windows")]
use devstack_core::{hosts, httpd_win, mysql, php_win, vendor};
use serde::Serialize;
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Mutex,
};
use tauri::{AppHandle, Emitter, Manager, State};

#[cfg(target_os = "macos")]
use devstack_core::{certs, dns, httpd, mysql, php, vendor_mac};

struct AppState {
    paths: Paths,
}

struct MetricsState(Mutex<metrics::Sampler>);

#[cfg(target_os = "macos")]
struct SetupGuard(AtomicBool);

#[derive(Serialize)]
struct ServiceStatus {
    name: String,
    running: bool,
}

#[derive(Serialize)]
struct SiteDto {
    name: String,
    domain: String,
    root: String,
    php: String,
    /// public URL port — Site.port if set, else config.public_port
    port: u16,
    /// true when Site.port is a custom override (not the default)
    custom_port: bool,
    https: bool,
}

#[derive(Serialize)]
struct PackageDto {
    name: String,    // "php-8.3" | "mariadb" | "httpd"
    kind: String,    // "php" | "mariadb" | "httpd"
    version: String, // "8.3" | "11.4.2" | "2.4.69"
    installed: bool,
    path: String, // vendor dir if installed
}

fn status_of(paths: &Paths, name: &str) -> ServiceStatus {
    ServiceStatus {
        name: name.to_string(),
        running: service::is_running(&paths.pid_file(name)),
    }
}

/// PHP service pid-file prefix differs per OS.
#[cfg(target_os = "macos")]
const PHP_SVC_PREFIX: &str = "php-fpm-";
#[cfg(target_os = "windows")]
const PHP_SVC_PREFIX: &str = "php-cgi-";

#[tauri::command]
fn get_system_metrics(state: State<'_, MetricsState>) -> metrics::SystemMetrics {
    state
        .0
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .sample()
}

#[cfg(target_os = "macos")]
#[tauri::command]
fn get_setup_status(
    state: State<'_, Mutex<AppState>>,
) -> Result<devstack_core::setup::SetupStatus, String> {
    let paths = state.lock().unwrap().paths.clone();
    let config = Config::load(&paths).map_err(|e| e.to_string())?;
    Ok(devstack_core::setup::status(&paths, &config))
}

#[cfg(target_os = "macos")]
#[tauri::command]
fn run_setup(
    app: AppHandle,
    state: State<'_, Mutex<AppState>>,
    guard: State<'_, SetupGuard>,
) -> Result<(), String> {
    if guard.0.swap(true, Ordering::AcqRel) {
        return Err("DevStack setup is already running".into());
    }
    let paths = state.lock().unwrap().paths.clone();
    std::thread::spawn(move || {
        let emit = |phase: &str, message: &str| {
            let _ = app.emit(
                "setup-progress",
                serde_json::json!({ "phase": phase, "message": message }),
            );
        };
        let result = (|| -> Result<(), String> {
            emit("preparing", "Preparing your local DevStack files…");
            let config = Config::load(&paths).map_err(|e| e.to_string())?;
            devstack_core::setup::prepare_user(&paths, &config, |message| {
                emit("preparing", message)
            })
            .map_err(|e| e.to_string())?;

            let redirects =
                devstack_core::setup::port_redirects(&paths, &config).map_err(|e| e.to_string())?;
            let status = devstack_core::setup::status(&paths, &config);
            if !matches!(status.state, devstack_core::setup::SetupState::Ready) {
                emit("admin", "Waiting for administrator approval…");
                match devstack_core::elevate_macos::run_with_admin_dialog(&config.tld, &redirects)
                    .map_err(|e| e.to_string())?
                {
                    devstack_core::elevate_macos::AdminOutcome::Cancelled => {
                        emit("cancelled", "Administrator approval was cancelled.");
                        return Err("Administrator setup was cancelled".into());
                    }
                    devstack_core::elevate_macos::AdminOutcome::Applied
                    | devstack_core::elevate_macos::AdminOutcome::AlreadyConfigured => {}
                }
            }
            emit("done", "DevStack is ready.");
            Ok(())
        })();
        if let Err(error) = &result {
            emit("error", error);
        }
        let _ = app.emit(
            "setup-finished",
            serde_json::json!({ "ok": result.is_ok() }),
        );
        if let Some(guard) = app.try_state::<SetupGuard>() {
            guard.0.store(false, Ordering::Release);
        }
    });
    Ok(())
}

#[tauri::command]
fn get_status(state: State<'_, Mutex<AppState>>) -> Vec<ServiceStatus> {
    let paths = &state.lock().unwrap().paths;
    let mut out: Vec<ServiceStatus> = Vec::new();
    #[cfg(target_os = "macos")]
    {
        out.push(status_of(paths, "dnsmasq"));
        out.push(status_of(paths, "mysqld"));
        out.push(status_of(paths, "httpd"));
    }
    #[cfg(target_os = "windows")]
    {
        out.push(status_of(paths, "mysqld"));
        out.push(status_of(paths, "httpd"));
    }
    // Per-version PHP services. Include every installed PHP version even when
    // no site currently uses it, so Start all manages the complete runtime set.
    let mut php_services = std::collections::BTreeSet::new();
    #[cfg(target_os = "macos")]
    {
        for version in php::detect_phps(paths).unwrap_or_default() {
            php_services.insert(format!("{PHP_SVC_PREFIX}{}", version.version));
        }
    }
    if let Ok(rd) = std::fs::read_dir(paths.run_dir()) {
        for e in rd.flatten() {
            let n = e.file_name().to_string_lossy().to_string();
            if n.starts_with(PHP_SVC_PREFIX) && n.ends_with(".pid") {
                php_services.insert(n.trim_end_matches(".pid").to_string());
            }
        }
    }
    for name in php_services {
        out.push(status_of(paths, &name));
    }
    out
}

#[tauri::command]
fn get_sites(state: State<'_, Mutex<AppState>>) -> Result<Vec<SiteDto>, String> {
    let paths = &state.lock().unwrap().paths;
    let cfg = Config::load(paths).map_err(|e| e.to_string())?;
    let reg = SiteRegistry::load(paths).map_err(|e| e.to_string())?;
    Ok(reg
        .sites
        .values()
        .map(|s| SiteDto {
            name: s.name.clone(),
            domain: s.domain(&cfg.tld),
            root: s.root.display().to_string(),
            php: s.php.clone(),
            port: s.port.unwrap_or(cfg.public_port),
            custom_port: s.port.is_some(),
            https: s.https,
        })
        .collect())
}

#[tauri::command]
fn get_php_versions(state: State<'_, Mutex<AppState>>) -> Vec<String> {
    let paths = &state.lock().unwrap().paths;
    #[cfg(target_os = "macos")]
    {
        return php::detect_brew_phps(paths)
            .unwrap_or_default()
            .iter()
            .map(|v| v.version.clone())
            .collect();
    }
    #[cfg(target_os = "windows")]
    {
        return devstack_core::php::detect_vendored_phps(paths).unwrap_or_default();
    }
}

#[tauri::command]
fn add_site(
    state: State<'_, Mutex<AppState>>,
    name: String,
    php: String,
    root: String,
    port: Option<u16>,
    https: bool,
) -> Result<(), String> {
    let paths = &state.lock().unwrap().paths;
    let cfg = Config::load(paths).map_err(|e| e.to_string())?;
    let mut reg = SiteRegistry::load(paths).map_err(|e| e.to_string())?;
    reg.add(Site {
        name: name.clone(),
        php,
        root: root.into(),
        port,
        https,
    })
    .map_err(|e| e.to_string())?;
    reg.save(paths).map_err(|e| e.to_string())?;
    VhostRenderer::new()
        .render_all(&reg, &cfg, paths)
        .map_err(|e| e.to_string())?;

    #[cfg(target_os = "macos")]
    {
        // https site needs a cert — mkcert CA comes from `sudo devctl setup`
        if https {
            let domain = format!("{}.{}", name, cfg.tld);
            certs::generate_site_cert(paths, &domain)
                .map_err(|e| format!("cert for {domain}: {e}"))?;
        }
        sync_httpd_binds(paths, &reg, &cfg)?;
    }

    #[cfg(target_os = "windows")]
    {
        // write hosts entry — needs the app run as admin once
        if let Err(e) = hosts::add_site(&name, &cfg.tld) {
            return Err(format!("site saved but hosts update failed: {e}"));
        }
        hosts::flush_dns();
    }
    Ok(())
}

#[tauri::command]
fn remove_site(state: State<'_, Mutex<AppState>>, name: String) -> Result<(), String> {
    let paths = &state.lock().unwrap().paths;
    let cfg = Config::load(paths).map_err(|e| e.to_string())?;
    let mut reg = SiteRegistry::load(paths).map_err(|e| e.to_string())?;
    let removed = reg.remove(&name).map_err(|e| e.to_string())?;
    reg.save(paths).map_err(|e| e.to_string())?;
    let _ = std::fs::remove_file(paths.vhosts_dir().join(format!("{name}.conf")));

    // clean up the site's log files — access/error/ssl variants
    let domain = format!("{}.{}", name, cfg.tld);
    for pat in [
        format!("{domain}-access.log"),
        format!("{domain}-error.log"),
        format!("{domain}-ssl-access.log"),
        format!("{domain}-ssl-error.log"),
    ] {
        let _ = std::fs::remove_file(paths.logs_dir().join(&pat));
    }

    // remove the site's TLS cert dir too
    #[cfg(target_os = "macos")]
    certs::remove_site_cert(paths, &domain);
    let _ = removed;

    VhostRenderer::new()
        .render_all(&reg, &cfg, paths)
        .map_err(|e| e.to_string())?;

    #[cfg(target_os = "macos")]
    sync_httpd_binds(paths, &reg, &cfg)?;

    #[cfg(target_os = "windows")]
    {
        let _ = hosts::remove_site(&name, &cfg.tld);
        hosts::flush_dns();
    }
    Ok(())
}

/// Sync httpd.conf Listen lines with the site's current port set (macOS).
/// Public :80 is forwarded to the unprivileged internal Apache bind by pf.
#[cfg(target_os = "macos")]
fn sync_httpd_binds(paths: &Paths, reg: &SiteRegistry, cfg: &Config) -> Result<(), String> {
    let mut binds = vec![cfg.http_port];
    for s in reg.sites.values() {
        if let Some(p) = s.port {
            if p >= 1024 {
                binds.push(p);
            }
        }
    }
    // any https site → also Listen on the TLS bind port
    if reg.sites.values().any(|s| s.https) {
        binds.push(cfg.https_port);
    }
    httpd::write_httpd_conf(paths, &binds).map_err(|e| e.to_string())?;
    if service::is_running(&paths.pid_file("httpd")) {
        let _ = httpd::stop_httpd(paths);
        httpd::start_httpd(paths).map_err(|e| e.to_string())?;
    }
    Ok(())
}

#[tauri::command]
fn set_site_php(
    state: State<'_, Mutex<AppState>>,
    name: String,
    version: String,
) -> Result<(), String> {
    let paths = &state.lock().unwrap().paths;
    let cfg = Config::load(paths).map_err(|e| e.to_string())?;
    let mut reg = SiteRegistry::load(paths).map_err(|e| e.to_string())?;
    let site = reg
        .sites
        .get_mut(&name)
        .ok_or_else(|| format!("site not found: {name}"))?;
    site.php = version;
    reg.save(paths).map_err(|e| e.to_string())?;
    VhostRenderer::new()
        .render_all(&reg, &cfg, paths)
        .map_err(|e| e.to_string())
}

/// Change a site's public port. `port = None` resets to the global default.
/// Regenerates httpd.conf so Apache binds the new set of ports.
#[tauri::command]
fn set_site_port(
    state: State<'_, Mutex<AppState>>,
    name: String,
    port: Option<u16>,
) -> Result<(), String> {
    if matches!(port, Some(value) if value < 1024) {
        return Err("custom site ports must be 1024 or higher; use the default port for 80".into());
    }
    let paths = &state.lock().unwrap().paths;
    let cfg = Config::load(paths).map_err(|e| e.to_string())?;
    let mut reg = SiteRegistry::load(paths).map_err(|e| e.to_string())?;
    let site = reg
        .sites
        .get_mut(&name)
        .ok_or_else(|| format!("site not found: {name}"))?;
    site.port = port;
    reg.save(paths).map_err(|e| e.to_string())?;
    VhostRenderer::new()
        .render_all(&reg, &cfg, paths)
        .map_err(|e| e.to_string())?;

    #[cfg(target_os = "macos")]
    sync_httpd_binds(paths, &reg, &cfg)?;
    Ok(())
}

// ── service lifecycle (platform-aware) ───────────────────────────────────────

fn start_one(paths: &Paths, name: &str) -> Result<(), String> {
    match name {
        "mysqld" => mysql::start_mysqld(paths).map_err(|e| e.to_string()),
        "httpd" => {
            #[cfg(target_os = "macos")]
            return httpd::start_httpd(paths).map_err(|e| e.to_string());
            #[cfg(target_os = "windows")]
            return httpd_win::start_httpd(paths).map_err(|e| e.to_string());
        }
        #[cfg(target_os = "macos")]
        "dnsmasq" => dns::start_dnsmasq(paths).map_err(|e| e.to_string()),
        n if n.starts_with(PHP_SVC_PREFIX) => {
            let v = n.trim_start_matches(PHP_SVC_PREFIX);
            #[cfg(target_os = "macos")]
            return php::start_fpm(v, paths).map_err(|e| e.to_string());
            #[cfg(target_os = "windows")]
            return php_win::start_php_cgi(v, paths).map_err(|e| e.to_string());
        }
        _ => Err(format!("unknown service: {name}")),
    }
}

fn stop_one(paths: &Paths, name: &str) -> Result<(), String> {
    match name {
        "mysqld" => mysql::stop_mysqld(paths).map_err(|e| e.to_string()),
        "httpd" => {
            #[cfg(target_os = "macos")]
            return httpd::stop_httpd(paths).map_err(|e| e.to_string());
            #[cfg(target_os = "windows")]
            return httpd_win::stop_httpd(paths).map_err(|e| e.to_string());
        }
        #[cfg(target_os = "macos")]
        "dnsmasq" => dns::stop_dnsmasq(paths).map_err(|e| e.to_string()),
        n if n.starts_with(PHP_SVC_PREFIX) => {
            let v = n.trim_start_matches(PHP_SVC_PREFIX);
            #[cfg(target_os = "macos")]
            return php::stop_fpm(v, paths).map_err(|e| e.to_string());
            #[cfg(target_os = "windows")]
            return php_win::stop_php_cgi(v, paths).map_err(|e| e.to_string());
        }
        _ => Err(format!("unknown service: {name}")),
    }
}

#[tauri::command]
fn service_start_all(state: State<'_, Mutex<AppState>>) -> Result<(), String> {
    let paths = state.lock().unwrap().paths.clone();
    std::thread::spawn(move || {
        let cfg = match Config::load(&paths) {
            Ok(c) => c,
            Err(_) => return,
        };
        let reg = match SiteRegistry::load(&paths) {
            Ok(r) => r,
            Err(_) => return,
        };
        let _ = paths.ensure_dirs();
        let _ = VhostRenderer::new().render_all(&reg, &cfg, &paths);
        #[cfg(target_os = "macos")]
        let _ = sync_httpd_binds(&paths, &reg, &cfg);

        // Ensure every detected PHP runtime has its pool and php.ini before
        // starting it. This is needed for versions installed from the UI,
        // because CLI setup is not necessarily run afterward.
        #[cfg(target_os = "macos")]
        for version in php::detect_phps(&paths).unwrap_or_default() {
            let _ = php::write_fpm_pool_conf(&version.version, &paths, cfg.ram_profile);
            let _ = php::ensure_php_ini(&version.version, &paths);
        }

        #[cfg(target_os = "macos")]
        if !service::is_running(&paths.pid_file("dnsmasq")) {
            let _ = start_one(&paths, "dnsmasq");
        }
        if !service::is_running(&paths.pid_file("mysqld")) {
            let _ = start_one(&paths, "mysqld");
        }
        // macOS: start every installed PHP-FPM version, not only versions
        // referenced by current sites. This keeps all runtimes ready to use.
        #[cfg(target_os = "macos")]
        {
            let versions: std::collections::BTreeSet<String> = php::detect_phps(&paths)
                .unwrap_or_default()
                .into_iter()
                .map(|v| v.version)
                .chain(reg.sites.values().map(|s| s.php.clone()))
                .collect();
            for v in versions {
                let name = format!("{PHP_SVC_PREFIX}{v}");
                if !service::is_running(&paths.pid_file(&name)) {
                    let _ = start_one(&paths, &name);
                }
            }
        }
        if !service::is_running(&paths.pid_file("httpd")) {
            let _ = start_one(&paths, "httpd");
        }
    });
    Ok(())
}

#[tauri::command]
fn service_stop_all(state: State<'_, Mutex<AppState>>) -> Result<(), String> {
    let paths = state.lock().unwrap().paths.clone();
    // Stop each daemon independently so a slow mysqld shutdown cannot prevent
    // httpd, dnsmasq, or PHP-FPM from being stopped in the same click.
    std::thread::spawn(move || {
        let mut services = vec!["httpd".to_owned(), "mysqld".to_owned()];
        #[cfg(target_os = "macos")]
        services.push("dnsmasq".to_owned());

        if let Ok(rd) = std::fs::read_dir(paths.run_dir()) {
            services.extend(rd.flatten().filter_map(|entry| {
                let name = entry.file_name().to_string_lossy().to_string();
                (name.starts_with(PHP_SVC_PREFIX) && name.ends_with(".pid"))
                    .then(|| name.trim_end_matches(".pid").to_owned())
            }));
        }

        let candidates = services;
        let mut active_services = Vec::with_capacity(candidates.len());
        for name in candidates {
            let pid_file = paths.pid_file(&name);
            if !pid_file.exists() {
                continue;
            }
            if service::is_running(&pid_file) {
                active_services.push(name);
            } else {
                // Do not signal a stale/reused PID; it is safe to remove our
                // own dead service marker before the next status poll.
                let _ = std::fs::remove_file(pid_file);
            }
        }

        for name in active_services {
            let service_paths = paths.clone();
            std::thread::spawn(move || {
                let _ = stop_one(&service_paths, &name);
            });
        }
    });
    Ok(())
}

#[tauri::command]
fn service_start(state: State<'_, Mutex<AppState>>, name: String) -> Result<(), String> {
    let paths = state.lock().unwrap().paths.clone();
    std::thread::spawn(move || {
        let _ = start_one(&paths, &name);
    });
    Ok(())
}

#[tauri::command]
fn service_stop(state: State<'_, Mutex<AppState>>, name: String) -> Result<(), String> {
    let paths = state.lock().unwrap().paths.clone();
    std::thread::spawn(move || {
        let _ = stop_one(&paths, &name);
    });
    Ok(())
}

#[tauri::command]
fn get_logs(
    state: State<'_, Mutex<AppState>>,
    service: String,
    lines: u32,
) -> Result<String, String> {
    let paths = &state.lock().unwrap().paths;
    let file = paths.log_file(&service);
    if !file.exists() {
        return Ok(String::new());
    }
    let content = std::fs::read_to_string(&file).map_err(|e| e.to_string())?;
    let all: Vec<&str> = content.lines().collect();
    let start = all.len().saturating_sub(lines as usize);
    Ok(all[start..].join("\n"))
}

#[tauri::command]
fn get_config(state: State<'_, Mutex<AppState>>) -> Result<Config, String> {
    let paths = &state.lock().unwrap().paths;
    Config::load(paths).map_err(|e| e.to_string())
}

#[tauri::command]
fn save_config(state: State<'_, Mutex<AppState>>, config: Config) -> Result<(), String> {
    devstack_core::dns::validate_tld(&config.tld).map_err(|e| e.to_string())?;
    let paths = &state.lock().unwrap().paths;
    config.save(paths).map_err(|e| e.to_string())
}

// ── package manager ─────────────────────────────────────────────────────────

/// List packages: installed vendored packages + available PHP versions.
#[tauri::command]
fn get_packages(state: State<'_, Mutex<AppState>>) -> Result<Vec<PackageDto>, String> {
    let paths = &state.lock().unwrap().paths;

    #[cfg(target_os = "windows")]
    {
        let vdir = vendor::vendor_dir(paths);
        let installed = vendor::list_vendored();
        let mut out: Vec<PackageDto> = Vec::new();
        for name in &installed {
            let (kind, version) = split_package(name);
            out.push(PackageDto {
                name: name.clone(),
                kind,
                version,
                installed: true,
                path: vdir.join(name).display().to_string(),
            });
        }
        if let Ok(vers) = vendor::available_php_versions() {
            for v in vers {
                let name = format!("php-{v}");
                if !installed.contains(&name) {
                    out.push(PackageDto {
                        name: name.clone(),
                        kind: "php".into(),
                        version: v,
                        installed: false,
                        path: String::new(),
                    });
                }
            }
        }
        for core_pkg in ["mariadb", "httpd"] {
            if !installed.iter().any(|n| n == core_pkg) {
                let (kind, version) = split_package(core_pkg);
                out.push(PackageDto {
                    name: core_pkg.to_string(),
                    kind,
                    version,
                    installed: false,
                    path: String::new(),
                });
            }
        }
        return Ok(out);
    }

    #[cfg(target_os = "macos")]
    {
        let installed = vendor_mac::list_installed(paths);
        let mut out: Vec<PackageDto> = Vec::new();
        for name in &installed {
            let (kind, version) = split_package(name);
            out.push(PackageDto {
                name: name.clone(),
                kind,
                version,
                installed: true,
                path: paths.root.join("vendor").join(name).display().to_string(),
            });
        }
        // core packages offered for install when missing
        for core_pkg in ["mysql", "dnsmasq"] {
            if !installed.iter().any(|n| n == core_pkg) {
                let (kind, version) = split_package(core_pkg);
                out.push(PackageDto {
                    name: core_pkg.to_string(),
                    kind,
                    version,
                    installed: false,
                    path: String::new(),
                });
            }
        }
        // common PHP versions not yet installed
        for v in ["8.4", "8.3", "8.2"] {
            let name = format!("php-{v}");
            if !installed.contains(&name) {
                out.push(PackageDto {
                    name,
                    kind: "php".into(),
                    version: v.into(),
                    installed: false,
                    path: String::new(),
                });
            }
        }
        return Ok(out);
    }

    #[allow(unreachable_code)]
    Ok(vec![])
}

fn split_package(name: &str) -> (String, String) {
    if let Some(v) = name.strip_prefix("php-") {
        ("php".into(), v.into())
    } else if name == "mariadb" {
        ("mariadb".into(), "11.4.2".into())
    } else if name == "mysql" {
        ("mysql".into(), "8.4.5".into())
    } else if name == "dnsmasq" {
        ("dnsmasq".into(), "2.93".into())
    } else if name == "httpd" {
        ("httpd".into(), "2.4.69".into())
    } else {
        ("other".into(), String::new())
    }
}

/// Install a package in the background; emits "install-progress" events.
#[tauri::command]
fn install_package(
    app: AppHandle,
    state: State<'_, Mutex<AppState>>,
    name: String,
) -> Result<(), String> {
    let paths = state.lock().unwrap().paths.clone();
    let pkg = name.clone();
    std::thread::spawn(move || {
        let app2 = app.clone();
        let emit = |phase: &str, msg: String| {
            let _ = app2.emit(
                "install-progress",
                serde_json::json!({ "package": pkg, "phase": phase, "msg": msg }),
            );
        };
        #[cfg(target_os = "windows")]
        let result = vendor::install(&paths, &name, |p| match p {
            vendor::Progress::Resolving => emit("resolving", "resolving…".into()),
            vendor::Progress::Downloading { got, total } => {
                let mb = got as f64 / 1_048_576.0;
                match total {
                    Some(t) => emit(
                        "downloading",
                        format!("{mb:.1}/{:.1} MB", t as f64 / 1_048_576.0),
                    ),
                    None => emit("downloading", format!("{mb:.1} MB")),
                }
            }
            vendor::Progress::Extracting => emit("extracting", "extracting…".into()),
            vendor::Progress::Done(d) => emit("done", d.display().to_string()),
        });
        #[cfg(target_os = "macos")]
        let result = vendor_mac::install(&paths, &name, |p| match p {
            vendor_mac::Progress::Resolving => emit("resolving", "resolving…".into()),
            vendor_mac::Progress::Downloading { got, total } => {
                let mb = got as f64 / 1_048_576.0;
                match total {
                    Some(t) => emit(
                        "downloading",
                        format!("{mb:.1}/{:.1} MB", t as f64 / 1_048_576.0),
                    ),
                    None => emit("downloading", format!("{mb:.1} MB")),
                }
            }
            vendor_mac::Progress::Extracting => emit("extracting", "extracting…".into()),
            vendor_mac::Progress::Done(d) => emit("done", d.display().to_string()),
        });

        #[cfg(target_os = "windows")]
        if result.is_ok() && name.starts_with("php-") {
            let ver = name.trim_start_matches("php-");
            let _ = php_win::ensure_php_ini(ver, &paths);
        }

        match result {
            Ok(_) => emit("installed", format!("{name} installed")),
            Err(e) => emit("error", format!("{e}")),
        }
    });
    Ok(())
}

/// Remove an installed vendored package.
#[tauri::command]
fn uninstall_package(state: State<'_, Mutex<AppState>>, name: String) -> Result<(), String> {
    let paths = &state.lock().unwrap().paths;
    // refuse to remove a php version that's still referenced by a site
    if name.starts_with("php-") {
        let ver = name.trim_start_matches("php-");
        if let Ok(reg) = SiteRegistry::load(paths) {
            if reg.sites.values().any(|s| s.php == ver) {
                return Err(format!("php-{ver} is in use by a site — reassign it first"));
            }
        }
    }
    let dir = paths.root.join("vendor").join(&name);
    if dir.exists() {
        std::fs::remove_dir_all(&dir).map_err(|e| e.to_string())?;
    }
    Ok(())
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    let paths = Paths::detect().expect("cannot resolve ~/.devstack");
    paths.ensure_dirs().ok();

    let mut builder = tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_dialog::init())
        .manage(Mutex::new(AppState { paths }))
        .manage(MetricsState(Mutex::new(metrics::Sampler::new())));
    #[cfg(target_os = "macos")]
    {
        builder = builder.manage(SetupGuard(AtomicBool::new(false)));
    }
    builder
        .invoke_handler(tauri::generate_handler![
            get_system_metrics,
            #[cfg(target_os = "macos")]
            get_setup_status,
            #[cfg(target_os = "macos")]
            run_setup,
            get_status,
            get_sites,
            get_php_versions,
            add_site,
            remove_site,
            set_site_php,
            set_site_port,
            service_start,
            service_stop,
            service_start_all,
            service_stop_all,
            get_logs,
            get_config,
            save_config,
            get_packages,
            install_package,
            uninstall_package,
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}

fn main() {
    run();
}
