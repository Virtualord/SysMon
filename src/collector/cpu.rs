//! CPU sampling.
//!
//! Utilization is always computed **over time**, never reported from the raw
//! cumulative counters. Two sources are combined:
//!
//! * `sysinfo` supplies the per-core and global utilization, the brand string, the
//!   frequencies and the core counts. It already diffs the counters of `/proc/stat`
//!   between two refreshes.
//! * [`CpuTimesTracker`] parses the aggregate `cpu` line of `/proc/stat` itself so
//!   that the user / system / idle / I/O wait breakdown is available. `sysinfo` no
//!   longer exposes those fields, and they are what a monitor needs to explain *why*
//!   the CPU is busy.
//!
//! Every counter in `/proc/stat` is monotonic since boot. All percentages produced
//! here are `delta(state) / delta(total) * 100` between two consecutive samples, so
//! the first sample after start-up reports zeros instead of a bogus spike.

use std::io;
use std::path::Path;

use sysinfo::System;

/// Path of the kernel CPU statistics file.
pub const PROC_STAT: &str = "/proc/stat";

/// Per logical core utilization.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct CpuCore {
    /// Percentage of the sampling interval the core was not idle (0.0 - 100.0).
    pub usage: f32,
    /// Current frequency in MHz, when the kernel exposes it.
    pub frequency_mhz: Option<u64>,
    /// Temperature in degrees Celsius, when a hwmon device exposes it.
    pub temperature: Option<f64>,
}

/// Aggregated `times` percentages across all logical cores.
///
/// The values do not necessarily add up to 100% because a core can be busy in more
/// than one state at a time (user + interrupt, for example).
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct CpuTimes {
    /// Time spent in user mode, including `nice`.
    pub user: f32,
    /// Time spent in kernel mode.
    pub system: f32,
    /// Time spent idle.
    pub idle: f32,
    /// Time spent waiting for block I/O.
    pub iowait: f32,
    /// Time spent servicing interrupts.
    pub interrupt: f32,
    /// Time stolen by the hypervisor.
    pub steal: f32,
    /// Whether at least two samples were available to compute the percentages.
    pub valid: bool,
}

impl CpuTimes {
    /// Busy time: everything that is neither idle nor I/O wait.
    pub fn busy(&self) -> f32 {
        if !self.valid {
            return 0.0;
        }
        (self.user + self.system + self.interrupt + self.steal).clamp(0.0, 100.0)
    }
}

/// A complete CPU sample.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct CpuSnapshot {
    /// Marketing name of the CPU, e.g. `AMD Ryzen 7 5800X 8-Core Processor`.
    pub brand: Option<String>,
    /// Number of physical cores, when it can be determined.
    pub physical_cores: Option<usize>,
    /// Number of logical cores (hardware threads).
    pub logical_cores: usize,
    /// Average frequency across all cores in MHz, when available.
    pub frequency_mhz: Option<u64>,
    /// Total utilization, 0.0 - 100.0.
    pub usage: f32,
    /// Per logical core utilization.
    pub cores: Vec<CpuCore>,
    /// Aggregated state percentages for the sampling interval.
    pub times: CpuTimes,
    /// Best-effort package/core temperature in degrees Celsius.
    pub temperature: Option<f64>,
}

impl CpuSnapshot {
    /// Percentage of the system-wide load that is not attributable to idle or I/O wait.
    ///
    /// Falls back to the total utilization when the `times` breakdown is unavailable
    /// (for example when `/proc` is not mounted).
    pub fn busy_percent(&self) -> f32 {
        if self.times.valid {
            self.times.busy()
        } else {
            self.usage
        }
    }
}

/// Raw cumulative jiffy counters of a `/proc/stat` CPU line.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct CpuJiffies {
    /// `user`
    pub user: u64,
    /// `nice`
    pub nice: u64,
    /// `system`
    pub system: u64,
    /// `idle`
    pub idle: u64,
    /// `iowait`
    pub iowait: u64,
    /// `irq`
    pub irq: u64,
    /// `softirq`
    pub softirq: u64,
    /// `steal`
    pub steal: u64,
}

impl CpuJiffies {
    /// Sum of all states, i.e. the total elapsed CPU time.
    pub fn total(&self) -> u64 {
        self.user
            .saturating_add(self.nice)
            .saturating_add(self.system)
            .saturating_add(self.idle)
            .saturating_add(self.iowait)
            .saturating_add(self.irq)
            .saturating_add(self.softirq)
            .saturating_add(self.steal)
    }

