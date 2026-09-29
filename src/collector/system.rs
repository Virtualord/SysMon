//! Static and slowly changing system information.
//!
//! Everything in here is either constant for the lifetime of the process (hostname,
//! distribution, kernel) or changes slowly (uptime, load average, temperatures) and
//! is therefore refreshed on a slower cadence than the CPU and memory metrics.

use std::path::{Path, PathBuf};

use sysinfo::{Component, Components, System};

/// Path of the distribution description file.
const ETC_OS_RELEASE: &str = "/etc/os-release";
/// Fallback used by some distributions (notably Debian derivatives).
const ETC_LSBN_RELEASE: &str = "/etc/lsb-release";
/// Path of the load average file.
const PROC_LOADAVG: &str = "/proc/loadavg";
/// Path of the process statistics file.
const PROC_STAT: &str = "/proc/stat";

/// Distribution metadata as published by `/etc/os-release`.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct OsRelease {
    /// `NAME`
    pub name: Option<String>,
    /// `VERSION`, the human readable release string.
    pub version: Option<String>,
    /// `VERSION_ID`, the machine readable release identifier.
    pub version_id: Option<String>,
    /// `ID`
    pub id: Option<String>,
    /// `PRETTY_NAME`
    pub pretty_name: Option<String>,
}

impl OsRelease {
    /// The most human friendly description available.
    ///
    /// The order is `PRETTY_NAME`, then `NAME` with `VERSION`, then whichever of the
    /// two is present. Falling all the way back to `None` is intentional: `ID=void` on
    /// its own describes nothing a user would recognise.
    pub fn describe(&self) -> Option<String> {
        if let Some(pretty) = self.pretty_name.as_ref().filter(|value| !value.is_empty()) {
            return Some(pretty.clone());
        }
        match (&self.name, &self.version) {
            (Some(name), Some(version)) => Some(format!("{name} {version}")),
            (Some(name), None) => Some(name.clone()),
            (None, Some(version)) => Some(version.clone()),
            (None, None) => None,
        }
    }
}

/// Parses a shell-like `KEY=value` document such as `/etc/os-release`.
///
/// Values may be quoted; single quotes are taken literally and double quotes may use
/// the usual backslash escapes.
pub fn parse_os_release(text: &str) -> OsRelease {
    let mut info = OsRelease::default();
    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let Some((key, raw_value)) = line.split_once('=') else {
            continue;
        };
        let value = unquote(raw_value.trim());
        if value.is_empty() {
            continue;
        }
        match key.trim() {
            "NAME" => info.name = Some(value),
            "VERSION" => info.version = Some(value),
            "VERSION_ID" => info.version_id = Some(value),
            "ID" => info.id = Some(value),
            "PRETTY_NAME" => info.pretty_name = Some(value),
            _ => {}
        }
    }
    info
}

/// Strips surrounding quotes and resolves backslash escapes inside double quotes.
fn unquote(value: &str) -> String {
    let bytes: Vec<char> = value.chars().collect();
    if bytes.len() >= 2 {
        let first = bytes[0];
        let last = bytes[bytes.len() - 1];
        if (first == '"' || first == '\'') && first == last {
            let inner: String = bytes[1..bytes.len() - 1].iter().collect();
            return if first == '"' {
                unescape(&inner)
            } else {
                inner
            };
        }
    }
    value.to_string()
}

/// Resolves the backslash escapes permitted inside double quotes by the format.
fn unescape(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    let mut chars = value.chars();
    while let Some(ch) = chars.next() {
        if ch != '\\' {
            out.push(ch);
            continue;
        }
        match chars.next() {
            Some('n') => out.push('\n'),
            Some('t') => out.push('\t'),
            Some('r') => out.push('\r'),
            Some('\\') => out.push('\\'),
            Some('"') => out.push('"'),
            Some(other) => {
                out.push('\\');
                out.push(other);
            }
            None => out.push('\\'),
        }
    }
    out
}

