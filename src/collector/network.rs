//! Network interface sampling.
//!
//! `/proc/net/dev` is the authoritative source for the byte counters because it is
//! cheap to parse, works without privileges and exposes the error/drop counters that
//! `sysinfo` does not. `sysinfo` is used on top of it for the MAC address and the
//! operational state of the interface.
//!
//! ## Throughput versus cumulative traffic
//!
//! The counters in `/proc/net/dev` are monotonic byte totals since the interface came
//! up. A rate is therefore *not* the counter itself but
//!
//! ```text
//! rate = (counter_now - counter_previous) / elapsed_seconds
//! ```
//!
//! where `elapsed` is the wall clock time between the two samples, not the number of
//! samples. When a counter moves backwards (32 bit wrap-around, interface reset or
//! counter truncation) the delta is discarded and the rate resets to zero instead of
//! producing a negative spike.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::Duration;

use sysinfo::{InterfaceOperationalState, Networks};

/// Path of the interface statistics file.
const PROC_NET_DEV: &str = "/proc/net/dev";

/// Interfaces that are noise on a typical Linux host and are skipped by default.
const NOISY_INTERFACES: &[&str] = &["lo"];

/// Raw counters of one interface, as read from `/proc/net/dev`.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct InterfaceCounters {
    /// Bytes received.
    pub received: u64,
    /// Packets received.
    pub packets_received: u64,
    /// Receive errors.
    pub errors_received: u64,
    /// Received packets dropped by the kernel.
    pub dropped_received: u64,
    /// Bytes transmitted.
    pub transmitted: u64,
    /// Packets transmitted.
    pub packets_transmitted: u64,
    /// Transmit errors.
    pub errors_transmitted: u64,
    /// Transmitted packets dropped by the kernel.
    pub dropped_transmitted: u64,
}

/// One row of the network table.
#[derive(Debug, Clone, PartialEq)]
pub struct InterfaceStats {
    /// Interface name, e.g. `wlp2s0`.
    pub name: String,
    /// Cumulative bytes received since the interface came up.
    pub total_received: u64,
    /// Cumulative bytes transmitted since the interface came up.
    pub total_transmitted: u64,
    /// Receive throughput in bytes per second.
    pub receive_rate: f64,
    /// Transmit throughput in bytes per second.
    pub transmit_rate: f64,
    /// Receive/transmit error counters.
    pub counters: InterfaceCounters,
    /// Hardware address, when the interface has one.
    pub mac_address: Option<String>,
    /// Whether the kernel reports the interface as up.
    pub is_up: bool,
    /// Whether the interface is the loopback device.
    pub is_loopback: bool,
}

impl InterfaceStats {
    /// Total traffic (received + transmitted) in bytes.
    pub fn total_traffic(&self) -> u64 {
        self.total_received.saturating_add(self.total_transmitted)
    }
}

/// A complete network sample.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct NetworkSnapshot {
    /// One entry per monitored interface.
    pub interfaces: Vec<InterfaceStats>,
    /// Sum of the receive throughput of all interfaces, bytes per second.
    pub receive_rate: f64,
    /// Sum of the transmit throughput of all interfaces, bytes per second.
    pub transmit_rate: f64,
    /// Sum of the cumulative counters of all interfaces.
    pub total_received: u64,
    /// Sum of the cumulative counters of all interfaces.
    pub total_transmitted: u64,
}

impl NetworkSnapshot {
    /// Looks an interface up by name.
    pub fn find(&self, name: &str) -> Option<&InterfaceStats> {
        self.interfaces
            .iter()
            .find(|interface| interface.name == name)
    }
}

/// Parses `/proc/net/dev`.
///
/// The format is a two line header followed by one `name: rx ... tx ...` line per
/// interface. Fields are decimal numbers, some kernels pad them with a `+` when the
/// value wrapped around, which is treated as a parse failure for that interface only.
pub fn parse_net_dev(text: &str) -> HashMap<String, InterfaceCounters> {
    let mut interfaces = HashMap::new();
    for line in text.lines() {
        let Some((name, rest)) = line.split_once(':') else {
            continue;
        };
        let name = name.trim();
        if name.is_empty() {
            continue;
        }
        let values: Vec<u64> = match rest
            .split_whitespace()
            .map(|value| value.parse::<u64>())
            .collect::<Result<Vec<_>, _>>()
        {
            Ok(values) if values.len() >= 16 => values,
            _ => continue,
        };
        let counters = InterfaceCounters {
            received: values[0],
            packets_received: values[1],
            errors_received: values[2],
            dropped_received: values[3],
            transmitted: values[8],
            packets_transmitted: values[9],
            errors_transmitted: values[10],
            dropped_transmitted: values[11],
        };
        interfaces.insert(name.to_string(), counters);
    }
    interfaces
}

/// Per interface state kept between two samples to compute rates.
#[derive(Debug, Clone, Copy, Default)]
struct PreviousCounters {
    received: u64,
    transmitted: u64,
}

/// Collects network statistics.
pub struct NetworkCollector {
    proc_net_dev: PathBuf,
    previous: HashMap<String, PreviousCounters>,
    include_loopback: bool,
    interface_filter: Vec<String>,
}

