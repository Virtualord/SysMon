//! Memory and swap sampling.
//!
//! ## How "used" memory is calculated
//!
//! On Linux the kernel does not track "used" RAM directly, it tracks the size of the
//! page cache. A page cache page still occupies RAM but can be reclaimed at any time,
//! so counting it as "used" makes a healthy system look full. Every tool in this
//! family therefore derives the figure from [`MemInfo::available`]:
//!
//! ```text
//! used = MemTotal - MemAvailable        (this is what sysinfo does as well)
//! used% = used / MemTotal * 100
//! ```
//!
//! `MemAvailable` is the kernel's own estimate (Linux 3.14+) of how much memory a new
//! workload could obtain without swapping. On kernels older than 3.14 the field does
//! not exist and the collector falls back to `MemFree + Buffers + Cached`.
//!
//! Swap is reported the same way: `swap_used = SwapTotal - SwapFree`.

use std::path::{Path, PathBuf};

use sysinfo::System;

/// Path of the kernel memory statistics file.
const PROC_MEMINFO: &str = "/proc/meminfo";

/// Raw fields of `/proc/meminfo`, already converted from kB to bytes.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct MemInfo {
    /// `MemTotal` – physical RAM installed.
    pub total: u64,
    /// `MemFree` – completely untouched pages.
    pub free: u64,
    /// `MemAvailable` – memory available to new workloads.
    pub available: u64,
    /// `MemAvailable` was present in the file.
    pub available_present: bool,
    /// `Buffers` – block I/O buffers.
    pub buffers: u64,
    /// `Cached` – page cache, minus `SReclaimable`.
    pub cached: u64,
    /// `SReclaimable` – slab pages that can be reclaimed (part of `Cached`).
    pub reclaimable: u64,
    /// `Shmem` – shared memory / tmpfs.
    pub shmem: u64,
    /// `SwapTotal` – configured swap.
    pub swap_total: u64,
    /// `SwapFree` – unused swap.
    pub swap_free: u64,
    /// `SwapCached` – pages that are also in RAM.
    pub swap_cached: u64,
}

impl MemInfo {
    /// `MemTotal - MemAvailable`, or an estimate on kernels without `MemAvailable`.
    pub fn used(&self) -> u64 {
        self.total.saturating_sub(self.available)
    }

    /// `SwapTotal - SwapFree`.
    pub fn swap_used(&self) -> u64 {
        self.swap_total.saturating_sub(self.swap_free)
    }
}

/// Parses a whole `/proc/meminfo` document.
///
/// Values are reported in kibibytes by the kernel and stored here in bytes. Unknown
/// keys are ignored so the parser keeps working when the kernel grows new fields.
pub fn parse_meminfo(text: &str) -> MemInfo {
    let mut info = MemInfo::default();
    for line in text.lines() {
        let Some((key, rest)) = line.split_once(':') else {
            continue;
        };
        // Format is `Key:   12345 kB`; the unit is not always present (e.g. HugePages_Total).
        let value: u64 = rest
            .split_whitespace()
            .next()
            .and_then(|v| v.parse().ok())
            .unwrap_or(0);
        let value = value.saturating_mul(1024);
        match key.trim() {
            "MemTotal" => info.total = value,
            "MemFree" => info.free = value,
            "MemAvailable" => {
                info.available = value;
                info.available_present = true;
            }
            "Buffers" => info.buffers = value,
            "Cached" => info.cached = value,
            "SReclaimable" => info.reclaimable = value,
            "Shmem" => info.shmem = value,
            "SwapTotal" => info.swap_total = value,
            "SwapFree" => info.swap_free = value,
            "SwapCached" => info.swap_cached = value,
            _ => {}
        }
    }
    if !info.available_present && info.total > 0 {
        // Pre-3.14 kernels: estimate the reclaimable part the classic way.
        info.available = info
            .free
            .saturating_add(info.buffers)
            .saturating_add(info.cached);
    }
    info
}

/// A complete memory sample.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct MemorySnapshot {
    /// Physical RAM installed.
    pub total: u64,
    /// `total - available`.
    pub used: u64,
    /// Memory available to new workloads.
    pub available: u64,
    /// Completely unused pages.
    pub free: u64,
    /// Block I/O buffers (`Buffers`), when exposed.
    pub buffers: Option<u64>,
    /// Page cache (`Cached`), when exposed.
    pub cached: Option<u64>,
    /// Reclaimable slab (`SReclaimable`), when exposed.
    pub reclaimable: Option<u64>,
    /// Shared memory / tmpfs (`Shmem`), when exposed.
    pub shmem: Option<u64>,
    /// Configured swap.
    pub swap_total: u64,
    /// Used swap.
    pub swap_used: u64,
    /// Free swap.
    pub swap_free: u64,
    /// Pages that are in RAM *and* in swap, when exposed.
    pub swap_cached: Option<u64>,
}

impl MemorySnapshot {
    /// RAM utilization in percent, 0.0 - 100.0.
    pub fn used_percent(&self) -> f64 {
        if self.total == 0 {
            0.0
        } else {
            (self.used as f64 / self.total as f64 * 100.0).clamp(0.0, 100.0)
        }
    }

    /// Swap utilization in percent, 0.0 - 100.0. Zero when no swap is configured.
    pub fn swap_used_percent(&self) -> f64 {
        if self.swap_total == 0 {
            0.0
        } else {
            (self.swap_used as f64 / self.swap_total as f64 * 100.0).clamp(0.0, 100.0)
        }
    }