/// One temperature reading exposed by `/sys/class/hwmon` or `lm-sensors`.
#[derive(Debug, Clone, PartialEq)]
pub struct Temperature {
    /// Sensor name, for example `coretemp` or `Package id 0`.
    pub label: String,
    /// Current temperature in degrees Celsius.
    pub celsius: f32,
    /// Upper limit reported by the driver, when available.
    pub max_celsius: Option<f32>,
    /// Critical threshold reported by the driver, when available.
    pub critical_celsius: Option<f32>,
}

/// Information that does not change while syswatch runs.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct SystemInfo {
    /// `gethostname()` result, or the host part of the machine id as a fallback.
    pub hostname: String,
    /// Distribution description, e.g. `Void Linux 20250202`.
    pub distribution: Option<String>,
    /// Distribution identifier, e.g. `void`, `arch`, `fedora`.
    pub distribution_id: Option<String>,
    /// Full kernel version string.
    pub kernel: String,
    /// Machine architecture, e.g. `x86_64`.
    pub architecture: String,
    /// CPU brand string.
    pub cpu_brand: Option<String>,
    /// Number of physical cores.
    pub physical_cores: Option<usize>,
    /// Number of logical cores.
    pub logical_cores: usize,
    /// Physical RAM in bytes.
    pub total_memory: u64,
    /// UNIX timestamp of the boot.
    pub boot_time: u64,
}

impl SystemInfo {
    /// Seconds elapsed since the system booted.
    pub fn uptime(&self) -> u64 {
        System::uptime()
    }
}

/// Slowly changing metrics.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct DynamicInfo {
    /// Seconds since boot.
    pub uptime: u64,
    /// One minute load average.
    pub load_one: f64,
    /// Five minute load average.
    pub load_five: f64,
    /// Fifteen minute load average.
    pub load_fifteen: f64,
    /// Tasks currently runnable, from `/proc/loadavg`.
    pub running_tasks: Option<u32>,
    /// Total number of tasks, from `/proc/loadavg`.
    pub total_tasks: Option<u32>,
    /// Temperatures exposed by the kernel.
    pub temperatures: Vec<Temperature>,
}

impl DynamicInfo {
    /// Number of load samples that are strictly positive.
    pub fn load_available(&self) -> usize {
        [self.load_one, self.load_five, self.load_fifteen]
            .iter()
            .filter(|value| value.is_finite() && **value > 0.0)
            .count()
    }
}

/// Collects the static and slowly changing system information.
pub struct SystemCollector {
    os_release: PathBuf,
    lsb_release: PathBuf,
    loadavg: PathBuf,
    proc_stat: PathBuf,
}

impl Default for SystemCollector {
    fn default() -> Self {
        Self::new(ETC_OS_RELEASE, ETC_LSBN_RELEASE, PROC_LOADAVG, PROC_STAT)
    }
}

impl SystemCollector {
    /// Creates a collector with explicit paths, which keeps the parsers testable.
    pub fn new(
        os_release: impl Into<PathBuf>,
        lsb_release: impl Into<PathBuf>,
        loadavg: impl Into<PathBuf>,
        proc_stat: impl Into<PathBuf>,
    ) -> Self {
        Self {
            os_release: os_release.into(),
            lsb_release: lsb_release.into(),
            loadavg: loadavg.into(),
            proc_stat: proc_stat.into(),
        }
    }

    /// Reads the information that does not change while the program runs.
    pub fn static_info(&self, system: &System) -> SystemInfo {
        let os = read_os_release(&self.os_release, &self.lsb_release);
        let logical_cores = system.cpus().len().max(1);
        let cpu_brand = system
            .cpus()
            .iter()
            .map(sysinfo::Cpu::brand)
            .find_map(|brand| {
                let trimmed = brand.trim();
                (!trimmed.is_empty()).then(|| trimmed.to_string())
            });
        SystemInfo {
            hostname: hostname(),
            distribution: os.as_ref().and_then(OsRelease::describe),
            distribution_id: os.as_ref().and_then(|os| os.id.clone()),
            kernel: System::kernel_version().unwrap_or_else(|| "unknown".to_string()),
            architecture: System::cpu_arch(),
            cpu_brand,
            physical_cores: System::physical_core_count(),
            logical_cores,
            total_memory: system.total_memory(),
            boot_time: System::boot_time(),
        }
    }

