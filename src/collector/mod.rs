//! Metric collection.
//!
//! The [`Collector`] owns every handle needed to read system metrics and is only ever
//! used from the monitoring thread, so it needs no locking. It produces a [`Snapshot`]
//! per tick, an immutable value that the UI thread can keep without worrying about
//! concurrent mutation.

pub mod cpu;
pub mod memory;
pub mod network;
pub mod processes;
pub mod storage;
pub mod system;

use std::time::{Duration, Instant};

use sysinfo::{Components, System};

use self::cpu::CpuCollector;
use self::memory::MemoryCollector;
use self::processes::ProcessCollector;
use self::storage::StorageCollector;
use self::system::{DynamicInfo, SystemCollector, SystemInfo};

/// Everything the UI needs to draw one frame of data.
#[derive(Debug, Clone, Default)]
pub struct Snapshot {
    /// Monotonically increasing counter, used to detect missed ticks.
    pub sequence: u64,
    /// Time elapsed since the previous snapshot.
    pub interval: Duration,
    /// Wall clock time of the sample, in milliseconds since the UNIX epoch.
    pub timestamp_millis: u64,
    /// CPU statistics.
    pub cpu: cpu::CpuSnapshot,
    /// Memory statistics.
    pub memory: memory::MemorySnapshot,
    /// Process table.
    pub processes: processes::ProcessList,
    /// Mounted filesystems.
    pub storage: Vec<storage::FilesystemInfo>,
    /// Network interface statistics.
    pub network: network::NetworkSnapshot,
    /// Static system information.
    pub system: SystemInfo,
    /// Slowly changing system information.
    pub dynamic: DynamicInfo,
    /// Non fatal problems encountered while sampling, shown in the status bar.
    pub warnings: Vec<String>,
}

impl Snapshot {
    /// Whether at least one process was collected.
    pub fn has_processes(&self) -> bool {
        !self.processes.processes.is_empty()
    }

    /// Whether any filesystem was collected.
    pub fn has_storage(&self) -> bool {
        !self.storage.is_empty()
    }

    /// Whether any network interface was collected.
    pub fn has_network(&self) -> bool {
        !self.network.interfaces.is_empty()
    }
}

/// Tuning knobs for the monitoring thread.
#[derive(Debug, Clone)]
pub struct CollectorConfig {
    /// Minimum delay between two samples.
    pub interval: Duration,
    /// How often the slowly changing information (disks, sensors, ...) is refreshed,
    /// expressed in samples. `1` means every sample.
    pub slow_refresh_every: u32,
    /// Keep kernel pseudo filesystems in the storage list.
    pub show_pseudo_filesystems: bool,
    /// Restrict network monitoring to these interfaces. Empty means "all".
    pub network_interfaces: Vec<String>,
    /// Whether to collect the process table.
    pub collect_processes: bool,
}

impl Default for CollectorConfig {
    fn default() -> Self {
        Self {
            interval: Duration::from_secs(1),
            slow_refresh_every: 5,
            show_pseudo_filesystems: false,
            network_interfaces: Vec::new(),
            collect_processes: true,
        }
    }
}

/// Owns the `sysinfo` handles and turns them into snapshots.
pub struct Collector {
    system: System,
    components: Components,
    cpu: CpuCollector,
    memory: MemoryCollector,
    processes: ProcessCollector,
    storage: StorageCollector,
    network: network::NetworkCollector,
    system_info: SystemCollector,
    config: CollectorConfig,
    sequence: u64,
    last_sample: Option<Instant>,
    cached_system: SystemInfo,
    cached_storage: Vec<storage::FilesystemInfo>,
    cached_dynamic: DynamicInfo,
    slow_countdown: u32,
    last_dynamic: DynamicInfo,
}

impl Collector {
    /// Creates a collector and performs the priming refreshes.
    pub fn new(config: CollectorConfig) -> Self {
        let mut system = System::new();
        system.refresh_cpu_usage();
        system.refresh_memory();

        let components = Components::new_with_refreshed_list();
        let system_info_collector = SystemCollector::default();
        let static_info = system_info_collector.static_info(&system);

        let cpu = CpuCollector::new(&mut system, cpu::PROC_STAT);
        let memory = MemoryCollector::default();
        let processes = ProcessCollector::new(&config);
        let storage = StorageCollector::new(config.show_pseudo_filesystems);
        let network = network::NetworkCollector::new(config.network_interfaces.clone());

        Self {
            system,
            components,
            cpu,
            memory,
            processes,
            storage,
            network,
            system_info: system_info_collector,
            config,
            sequence: 0,
            last_sample: None,
            cached_system: static_info,
            cached_storage: Vec::new(),
            cached_dynamic: DynamicInfo::default(),
            slow_countdown: 1,
            last_dynamic: DynamicInfo::default(),
        }
    }