    /// Sum of the states that represent actual work.
    pub fn busy(&self) -> u64 {
        self.user
            .saturating_add(self.nice)
            .saturating_add(self.system)
            .saturating_add(self.irq)
            .saturating_add(self.softirq)
            .saturating_add(self.steal)
    }
}

/// Parses a `cpu` / `cpu0` / `cpu12` line of `/proc/stat`.
///
/// Returns `None` for lines that describe something else (`intr`, `ctxt`, ...) or
/// that are truncated. The kernel gained the `guest` fields after 2.6.24; they are
/// deliberately ignored because they are already accounted for in `user`.
pub fn parse_cpu_line(line: &str) -> Option<CpuJiffies> {
    let mut parts = line.split_whitespace();
    let label = parts.next()?;
    if label != "cpu"
        && !label
            .strip_prefix("cpu")
            .is_some_and(|rest| !rest.is_empty() && rest.chars().all(|c| c.is_ascii_digit()))
    {
        return None;
    }
    let values: Vec<u64> = parts
        .map(|p| p.parse::<u64>().ok())
        .collect::<Option<Vec<_>>>()?;
    if values.len() < 4 {
        return None;
    }
    let mut jiffies = CpuJiffies {
        user: values[0],
        nice: values[1],
        system: values[2],
        idle: values[3],
        ..CpuJiffies::default()
    };
    if let Some(v) = values.get(4) {
        jiffies.iowait = *v;
    }
    if let Some(v) = values.get(5) {
        jiffies.irq = *v;
    }
    if let Some(v) = values.get(6) {
        jiffies.softirq = *v;
    }
    if let Some(v) = values.get(7) {
        jiffies.steal = *v;
    }
    Some(jiffies)
}

/// Computes the state percentages between two `/proc/stat` samples.
#[derive(Debug, Clone, Copy, Default)]
pub struct CpuTimesTracker {
    previous: Option<CpuJiffies>,
}

impl CpuTimesTracker {
    /// Creates a tracker without a previous sample.
    pub const fn new() -> Self {
        Self { previous: None }
    }

    /// Feeds a new raw sample and returns the percentages since the previous one.
    ///
    /// The first call returns an invalid (all zero) [`CpuTimes`]; identical counters
    /// (no elapsed jiffies) do the same instead of dividing by zero.
    pub fn update(&mut self, current: CpuJiffies) -> CpuTimes {
        let Some(previous) = self.previous else {
            self.previous = Some(current);
            return CpuTimes::default();
        };
        self.previous = Some(current);

        let total = current.total().saturating_sub(previous.total());
        if total == 0 {
            return CpuTimes::default();
        }
        let delta = |now: u64, before: u64| {
            (now.saturating_sub(before) as f64 / total as f64 * 100.0).clamp(0.0, 100.0) as f32
        };
        CpuTimes {
            user: delta(current.user + current.nice, previous.user + previous.nice),
            system: delta(current.system, previous.system),
            idle: delta(current.idle, previous.idle),
            iowait: delta(current.iowait, previous.iowait),
            interrupt: delta(
                current.irq + current.softirq,
                previous.irq + previous.softirq,
            ),
            steal: delta(current.steal, previous.steal),
            valid: true,
        }
    }

    /// Parses `/proc/stat` and updates the tracker.
    ///
    /// Failures are not fatal: the previous sample is kept and an invalid
    /// [`CpuTimes`] is returned so the UI can fall back to the total utilization.
    pub fn update_from_file(&mut self, path: &Path) -> CpuTimes {
        match std::fs::read_to_string(path) {
            Ok(text) => {
                // Note the separator in `/proc/stat` is *two* spaces after the label
                // (`cpu  961474 80 ...`), so the aggregate line is matched on its label
                // rather than on a raw `cpu ` prefix.
                let aggregate = text
                    .lines()
                    .find(|line| line.split_whitespace().next() == Some("cpu"))
                    .and_then(parse_cpu_line);
                match aggregate {
                    Some(jiffies) => self.update(jiffies),
                    None => {
                        tracing::debug!(path = %path.display(), "no aggregate cpu line in proc stat");
                        CpuTimes::default()
                    }
                }
            }
            Err(err) => {
                tracing::debug!(path = %path.display(), %err, "cannot read proc stat");
                CpuTimes::default()
            }
        }
    }
}