    /// Reads uptime, load averages and sensor temperatures.
    pub fn dynamic_info(&self, components: &mut Components) -> DynamicInfo {
        let load = System::load_average();
        let (running_tasks, total_tasks) = read_loadavg_tasks(&self.loadavg);
        components.refresh(true);
        let temperatures = read_temperatures(components);
        DynamicInfo {
            uptime: System::uptime(),
            load_one: load.one,
            load_five: load.five,
            load_fifteen: load.fifteen,
            running_tasks,
            total_tasks,
            temperatures,
        }
    }

    /// Counts the runnable/total tasks from `/proc/stat` as a fallback source.
    pub fn stat_tasks(&self) -> (Option<u32>, Option<u32>) {
        let Ok(text) = std::fs::read_to_string(&self.proc_stat) else {
            return (None, None);
        };
        let mut procs_running: Option<u32> = None;
        let mut procs_blocked: Option<u32> = None;
        for line in text.lines() {
            if let Some(rest) = line.strip_prefix("procs_running ") {
                procs_running = rest.trim().parse().ok();
            } else if let Some(rest) = line.strip_prefix("procs_blocked ") {
                procs_blocked = rest.trim().parse().ok();
            }
        }
        match (procs_running, procs_blocked) {
            (Some(running), Some(blocked)) => (Some(running), Some(running + blocked)),
            (running, _) => (running, None),
        }
    }
}

/// Reads the hostname, falling back to `/etc/hostname` and then to `localhost`.
pub fn hostname() -> String {
    System::host_name()
        .filter(|name| !name.trim().is_empty())
        .or_else(|| {
            std::fs::read_to_string("/etc/hostname")
                .ok()
                .map(|name| name.trim().to_string())
        })
        .filter(|name| !name.is_empty())
        .unwrap_or_else(|| "localhost".to_string())
}

/// Reads and parses the distribution description file.
pub fn read_os_release(path: &Path, lsb_release: &Path) -> Option<OsRelease> {
    if let Ok(text) = std::fs::read_to_string(path) {
        let info = parse_os_release(&text);
        if info.describe().is_some() {
            return Some(info);
        }
    }
    // Debian/Ubuntu family keeps a minimal /etc/os-release but a richer lsb-release.
    let text = std::fs::read_to_string(lsb_release).ok()?;
    let info = parse_os_release(&text);
    info.describe().map(|_| info)
}

/// Parses the first three fields of `/proc/loadavg`.
pub fn read_loadavg_tasks(path: &Path) -> (Option<u32>, Option<u32>) {
    let Ok(text) = std::fs::read_to_string(path) else {
        return (None, None);
    };
    // `0.00 0.00 0.00 2/345 6789`: three load averages, then runnable/total tasks.
    let mut fields = text.split_whitespace();
    for _ in 0..3 {
        if fields.next().is_none() {
            return (None, None);
        }
    }
    // The fourth field is `runnable/total`, not two separate fields.
    let (running, total) = match fields.next() {
        Some(field) => match field.split_once('/') {
            Some((running, total)) => (running.parse().ok(), total.parse().ok()),
            None => (field.parse().ok(), None),
        },
        None => (None, None),
    };
    (running, total)
}