impl NetworkCollector {
    /// Creates a collector.
    ///
    /// `interface_filter` restricts monitoring to the named interfaces; an empty list
    /// monitors everything except the loopback device.
    pub fn new(interface_filter: Vec<String>) -> Self {
        Self {
            proc_net_dev: PathBuf::from(PROC_NET_DEV),
            previous: HashMap::new(),
            include_loopback: false,
            interface_filter,
        }
    }

    /// Creates a collector with an explicit path, which keeps the tests hermetic.
    pub fn with_path(path: impl Into<PathBuf>, interface_filter: Vec<String>) -> Self {
        Self {
            proc_net_dev: path.into(),
            ..Self::new(interface_filter)
        }
    }

    /// Whether loopback traffic is included.
    pub fn include_loopback(&mut self, include: bool) {
        self.include_loopback = include;
    }

    /// Reads a fresh network sample.
    pub fn sample(&mut self) -> NetworkSnapshot {
        self.sample_with_elapsed(Networks::new_with_refreshed_list(), Duration::from_secs(1))
    }

    /// Reads a sample, computing rates from the counters of the previous call.
    pub fn sample_with_elapsed(
        &mut self,
        networks: Networks,
        elapsed: Duration,
    ) -> NetworkSnapshot {
        let seconds = if elapsed.as_secs_f64() > 0.0 {
            elapsed.as_secs_f64()
        } else {
            f64::NAN
        };
        let current = read_counters(&self.proc_net_dev);
        let mut interfaces = Vec::with_capacity(current.len());
        let mut snapshot = NetworkSnapshot::default();
        let mut previous = HashMap::new();

        for (name, counters) in current {
            if !self.wanted(&name) {
                continue;
            }
            let (receive_rate, transmit_rate) = match self.previous.get(&name) {
                // A counter that went backwards means the interface was reset; report
                // no traffic for this interval instead of a bogus negative rate.
                Some(before)
                    if seconds.is_finite()
                        && counters.received >= before.received
                        && counters.transmitted >= before.transmitted =>
                {
                    (
                        (counters.received - before.received) as f64 / seconds,
                        (counters.transmitted - before.transmitted) as f64 / seconds,
                    )
                }
                _ => (0.0, 0.0),
            };
            let rates_are_valid = receive_rate.is_finite() && receive_rate >= 0.0;
            let data = networks.list().get(&name);
            let interface = InterfaceStats {
                is_loopback: name == "lo",
                mac_address: data.map(|data| data.mac_address().to_string()),
                is_up: data.is_some_and(|data| {
                    matches!(data.operational_state(), InterfaceOperationalState::Up)
                }),
                receive_rate: if rates_are_valid { receive_rate } else { 0.0 },
                transmit_rate: if transmit_rate.is_finite() && transmit_rate >= 0.0 {
                    transmit_rate
                } else {
                    0.0
                },
                name: name.clone(),
                total_received: counters.received,
                total_transmitted: counters.transmitted,
                counters,
            };
            snapshot.receive_rate += interface.receive_rate;
            snapshot.transmit_rate += interface.transmit_rate;
            snapshot.total_received = snapshot.total_received.saturating_add(counters.received);
            snapshot.total_transmitted = snapshot
                .total_transmitted
                .saturating_add(counters.transmitted);
            previous.insert(
                name,
                PreviousCounters {
                    received: counters.received,
                    transmitted: counters.transmitted,
                },
            );
            interfaces.push(interface);
        }

        // Drop counters of interfaces that disappeared so a hotplug event does not
        // resurrect a stale rate later on.
        self.previous = previous;
        snapshot.interfaces = interfaces;
        snapshot
    }

    /// Whether an interface should be reported.
    ///
    /// An explicitly configured interface always wins, which is how a user can ask
    /// for loopback traffic even though it is hidden by default.
    fn wanted(&self, name: &str) -> bool {
        if self.interface_filter.iter().any(|filter| filter == name) {
            return true;
        }
        if !self.interface_filter.is_empty() {
            return false;
        }
        self.include_loopback || !NOISY_INTERFACES.contains(&name)
    }
}