/// Collects CPU statistics, combining `sysinfo` and `/proc/stat`.
pub struct CpuCollector {
    brand: Option<String>,
    physical_cores: Option<usize>,
    logical_cores: usize,
    times: CpuTimesTracker,
    proc_stat: std::path::PathBuf,
}

impl CpuCollector {
    /// Creates a collector and performs the priming refresh required by `sysinfo`.
    ///
    /// `sysinfo` needs two samples to compute deltas, therefore the very first call to
    /// [`CpuCollector::sample`] reports zeros instead of a bogus value.
    pub fn new(system: &mut System, proc_stat: impl Into<std::path::PathBuf>) -> Self {
        let proc_stat = proc_stat.into();
        system.refresh_cpu_usage();
        let logical_cores = system.cpus().len();
        let brand = system
            .cpus()
            .iter()
            .map(sysinfo::Cpu::brand)
            .find_map(|brand| {
                let trimmed = brand.trim();
                (!trimmed.is_empty()).then(|| trimmed.to_string())
            });
        let physical_cores = System::physical_core_count();
        let mut times = CpuTimesTracker::new();
        times.update_from_file(&proc_stat);
        Self {
            brand,
            physical_cores,
            logical_cores,
            times,
            proc_stat,
        }
    }

    /// Number of logical cores known to the collector.
    pub fn logical_cores(&self) -> usize {
        self.logical_cores
    }

    /// Refreshes the CPU counters and returns a snapshot for the sampling interval.
    pub fn sample(&mut self, system: &mut System) -> CpuSnapshot {
        system.refresh_cpu_usage();
        let times = self.times.update_from_file(&self.proc_stat);

        let mut frequency_total: u64 = 0;
        let mut frequency_count: u64 = 0;
        let mut cores = Vec::with_capacity(self.logical_cores);
        for cpu in system.cpus() {
            let frequency = cpu.frequency();
            if frequency > 0 {
                frequency_total += frequency;
                frequency_count += 1;
            }
            cores.push(CpuCore {
                usage: cpu.cpu_usage(),
                frequency_mhz: (frequency > 0).then_some(frequency),
                temperature: None,
            });
        }

        CpuSnapshot {
            brand: self.brand.clone(),
            physical_cores: self.physical_cores,
            logical_cores: self.logical_cores,
            frequency_mhz: (frequency_count > 0).then(|| frequency_total / frequency_count),
            usage: system.global_cpu_usage(),
            cores,
            times,
            temperature: None,
        }
    }
}