/// Extracts every temperature currently exposed by the kernel.
pub fn read_temperatures(components: &Components) -> Vec<Temperature> {
    components
        .iter()
        .filter_map(|component: &Component| {
            component.temperature().map(|celsius| Temperature {
                label: component.label().to_string(),
                celsius,
                max_celsius: component.max(),
                critical_celsius: component.critical(),
            })
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    const OS_RELEASE: &str = r#"
PRETTY_NAME="Void Linux 20250202 (rolling)"
NAME="Void Linux"
ID=void
VERSION_ID=20250202
HOME_URL="https://voidlinux.org/"
VARIANT_ID=glibc
"#;

    #[test]
    fn parses_os_release() {
        let info = parse_os_release(OS_RELEASE);
        assert_eq!(info.id.as_deref(), Some("void"));
        assert_eq!(info.name.as_deref(), Some("Void Linux"));
        assert_eq!(info.version, None);
        assert_eq!(info.version_id.as_deref(), Some("20250202"));
        assert_eq!(
            info.describe().as_deref(),
            Some("Void Linux 20250202 (rolling)")
        );
    }

    #[test]
    fn parses_version_and_version_id() {
        // `VERSION` is a human string and `VERSION_ID` a machine one; both are useful
        // and several distributions only set one of them.
        let info =
            parse_os_release("ID=fedora\nVERSION=\"41 (Workstation Edition)\"\nVERSION_ID=41\n");
        assert_eq!(info.version.as_deref(), Some("41 (Workstation Edition)"));
        assert_eq!(info.version_id.as_deref(), Some("41"));
        assert_eq!(info.describe().as_deref(), Some("41 (Workstation Edition)"));
    }

    #[test]
    fn describe_prefers_pretty_name_then_version_then_name() {
        assert_eq!(
            parse_os_release("PRETTY_NAME=\"Nice Name\"\nNAME=\"Plain\"\nVERSION=\"1.0\"\n")
                .describe(),
            Some("Nice Name".to_string())
        );
        assert_eq!(
            parse_os_release("NAME=\"Plain\"\nVERSION=\"1.0\"\n").describe(),
            Some("Plain 1.0".to_string())
        );
        assert_eq!(
            parse_os_release("NAME=\"Plain\"\n").describe(),
            Some("Plain".to_string())
        );
        // A version on its own is still better than showing N/A.
        assert_eq!(
            parse_os_release("VERSION=\"1.0\"\n").describe(),
            Some("1.0".to_string())
        );
        assert_eq!(parse_os_release("ID=void\n").describe(), None);
    }

    #[test]
    fn parses_single_quotes_and_comments() {
        let info = parse_os_release("# comment\n\nNAME='My Distro'\nID=\"my_distro\"\nBOGUS\n");
        assert_eq!(info.name.as_deref(), Some("My Distro"));
        assert_eq!(info.id.as_deref(), Some("my_distro"));
    }

    #[test]
    fn describes_from_name_and_version_when_pretty_is_absent() {
        let info = parse_os_release("NAME=Fedora Linux\nVERSION=41\n");
        assert_eq!(info.describe().as_deref(), Some("Fedora Linux 41"));
    }

    #[test]
    fn describe_returns_none_for_empty_document() {
        assert_eq!(parse_os_release("").describe(), None);
        assert_eq!(parse_os_release("ID=\n").describe(), None);
        assert_eq!(
            parse_os_release("ID=void\nBUILD_ID=rolling\n").describe(),
            None
        );
    }

    #[test]
    fn unescapes_double_quoted_values() {
        let info = parse_os_release(r#"PRETTY_NAME="Debian \"bookworm\" edition""#);
        assert_eq!(
            info.pretty_name.as_deref(),
            Some("Debian \"bookworm\" edition")
        );
    }

    #[test]
    fn loadavg_task_counts_are_parsed() {
        let dir = tempfile::tempdir().expect("temp dir");
        let path = dir.path().join("loadavg");
        std::fs::write(&path, "0.42 0.31 0.20 1/234 5678\n").expect("write");
        assert_eq!(read_loadavg_tasks(&path), (Some(1), Some(234)));
    }

    #[test]
    fn missing_loadavg_is_tolerated() {
        assert_eq!(
            read_loadavg_tasks(Path::new("/definitely/not/here")),
            (None, None)
        );
    }

    #[test]
    fn static_info_reports_a_hostname() {
        let mut system = System::new();
        system.refresh_cpu_usage();
        system.refresh_memory();
        let info = SystemCollector::default().static_info(&system);
        assert!(!info.hostname.is_empty());
        assert!(!info.kernel.is_empty());
        assert!(!info.architecture.is_empty());
        assert_eq!(info.logical_cores, system.cpus().len().max(1));
    }

    #[test]
    fn dynamic_info_reports_load_and_uptime() {
        let mut components = Components::new_with_refreshed_list();
        let info = SystemCollector::default().dynamic_info(&mut components);
        assert!(info.load_one.is_finite());
        assert!(info.uptime > 0);
    }
}