/// Reads and parses `/proc/net/dev`, returning an empty map when unavailable.
pub fn read_counters(path: &Path) -> HashMap<String, InterfaceCounters> {
    match std::fs::read_to_string(path) {
        Ok(text) => parse_net_dev(&text),
        Err(err) => {
            tracing::debug!(path = %path.display(), %err, "cannot read net dev");
            HashMap::new()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = "\
Inter-|   Receive                                                |  Transmit
 face |bytes    packets errs drop fifo frame compressed multicast|bytes    packets errs drop fifo colls carrier compressed
    lo: 1000      10    0    0    0     0          0         0     1000      10    0    0    0     0       0          0
  eth0: 500000    400    1    2    0     0          0         0    250000    200    3    4    0     0       0          0
";

    #[test]
    fn parses_interfaces_and_counters() {
        let parsed = parse_net_dev(SAMPLE);
        assert_eq!(parsed.len(), 2);
        let eth0 = parsed.get("eth0").expect("eth0 present");
        assert_eq!(eth0.received, 500_000);
        assert_eq!(eth0.transmitted, 250_000);
        assert_eq!(eth0.errors_received, 1);
        assert_eq!(eth0.dropped_transmitted, 4);
    }

    #[test]
    fn ignores_headers_and_garbage() {
        let parsed = parse_net_dev("Inter-|   Receive\n face |bytes\nbroken line without colon\n");
        assert!(parsed.is_empty());
    }

    #[test]
    fn rejects_truncated_interface_lines() {
        let parsed = parse_net_dev("  eth0: 100 1 2\n");
        assert!(parsed.is_empty());
    }

    #[test]
    fn collector_computes_rates_from_deltas() {
        let dir = tempfile::tempdir().expect("temp dir");
        let path = dir.path().join("net_dev");

        std::fs::write(&path, SAMPLE).expect("write");
        let mut collector = NetworkCollector::with_path(&path, Vec::new());
        let first = collector
            .sample_with_elapsed(Networks::new_with_refreshed_list(), Duration::from_secs(1));
        // First sample has no previous counters, so rates start at zero.
        assert_eq!(first.receive_rate, 0.0);
        assert_eq!(first.interfaces.len(), 1);
        assert_eq!(first.interfaces[0].name, "eth0");
        assert!(first.total_received > 0);

        std::fs::write(
            &path,
            SAMPLE
                .replace("500000", "600000")
                .replace("250000", "260000"),
        )
        .expect("write");
        let second = collector
            .sample_with_elapsed(Networks::new_with_refreshed_list(), Duration::from_secs(2));
        let eth0 = second.find("eth0").expect("eth0 present");
        assert!((eth0.receive_rate - 50_000.0).abs() < f64::EPSILON);
        assert!((eth0.transmit_rate - 5_000.0).abs() < f64::EPSILON);
        assert_eq!(second.receive_rate, 50_000.0);
    }

    #[test]
    fn counter_reset_yields_zero_rate() {
        let dir = tempfile::tempdir().expect("temp dir");
        let path = dir.path().join("net_dev");
        std::fs::write(&path, SAMPLE).expect("write");
        let mut collector = NetworkCollector::with_path(&path, Vec::new());
        collector.sample_with_elapsed(Networks::new_with_refreshed_list(), Duration::from_secs(1));

        // Simulate an interface reset: counters go backwards.
        std::fs::write(
            &path,
            SAMPLE.replace("500000", "10").replace("250000", "10"),
        )
        .expect("write");
        let after = collector
            .sample_with_elapsed(Networks::new_with_refreshed_list(), Duration::from_secs(1));
        assert_eq!(after.receive_rate, 0.0);
        assert_eq!(after.transmit_rate, 0.0);
        assert!(after.interfaces[0].receive_rate.is_finite());
    }

    #[test]
    fn disappearing_interfaces_are_forgotten() {
        let dir = tempfile::tempdir().expect("temp dir");
        let path = dir.path().join("net_dev");
        std::fs::write(&path, SAMPLE).expect("write");
        let mut collector = NetworkCollector::with_path(&path, Vec::new());
        collector.sample_with_elapsed(Networks::new_with_refreshed_list(), Duration::from_secs(1));

        std::fs::write(&path, "  wlan0: 1 1 0 0 0 0 0 0 1 1 0 0 0 0 0 0\n").expect("write");
        let after = collector
            .sample_with_elapsed(Networks::new_with_refreshed_list(), Duration::from_secs(1));
        assert_eq!(after.interfaces.len(), 1);
        assert_eq!(after.interfaces[0].name, "wlan0");
        assert_eq!(after.receive_rate, 0.0);
    }

    #[test]
    fn interface_filter_is_honoured() {
        let dir = tempfile::tempdir().expect("temp dir");
        let path = dir.path().join("net_dev");
        std::fs::write(&path, SAMPLE).expect("write");
        let mut collector = NetworkCollector::with_path(&path, vec!["lo".to_string()]);
        let snapshot = collector
            .sample_with_elapsed(Networks::new_with_refreshed_list(), Duration::from_secs(1));
        assert_eq!(snapshot.interfaces.len(), 1);
        assert_eq!(snapshot.interfaces[0].name, "lo");
    }

    #[test]
    fn missing_proc_net_dev_yields_empty_snapshot() {
        let mut collector = NetworkCollector::with_path("/definitely/not/here", Vec::new());
        let snapshot = collector.sample();
        assert!(snapshot.interfaces.is_empty());
        assert_eq!(snapshot.receive_rate, 0.0);
    }

    #[test]
    fn total_traffic_sums_both_directions() {
        let interface = InterfaceStats {
            name: "eth0".to_string(),
            total_received: 100,
            total_transmitted: 50,
            receive_rate: 0.0,
            transmit_rate: 0.0,
            counters: InterfaceCounters::default(),
            mac_address: None,
            is_up: true,
            is_loopback: false,
        };
        assert_eq!(interface.total_traffic(), 150);
    }
}