/// Returns the raw aggregate counters, exposed for tests and diagnostics.
pub fn read_aggregate(path: &Path) -> io::Result<CpuJiffies> {
    let text = std::fs::read_to_string(path)?;
    text.lines()
        .filter(|line| line.starts_with("cpu "))
        .find_map(parse_cpu_line)
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, "no aggregate cpu line"))
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE_A: &str = "cpu  100 20 50 900 30 5 5 0 0 0";
    const SAMPLE_B: &str = "cpu  200 20 100 1000 30 5 5 0 0 0";

    #[test]
    fn parses_aggregate_line() {
        let jiffies = parse_cpu_line(SAMPLE_A).expect("line should parse");
        assert_eq!(
            jiffies,
            CpuJiffies {
                user: 100,
                nice: 20,
                system: 50,
                idle: 900,
                iowait: 30,
                irq: 5,
                softirq: 5,
                steal: 0
            }
        );
        assert_eq!(jiffies.total(), 1110);
        assert_eq!(jiffies.busy(), 180);
    }

    #[test]
    fn parses_per_core_line() {
        let jiffies = parse_cpu_line("cpu3 1 2 3 4 5 6 7 8").expect("line should parse");
        assert_eq!(jiffies.user, 1);
        assert_eq!(jiffies.steal, 8);
    }

    #[test]
    fn reads_the_real_proc_stat() {
        // Guards against a parsing regression that would silently disable the whole
        // user/system/idle/iowait breakdown on a real system.
        let jiffies = read_aggregate(Path::new(PROC_STAT)).expect("/proc/stat must be readable");
        assert!(
            jiffies.total() > 0,
            "the running system must have accumulated CPU time"
        );
        assert!(jiffies.idle > 0);
    }

    #[test]
    fn tracker_reads_the_real_proc_stat() {
        let mut tracker = CpuTimesTracker::new();
        let first = tracker.update_from_file(Path::new(PROC_STAT));
        assert!(!first.valid, "the first sample has no predecessor");
        // A second call with unchanged counters must not divide by zero.
        let second = tracker.update_from_file(Path::new(PROC_STAT));
        assert!(second.user.is_finite());
    }

    #[test]
    fn handles_the_double_space_separator_of_proc_stat() {
        // `/proc/stat` separates the label from the first counter with two spaces.
        let text = "cpu  961474 80 177150 6807801 22789 47439 19969 0 0 0\ncpu0 480737 40 88575 3403900 11394 23719 9984 0 0 0\n";
        let dir = tempfile::tempdir().expect("temp dir");
        let path = dir.path().join("stat");
        std::fs::write(&path, text).expect("write");

        let mut tracker = CpuTimesTracker::new();
        tracker.update_from_file(&path);
        // Feed a second sample with different counters.
        let text2 = "cpu  1061474 80 187150 6807801 22789 47439 19969 0 0 0\n";
        std::fs::write(&path, text2).expect("write");
        let times = tracker.update_from_file(&path);
        assert!(
            times.valid,
            "the aggregate line must be found despite the padding"
        );
        assert!(times.user > 0.0);
    }

    #[test]
    fn rejects_other_lines() {
        assert!(parse_cpu_line("intr 12345 1 2 3").is_none());
        assert!(parse_cpu_line("cpuwat 1 2 3 4").is_none());
        assert!(parse_cpu_line("cpufreq 1 2 3 4").is_none());
        assert!(parse_cpu_line("cpu 1 2 3").is_none());
        assert!(parse_cpu_line("").is_none());
    }

    #[test]
    fn tolerates_missing_trailing_fields() {
        // Older kernels do not report iowait/irq/softirq/steal.
        let jiffies = parse_cpu_line("cpu 1 2 3 4").expect("line should parse");
        assert_eq!(jiffies.iowait, 0);
        assert_eq!(jiffies.steal, 0);
    }

    #[test]
    fn first_update_is_invalid() {
        let mut tracker = CpuTimesTracker::new();
        let times = tracker.update(parse_cpu_line(SAMPLE_A).unwrap());
        assert!(!times.valid);
        assert_eq!(times.busy(), 0.0);
    }

    #[test]
    fn computes_state_percentages_over_time() {
        let mut tracker = CpuTimesTracker::new();
        tracker.update(parse_cpu_line(SAMPLE_A).unwrap());
        let times = tracker.update(parse_cpu_line(SAMPLE_B).unwrap());
        assert!(times.valid);
        // Deltas: user+nice 100, system 50, idle 100, total 250.
        assert!((times.user - (100.0 / 250.0 * 100.0)).abs() < 0.01);
        assert!((times.system - (50.0 / 250.0 * 100.0)).abs() < 0.01);
        assert!((times.idle - (100.0 / 250.0 * 100.0)).abs() < 0.01);
        assert_eq!(times.iowait, 0.0);
        // Busy excludes idle: (100 + 50) / 250.
        assert!((times.busy() - (150.0 / 250.0 * 100.0)).abs() < 0.05);
    }

    #[test]
    fn identical_samples_do_not_divide_by_zero() {
        let mut tracker = CpuTimesTracker::new();
        let jiffies = parse_cpu_line(SAMPLE_A).unwrap();
        tracker.update(jiffies);
        let times = tracker.update(jiffies);
        assert!(!times.valid);
        assert!(times.user.is_finite());
    }

    #[test]
    fn busy_percent_falls_back_to_total_usage() {
        let snapshot = CpuSnapshot {
            usage: 42.0,
            ..CpuSnapshot::default()
        };
        assert_eq!(snapshot.busy_percent(), 42.0);
    }

    #[test]
    fn busy_percent_uses_breakdown_when_valid() {
        let snapshot = CpuSnapshot {
            usage: 100.0,
            times: CpuTimes {
                user: 10.0,
                system: 5.0,
                valid: true,
                ..CpuTimes::default()
            },
            ..CpuSnapshot::default()
        };
        assert_eq!(snapshot.busy_percent(), 15.0);
    }

    #[test]
    fn missing_proc_stat_does_not_panic() {
        let mut tracker = CpuTimesTracker::new();
        let times = tracker.update_from_file(Path::new("/definitely/not/here"));
        assert!(!times.valid);
    }
}