    /// Collects a full snapshot.
    ///
    /// The expensive, rarely changing parts (filesystems, sensors, process table) are
    /// refreshed on [`CollectorConfig::slow_refresh_every`] samples, while the cheap
    /// and volatile parts run on every call.
    pub fn sample(&mut self) -> Snapshot {
        let now = Instant::now();
        let interval = self
            .last_sample
            .map_or(self.config.interval, |last| now.duration_since(last));
        self.last_sample = Some(now);
        self.sequence += 1;
        self.slow_countdown = self.slow_countdown.saturating_sub(1);

        let mut warnings = Vec::new();
        let mut cpu_snapshot = self.cpu.sample(&mut self.system);
        let memory_snapshot = self.memory.sample(&mut self.system);

        // The slow cadence covers everything that does not change tick to tick: the
        // mount table, the sensors, and the passwd-backed user names.
        let slow_tick = self.slow_countdown == 0;
        if slow_tick {
            self.slow_countdown = self.config.slow_refresh_every.max(1);
            self.cached_storage = self.storage.sample();
            self.cached_dynamic = self.system_info.dynamic_info(&mut self.components);
            self.cached_system = self.system_info.static_info(&self.system);
            if self.cached_dynamic.temperatures.is_empty() {
                warnings.push("temperature sensors unavailable".to_string());
            }
        }

        // The package temperature is the most representative reading; fall back to any
        // core sensor when the machine does not expose a package sensor.
        if cpu_snapshot.temperature.is_none() {
            cpu_snapshot.temperature = self
                .cached_dynamic
                .temperatures
                .iter()
                .find(|temp| is_package_label(&temp.label))
                .or_else(|| self.cached_dynamic.temperatures.first())
                .map(|temp| f64::from(temp.celsius));
        }
        self.last_dynamic = self.cached_dynamic.clone();

        let processes = if self.config.collect_processes {
            self.processes.sample(&mut self.system, slow_tick)
        } else {
            processes::ProcessList::default()
        };
        let network_snapshot = self.network.sample();

        Snapshot {
            sequence: self.sequence,
            interval,
            timestamp_millis: now_millis(),
            cpu: cpu_snapshot,
            memory: memory_snapshot,
            processes,
            storage: self.cached_storage.clone(),
            network: network_snapshot,
            system: self.cached_system.clone(),
            dynamic: self.last_dynamic.clone(),
            warnings,
        }
    }
}

/// Whether a temperature label looks like a CPU package sensor.
fn is_package_label(label: &str) -> bool {
    let lower = label.to_ascii_lowercase();
    lower.contains("package") || lower.contains("tdie") || lower.contains("tctl")
}

/// Wall clock time in milliseconds since the UNIX epoch (0 on failure).
fn now_millis() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|elapsed| u64::try_from(elapsed.as_millis()).unwrap_or(u64::MAX))
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn package_labels_are_recognized() {
        assert!(is_package_label("Package id 0"));
        assert!(is_package_label("Tdie"));
        assert!(!is_package_label("acpitz"));
    }

    #[test]
    fn snapshot_defaults_are_empty() {
        let snapshot = Snapshot::default();
        assert!(!snapshot.has_processes());
        assert!(!snapshot.has_storage());
        assert!(!snapshot.has_network());
    }

    #[test]
    fn collector_produces_a_complete_snapshot() {
        let config = CollectorConfig {
            interval: Duration::from_millis(200),
            slow_refresh_every: 1,
            ..CollectorConfig::default()
        };
        let mut collector = Collector::new(config);
        let snapshot = collector.sample();
        assert!(snapshot.cpu.logical_cores > 0);
        assert!(snapshot.memory.total > 0);
        assert!(!snapshot.system.hostname.is_empty());
        assert_eq!(snapshot.sequence, 1);

        let second = collector.sample();
        assert_eq!(second.sequence, 2);
        assert!(second.cpu.times.valid || second.cpu.usage >= 0.0);
    }
}