    /// Whether the system has any swap configured at all.
    pub fn has_swap(&self) -> bool {
        self.swap_total > 0
    }
}

/// Collects memory statistics from `sysinfo` plus `/proc/meminfo` details.
pub struct MemoryCollector {
    proc_meminfo: PathBuf,
}

impl Default for MemoryCollector {
    fn default() -> Self {
        Self::new(PROC_MEMINFO)
    }
}

impl MemoryCollector {
    /// Creates a collector reading extra details from `path`.
    pub fn new(proc_meminfo: impl Into<PathBuf>) -> Self {
        Self {
            proc_meminfo: proc_meminfo.into(),
        }
    }

    /// Reads a fresh memory sample.
    ///
    /// `sysinfo` is the primary source because it already handles kernels without
    /// `MemAvailable`. `/proc/meminfo` is parsed afterwards to enrich the snapshot
    /// with the cache and buffer breakdown; if that read fails the corresponding
    /// fields simply stay `None` and the UI shows `N/A`.
    pub fn sample(&self, system: &mut System) -> MemorySnapshot {
        system.refresh_memory();

        let details = read_meminfo(&self.proc_meminfo);
        MemorySnapshot {
            total: system.total_memory(),
            used: system.used_memory(),
            available: system.available_memory(),
            free: system.free_memory(),
            buffers: details.map(|info| info.buffers),
            cached: details.map(|info| info.cached),
            reclaimable: details.map(|info| info.reclaimable),
            shmem: details.map(|info| info.shmem),
            swap_total: system.total_swap(),
            swap_used: system.used_swap(),
            swap_free: system.free_swap(),
            swap_cached: details.map(|info| info.swap_cached),
        }
    }
}

/// Reads and parses `/proc/meminfo`, returning `None` when unavailable.
pub fn read_meminfo(path: &Path) -> Option<MemInfo> {
    match std::fs::read_to_string(path) {
        Ok(text) => {
            let info = parse_meminfo(&text);
            (info.total > 0).then_some(info)
        }
        Err(err) => {
            tracing::debug!(path = %path.display(), %err, "cannot read meminfo");
            None
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = "\
MemTotal:       16000000 kB
MemFree:         1000000 kB
MemAvailable:    8000000 kB
Buffers:          500000 kB
Cached:          6000000 kB
SwapCached:       100000 kB
Shmem:            200000 kB
SReclaimable:     300000 kB
SwapTotal:       4000000 kB
SwapFree:        3500000 kB
HugePages_Total:       0
";

    #[test]
    fn parses_known_fields_in_bytes() {
        let info = parse_meminfo(SAMPLE);
        assert_eq!(info.total, 16_000_000 * 1024);
        assert_eq!(info.free, 1_000_000 * 1024);
        assert_eq!(info.available, 8_000_000 * 1024);
        assert_eq!(info.buffers, 500_000 * 1024);
        assert_eq!(info.cached, 6_000_000 * 1024);
        assert_eq!(info.reclaimable, 300_000 * 1024);
        assert_eq!(info.shmem, 200_000 * 1024);
        assert_eq!(info.swap_total, 4_000_000 * 1024);
        assert_eq!(info.swap_free, 3_500_000 * 1024);
        assert_eq!(info.swap_cached, 100_000 * 1024);
        assert!(info.available_present);
    }

    #[test]
    fn used_is_total_minus_available() {
        let info = parse_meminfo(SAMPLE);
        assert_eq!(info.used(), (16_000_000 - 8_000_000) * 1024);
        assert_eq!(info.swap_used(), 500_000 * 1024);
    }

    #[test]
    fn estimates_available_on_old_kernels() {
        let text = "MemTotal: 1000 kB\nMemFree: 100 kB\nBuffers: 200 kB\nCached: 300 kB\n";
        let info = parse_meminfo(text);
        assert!(!info.available_present);
        assert_eq!(info.available, 600 * 1024);
        assert_eq!(info.used(), 400 * 1024);
    }

    #[test]
    fn tolerates_malformed_lines() {
        let text = "garbage\nMemTotal: abc kB\nMemFree:\n\nMemTotal: 2000 kB";
        let info = parse_meminfo(text);
        assert_eq!(info.total, 2000 * 1024);
        assert_eq!(info.free, 0);
    }

    #[test]
    fn percent_helpers_clamp_and_handle_zero() {
        let empty = MemorySnapshot::default();
        assert_eq!(empty.used_percent(), 0.0);
        assert_eq!(empty.swap_used_percent(), 0.0);
        assert!(!empty.has_swap());

        let snapshot = MemorySnapshot {
            total: 1000,
            used: 250,
            swap_total: 500,
            swap_used: 500,
            ..MemorySnapshot::default()
        };
        assert_eq!(snapshot.used_percent(), 25.0);
        assert_eq!(snapshot.swap_used_percent(), 100.0);
        assert!(snapshot.has_swap());
    }

    #[test]
    fn missing_meminfo_yields_none() {
        assert!(read_meminfo(Path::new("/definitely/not/here")).is_none());
    }

    #[test]
    fn collector_reads_the_running_system() {
        let mut system = System::new();
        let snapshot = MemoryCollector::default().sample(&mut system);
        assert!(
            snapshot.total > 0,
            "the running system must report physical RAM"
        );
        assert!(snapshot.used <= snapshot.total);
        assert!(snapshot.available <= snapshot.total);
    }
}
