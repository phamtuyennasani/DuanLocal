// Prevents a console window on release Windows builds.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use devstack_core::{
    config::Config,
    paths::Paths,
    service,
    site::{Site, SiteRegistry},
    vhost::VhostRenderer,
    vendor,
};
use serde::Serialize;
use std::sync::Mutex;
use tauri::{AppHandle, Emitter, State};

#[cfg(target_os = "macos")]
use devstack_core::{dns, httpd, php};
#[cfg(target_os = "windows")]
use devstack_core::{hosts, httpd_win, mysql, php_win};

struct AppState {
    paths: Paths,
}

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
    https: bool,
}

#[derive(Serialize)]
struct PackageDto {
    name: String,        // "php-8.3" | "mariadb" | "httpd"
    kind: String,        // "php" | "mariadb" | "httpd"
    version: String,     // "8.3" | "11.4.2" | "2.4.69"
    installed: bool,
    path: String,        // vendor dir if installed
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
    // per-version php service pid files
    if let Ok(rd) = std::fs::read_dir(paths.run_dir()) {
        for e in rd.flatten() {
            let n = e.file_name().to_string_lossy().to_string();
            if n.starts_with(PHP_SVC_PREFIX) && n.ends_with(".pid") {
                let name = n.trim_end_matches(".pid").to_string();
                out.push(ServiceStatus {
                    running: service::is_running(&e.path()),
                    name,
                });
            }
        }
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
    https: bool,
) -> Result<(), String> {
    let paths = &state.lock().unwrap().paths;
    let cfg = Config::load(paths).map_err(|e| e.to_string())?;
    let mut reg = SiteRegistry::load(paths).map_err(|e| e.to_string())?;
    reg.add(Site {
        name: name.clone(),
        php,
        root: root.into(),
        https,
    })
    .map_err(|e| e.to_string())?;
    reg.save(paths).map_err(|e| e.to_string())?;
    VhostRenderer::new()
        .render_all(&reg, &cfg, paths)
        .map_err(|e| e.to_string())?;

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
    reg.remove(&name).map_err(|e| e.to_string())?;
    reg.save(paths).map_err(|e| e.to_string())?;
    let _ = std::fs::remove_file(paths.vhosts_dir().join(format!("{name}.conf")));
    VhostRenderer::new()
        .render_all(&reg, &cfg, paths)
        .map_err(|e| e.to_string())?;

    #[cfg(target_os = "windows")]
    {
        let _ = hosts::remove_site(&name, &cfg.tld);
        hosts::flush_dns();
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
    let paths = &state.lock().unwrap().paths;
    let cfg = Config::load(paths).map_err(|e| e.to_string())?;
    let reg = SiteRegistry::load(paths).map_err(|e| e.to_string())?;
    paths.ensure_dirs().map_err(|e| e.to_string())?;

    VhostRenderer::new()
        .render_all(&reg, &cfg, paths)
        .map_err(|e| e.to_string())?;

    #[cfg(target_os = "macos")]
    if !service::is_running(&paths.pid_file("dnsmasq")) {
        start_one(paths, "dnsmasq")?;
    }
    if !service::is_running(&paths.pid_file("mysqld")) {
        start_one(paths, "mysqld")?;
    }
    // macOS: php-fpm per-version. Windows: php runs as CGI — no daemon needed.
    #[cfg(target_os = "macos")]
    {
        let versions: std::collections::BTreeSet<String> =
            reg.sites.values().map(|s| s.php.clone()).collect();
        for v in versions {
            let name = format!("{PHP_SVC_PREFIX}{v}");
            if !service::is_running(&paths.pid_file(&name)) {
                start_one(paths, &name)?;
            }
        }
    }
    if !service::is_running(&paths.pid_file("httpd")) {
        start_one(paths, "httpd")?;
    }
    Ok(())
}

#[tauri::command]
fn service_stop_all(state: State<'_, Mutex<AppState>>) -> Result<(), String> {
    let paths = &state.lock().unwrap().paths;
    #[cfg(target_os = "macos")]
    let _ = dns::stop_dnsmasq(paths);
    let _ = httpd_stop(paths);
    let _ = mysql::stop_mysqld(paths);
    if let Ok(rd) = std::fs::read_dir(paths.run_dir()) {
        for e in rd.flatten() {
            let n = e.file_name().to_string_lossy().to_string();
            if n.starts_with(PHP_SVC_PREFIX) && n.ends_with(".pid") {
                let _ = service::stop_daemon(&e.path(), 5);
            }
        }
    }
    Ok(())
}

fn httpd_stop(paths: &Paths) -> Result<(), String> {
    #[cfg(target_os = "macos")]
    return httpd::stop_httpd(paths).map_err(|e| e.to_string());
    #[cfg(target_os = "windows")]
    return httpd_win::stop_httpd(paths).map_err(|e| e.to_string());
}

#[tauri::command]
fn service_start(state: State<'_, Mutex<AppState>>, name: String) -> Result<(), String> {
    let paths = &state.lock().unwrap().paths;
    start_one(paths, &name)
}

#[tauri::command]
fn service_stop(state: State<'_, Mutex<AppState>>, name: String) -> Result<(), String> {
    let paths = &state.lock().unwrap().paths;
    stop_one(paths, &name)
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
    let paths = &state.lock().unwrap().paths;
    config.save(paths).map_err(|e| e.to_string())
}

// ── package manager (Windows) ────────────────────────────────────────────────

/// List packages: installed vendored packages + available PHP versions.
#[tauri::command]
fn get_packages(state: State<'_, Mutex<AppState>>) -> Result<Vec<PackageDto>, String> {
    let paths = &state.lock().unwrap().paths;
    let vdir = vendor::vendor_dir(paths);
    let installed = vendor::list_vendored();

    let mut out: Vec<PackageDto> = Vec::new();

    // installed packages first
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

    // available PHP versions not yet installed (Windows only — macOS uses brew)
    #[cfg(target_os = "windows")]
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

    // offer core packages that aren't installed
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

    Ok(out)
}

fn split_package(name: &str) -> (String, String) {
    if let Some(v) = name.strip_prefix("php-") {
        ("php".into(), v.into())
    } else if name == "mariadb" {
        ("mariadb".into(), "11.4.2".into())
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
        let result = vendor::install(&paths, &name, |p| match p {
            vendor::Progress::Resolving => emit("resolving", "resolving…".into()),
            vendor::Progress::Downloading { got, total } => {
                let mb = got as f64 / 1_048_576.0;
                match total {
                    Some(t) => emit("downloading", format!("{mb:.1}/{:.1} MB", t as f64 / 1_048_576.0)),
                    None => emit("downloading", format!("{mb:.1} MB")),
                }
            }
            vendor::Progress::Extracting => emit("extracting", "extracting…".into()),
            vendor::Progress::Done(d) => emit("done", d.display().to_string()),
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
fn uninstall_package(
    state: State<'_, Mutex<AppState>>,
    name: String,
) -> Result<(), String> {
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
    let dir = vendor::vendor_dir(paths).join(&name);
    if dir.exists() {
        std::fs::remove_dir_all(&dir).map_err(|e| e.to_string())?;
    }
    Ok(())
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    let paths = Paths::detect().expect("cannot resolve ~/.devstack");
    paths.ensure_dirs().ok();

    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .manage(Mutex::new(AppState { paths }))
        .invoke_handler(tauri::generate_handler![
            get_status,
            get_sites,
            get_php_versions,
            add_site,
            remove_site,
            set_site_php,
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
