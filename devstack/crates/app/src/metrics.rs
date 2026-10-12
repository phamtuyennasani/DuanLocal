use serde::Serialize;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};
use sysinfo::{Disks, Networks, System};

const DISK_REFRESH_INTERVAL: Duration = Duration::from_secs(30);

#[derive(Debug, Serialize)]
pub struct SystemMetrics {
    pub timestamp_ms: u64,
    pub cpu_usage_pct: f32,
    pub cpu_cores: usize,
    pub load_average: Option<LoadAverage>,
    pub memory: MemoryMetrics,
    pub storage: Vec<StorageMetrics>,
    pub network: NetworkMetrics,
}

#[derive(Debug, Serialize)]
pub struct LoadAverage {
    pub one_minute: f64,
    pub five_minutes: f64,
    pub fifteen_minutes: f64,
}

#[derive(Debug, Serialize)]
pub struct MemoryMetrics {
    pub total_bytes: u64,
    pub used_bytes: u64,
    pub available_bytes: u64,
    pub swap_total_bytes: u64,
    pub swap_used_bytes: u64,
    pub pressure_pct: f32,
}

#[derive(Debug, Serialize)]
pub struct StorageMetrics {
    pub name: String,
    pub mount: String,
    pub total_bytes: u64,
    pub available_bytes: u64,
    pub primary: bool,
}

#[derive(Debug, Serialize)]
pub struct NetworkMetrics {
    pub rx_bytes_per_sec: u64,
    pub tx_bytes_per_sec: u64,
}

/// Stateful, on-demand system sampler. The state lets sysinfo calculate CPU and
/// network deltas without a permanent background worker.
pub struct Sampler {
    system: System,
    networks: Networks,
    disks: Disks,
    last_network_refresh: Instant,
    last_disk_refresh: Instant,
}

impl Sampler {
    pub fn new() -> Self {
        let mut system = System::new();
        system.refresh_cpu_usage();
        system.refresh_memory();

        Self {
            system,
            networks: Networks::new_with_refreshed_list(),
            disks: Disks::new_with_refreshed_list(),
            last_network_refresh: Instant::now(),
            last_disk_refresh: Instant::now(),
        }
    }

    pub fn sample(&mut self) -> SystemMetrics {
        self.system.refresh_cpu_usage();
        self.system.refresh_memory();

        let now = Instant::now();
        let elapsed_secs = now
            .saturating_duration_since(self.last_network_refresh)
            .as_secs_f64()
            .max(0.001);
        self.networks.refresh(true);
        self.last_network_refresh = now;

        if now.saturating_duration_since(self.last_disk_refresh) >= DISK_REFRESH_INTERVAL {
            self.disks.refresh(true);
            self.last_disk_refresh = now;
        }

        let received_delta: u64 = self
            .networks
            .iter()
            .filter(|(name, _)| is_counted_network(name))
            .map(|(_, network)| network.received())
            .sum();
        let transmitted_delta: u64 = self
            .networks
            .iter()
            .filter(|(name, _)| is_counted_network(name))
            .map(|(_, network)| network.transmitted())
            .sum();

        let memory_total = self.system.total_memory();
        let memory_available = self.system.available_memory().min(memory_total);
        let memory_used = self.system.used_memory().min(memory_total);
        let load_average = load_average();

        SystemMetrics {
            timestamp_ms: SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap_or_default()
                .as_millis() as u64,
            cpu_usage_pct: self.system.global_cpu_usage().clamp(0.0, 100.0),
            cpu_cores: self.system.cpus().len(),
            load_average,
            memory: MemoryMetrics {
                total_bytes: memory_total,
                used_bytes: memory_used,
                available_bytes: memory_available,
                swap_total_bytes: self.system.total_swap(),
                swap_used_bytes: self.system.used_swap().min(self.system.total_swap()),
                pressure_pct: percent(memory_used, memory_total),
            },
            storage: storage_metrics(&self.disks),
            network: NetworkMetrics {
                rx_bytes_per_sec: (received_delta as f64 / elapsed_secs) as u64,
                tx_bytes_per_sec: (transmitted_delta as f64 / elapsed_secs) as u64,
            },
        }
    }
}

fn load_average() -> Option<LoadAverage> {
    #[cfg(target_os = "windows")]
    {
        None
    }

    #[cfg(not(target_os = "windows"))]
    {
        let load = System::load_average();
        Some(LoadAverage {
            one_minute: load.one,
            five_minutes: load.five,
            fifteen_minutes: load.fifteen,
        })
    }
}

fn is_counted_network(name: &str) -> bool {
    let lower = name.to_ascii_lowercase();
    !matches!(lower.as_str(), "lo" | "lo0")
        && !lower.starts_with("utun")
        && !lower.starts_with("awdl")
        && !lower.starts_with("bridge")
        && !lower.starts_with("docker")
        && !lower.starts_with("veth")
}

fn storage_metrics(disks: &Disks) -> Vec<StorageMetrics> {
    let storage = disks
        .list()
        .iter()
        .filter(|disk| disk.total_space() > 0)
        .map(|disk| StorageMetrics {
            name: disk.name().to_string_lossy().to_string(),
            mount: disk.mount_point().display().to_string(),
            total_bytes: disk.total_space(),
            available_bytes: disk.available_space(),
            primary: false,
        })
        .collect();

    normalize_storage(storage)
}

fn normalize_storage(mut storage: Vec<StorageMetrics>) -> Vec<StorageMetrics> {
    #[cfg(target_os = "macos")]
    {
        let has_data_volume = storage
            .iter()
            .any(|disk| disk.mount == "/System/Volumes/Data");
        storage.retain(|disk| {
            disk.mount == "/System/Volumes/Data"
                || !disk.mount.starts_with("/System/Volumes/")
                    && (!has_data_volume || disk.mount != "/")
        });
    }

    let primary_index = storage
        .iter()
        .position(is_primary_mount)
        .or_else(|| (!storage.is_empty()).then_some(0));
    if let Some(index) = primary_index {
        storage[index].primary = true;
    }

    storage.sort_by(|left, right| {
        (!left.primary, left.mount.as_str()).cmp(&(!right.primary, right.mount.as_str()))
    });
    storage.truncate(4);
    storage
}

fn is_primary_mount(disk: &StorageMetrics) -> bool {
    #[cfg(target_os = "macos")]
    {
        disk.mount == "/System/Volumes/Data" || disk.mount == "/"
    }

    #[cfg(target_os = "windows")]
    {
        let mount = disk.mount.to_ascii_lowercase();
        mount == "c:" || mount.starts_with("c:\\") || mount.starts_with("c:/")
    }

    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    {
        disk.mount == "/"
    }
}

fn percent(value: u64, total: u64) -> f32 {
    if total == 0 {
        0.0
    } else {
        (value as f64 / total as f64 * 100.0).clamp(0.0, 100.0) as f32
    }
}

#[cfg(test)]
mod tests {
    use super::percent;

    #[test]
    fn percent_handles_zero_total_and_clamps_values() {
        assert_eq!(percent(1, 0), 0.0);
        assert_eq!(percent(50, 100), 50.0);
        assert_eq!(percent(200, 100), 100.0);
    }
}
