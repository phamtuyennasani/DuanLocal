use devstack_core::{
    config::Config,
    dns,
    httpd,
    mysql,
    paths::Paths,
    php,
    service,
    site::{Site, SiteRegistry},
    vhost::VhostRenderer,
};
use serde::Serialize;
use std::sync::Mutex;
use tauri::{Manager, State};

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

fn status_of(paths: &Paths, name: &str) -> ServiceStatus {
    ServiceStatus {
        name: name.to_string(),
        running: service::is_running(&paths.pid_file(name)),
    }
}

#[tauri::command]
fn get_status(state: State<'_, Mutex<AppState>>) -> Vec<ServiceStatus> {
    let paths = &state.lock().unwrap().paths;
    let mut out = vec![
        status_of(paths, "dnsmasq"),
        status_of(paths, "mysqld"),
        status_of(paths, "httpd"),
    ];
    // php-fpm per-version pid files
    if let Ok(rd) = std::fs::read_dir(paths.run_dir()) {
        for e in rd.flatten() {
            let n = e.file_name().to_string_lossy().to_string();
            if n.starts_with("php-fpm-") && n.ends_with(".pid") {
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
    php::detect_brew_phps(paths)
        .unwrap_or_default()
        .iter()
        .map(|v| v.version.clone())
        .collect()
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
        name,
        php,
        root: root.into(),
        https,
    })
    .map_err(|e| e.to_string())?;
    reg.save(paths).map_err(|e| e.to_string())?;
    VhostRenderer::new()
        .render_all(&reg, &cfg, paths)
        .map_err(|e| e.to_string())
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
        .map_err(|e| e.to_string())
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

#[tauri::command]
fn service_start_all(state: State<'_, Mutex<AppState>>) -> Result<(), String> {
    let paths = &state.lock().unwrap().paths;
    let cfg = Config::load(paths).map_err(|e| e.to_string())?;
    let reg = SiteRegistry::load(paths).map_err(|e| e.to_string())?;
    paths.ensure_dirs().map_err(|e| e.to_string())?;

    VhostRenderer::new()
        .render_all(&reg, &cfg, paths)
        .map_err(|e| e.to_string())?;

    if !service::is_running(&paths.pid_file("dnsmasq")) {
        dns::start_dnsmasq(paths).map_err(|e| e.to_string())?;
    }
    if !service::is_running(&paths.pid_file("mysqld")) {
        mysql::start_mysqld(paths).map_err(|e| e.to_string())?;
    }
    let versions: std::collections::BTreeSet<String> =
        reg.sites.values().map(|s| s.php.clone()).collect();
    for v in versions {
        if !service::is_running(&paths.pid_file(&format!("php-fpm-{v}"))) {
            php::start_fpm(&v, paths).map_err(|e| e.to_string())?;
        }
    }
    if !service::is_running(&paths.pid_file("httpd")) {
        httpd::start_httpd(paths).map_err(|e| e.to_string())?;
    }
    Ok(())
}

#[tauri::command]
fn service_stop_all(state: State<'_, Mutex<AppState>>) -> Result<(), String> {
    let paths = &state.lock().unwrap().paths;
    let _ = httpd::stop_httpd(paths);
    let _ = mysql::stop_mysqld(paths);
    let _ = dns::stop_dnsmasq(paths);
    if let Ok(rd) = std::fs::read_dir(paths.run_dir()) {
        for e in rd.flatten() {
            let n = e.file_name().to_string_lossy();
            if n.starts_with("php-fpm-") && n.ends_with(".pid") {
                let _ = service::stop_daemon(&e.path(), 5);
            }
        }
    }
    Ok(())
}

#[tauri::command]
fn service_start(state: State<'_, Mutex<AppState>>, name: String) -> Result<(), String> {
    let paths = &state.lock().unwrap().paths;
    match name.as_str() {
        "dnsmasq" => dns::start_dnsmasq(paths).map_err(|e| e.to_string()),
        "mysqld" => mysql::start_mysqld(paths).map_err(|e| e.to_string()),
        "httpd" => httpd::start_httpd(paths).map_err(|e| e.to_string()),
        n if n.starts_with("php-fpm-") => {
            let v = n.trim_start_matches("php-fpm-");
            php::start_fpm(v, paths).map_err(|e| e.to_string())
        }
        _ => Err(format!("unknown service: {name}")),
    }
}

#[tauri::command]
fn service_stop(state: State<'_, Mutex<AppState>>, name: String) -> Result<(), String> {
    let paths = &state.lock().unwrap().paths;
    match name.as_str() {
        "dnsmasq" => dns::stop_dnsmasq(paths).map_err(|e| e.to_string()),
        "mysqld" => mysql::stop_mysqld(paths).map_err(|e| e.to_string()),
        "httpd" => httpd::stop_httpd(paths).map_err(|e| e.to_string()),
        n if n.starts_with("php-fpm-") => {
            let v = n.trim_start_matches("php-fpm-");
            php::stop_fpm(v, paths).map_err(|e| e.to_string())
        }
        _ => Err(format!("unknown service: {name}")),
    }
}

#[tauri::command]
fn get_logs(state: State<'_, Mutex<AppState>>, service: String, lines: u32) -> Result<String, String> {
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
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
