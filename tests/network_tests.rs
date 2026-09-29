//! Integration tests for the network collector.
//!
//! The important properties are that a rate is a *difference* between two samples
//! divided by the elapsed time, that a counter reset does not produce a negative or
//! absurd rate, and that interfaces appearing and disappearing is not an error.

use std::time::Duration;

use sysinfo::Networks;
use syswatch::collector::network::{
    self, InterfaceCounters, InterfaceStats, NetworkCollector, NetworkSnapshot,
};

const SAMPLE_A: &str = "\
Inter-|   Receive                                                |  Transmit
 face |bytes    packets errs drop fifo frame compressed multicast|bytes    packets errs drop fifo colls carrier compressed
    lo:  1000      10    0    0    0     0          0         0     1000      10    0    0    0     0       0          0
  eth0: 500000    400    1    2    0     0          0         0   250000    200    3    4    0     0       0          0
 wlan0:  20000     20    0    0    0     0          0         0    10000     10    0    0    0     0       0          0
";

/// Writes `text` to a temporary `/proc/net/dev` stand-in and returns a collector for it.
fn collector_for(text: &str, dir: &tempfile::TempDir) -> (std::path::PathBuf, NetworkCollector) {
    let path = dir.path().join("net_dev");
    std::fs::write(&path, text).expect("write net_dev");
    let collector = NetworkCollector::with_path(&path, Vec::new());
    (path, collector)
}

fn sample(collector: &mut NetworkCollector, seconds: u64) -> NetworkSnapshot {
    collector.sample_with_elapsed(
        Networks::new_with_refreshed_list(),
        Duration::from_secs(seconds),
    )
}

#[test]
fn interface_counters_parse() {
    let parsed = network::parse_net_dev(SAMPLE_A);
    assert_eq!(
        parsed.len(),
        3,
        "loopback must be parsed even though it is filtered later"
    );

    let eth0 = parsed.get("eth0").expect("eth0");
    assert_eq!(eth0.received, 500_000);
    assert_eq!(eth0.transmitted, 250_000);
    assert_eq!(eth0.packets_received, 400);
    assert_eq!(eth0.packets_transmitted, 200);
    assert_eq!(eth0.errors_received, 1);
    assert_eq!(eth0.dropped_transmitted, 4);
}

#[test]
fn headers_and_junk_are_ignored() {
    for document in [
        "",
        "Inter-|   Receive\n face |bytes\n",
        "broken line\n",
        "  eth0: 1 2 3\n",
        "  eth0: 1 2 x 4 5 6 7 8 9 10 11 12 13 14 15 16\n",
    ] {
        assert!(
            network::parse_net_dev(document).is_empty(),
            "{document:?} must parse to nothing"
        );
    }
}

#[test]
fn the_first_sample_reports_no_throughput() {
    // There is nothing to difference against yet, so claiming traffic would be a lie.
    let dir = tempfile::tempdir().expect("temp dir");
    let (_path, mut collector) = collector_for(SAMPLE_A, &dir);
    let snapshot = sample(&mut collector, 1);

    assert_eq!(snapshot.receive_rate, 0.0);
    assert_eq!(snapshot.transmit_rate, 0.0);
    assert!(
        snapshot.total_received > 0,
        "cumulative totals are known immediately"
    );
}

#[test]
fn throughput_is_the_delta_divided_by_the_elapsed_time() {
    let dir = tempfile::tempdir().expect("temp dir");
    let (path, mut collector) = collector_for(SAMPLE_A, &dir);
    sample(&mut collector, 1);

    // eth0 gains 100 000 receive bytes and 10 000 transmit bytes.
    let next = SAMPLE_A
        .replace("eth0: 500000", "eth0: 600000")
        .replace("250000", "260000");
    std::fs::write(&path, next).expect("write");

    let snapshot = sample(&mut collector, 2);
    let eth0 = snapshot.find("eth0").expect("eth0");
    assert!(
        (eth0.receive_rate - 50_000.0).abs() < 0.001,
        "got {}",
        eth0.receive_rate
    );
    assert!(
        (eth0.transmit_rate - 5_000.0).abs() < 0.001,
        "got {}",
        eth0.transmit_rate
    );
}

#[test]
fn the_elapsed_time_is_used_rather_than_the_sample_count() {
    // One second and one hour of the same delta must produce very different rates.
    let dir = tempfile::tempdir().expect("temp dir");
    let (path, mut collector) = collector_for(SAMPLE_A, &dir);
    sample(&mut collector, 1);
    std::fs::write(&path, SAMPLE_A.replace("500000", "600000")).expect("write");
    let one_second = sample(&mut collector, 1)
        .find("eth0")
        .expect("eth0")
        .receive_rate;

    let dir = tempfile::tempdir().expect("temp dir");
    let (path, mut collector) = collector_for(SAMPLE_A, &dir);
    sample(&mut collector, 1);
    std::fs::write(&path, SAMPLE_A.replace("500000", "600000")).expect("write");
    let one_hour = sample(&mut collector, 3600)
        .find("eth0")
        .expect("eth0")
        .receive_rate;

    assert!(
        (one_second / one_hour - 3600.0).abs() < 1.0,
        "rates do not scale with elapsed time"
    );
}

#[test]
fn totals_are_cumulative_and_never_reported_as_rates() {
    let dir = tempfile::tempdir().expect("temp dir");
    let (path, mut collector) = collector_for(SAMPLE_A, &dir);
    sample(&mut collector, 1);
    std::fs::write(&path, SAMPLE_A.replace("500000", "1500000")).expect("write");
    let snapshot = sample(&mut collector, 1);
    let eth0 = snapshot.find("eth0").expect("eth0");

    assert_eq!(
        eth0.total_received, 1_500_000,
        "the total keeps counting from boot"
    );
    assert!(
        (eth0.receive_rate - 1_000_000.0).abs() < 0.001,
        "the rate is per second"
    );
    assert_eq!(
        eth0.total_traffic(),
        eth0.total_received + eth0.total_transmitted
    );
}

#[test]
fn a_counter_reset_reports_zero_rather_than_a_negative_rate() {
    let dir = tempfile::tempdir().expect("temp dir");
    let (path, mut collector) = collector_for(SAMPLE_A, &dir);
    sample(&mut collector, 1);

    // A 32-bit wrap or an interface reset makes the counter go backwards.
    std::fs::write(&path, SAMPLE_A.replace("eth0: 500000", "eth0: 10")).expect("write");
    let snapshot = sample(&mut collector, 1);

    for interface in &snapshot.interfaces {
        assert!(interface.receive_rate >= 0.0, "{} rx", interface.name);
        assert!(interface.transmit_rate >= 0.0, "{} tx", interface.name);
        assert!(interface.receive_rate.is_finite());
    }
    assert_eq!(snapshot.find("eth0").expect("eth0").receive_rate, 0.0);
}

#[test]
fn a_disappearing_interface_is_forgotten() {
    let dir = tempfile::tempdir().expect("temp dir");
    let (path, mut collector) = collector_for(SAMPLE_A, &dir);
    let before = sample(&mut collector, 1);
    assert!(before.find("eth0").is_some());
    assert!(before.find("wlan0").is_some());

    std::fs::write(
        &path,
        "  eth0: 600000 400 0 0 0 0 0 0 250000 200 0 0 0 0 0 0\n",
    )
    .expect("write");
    let after = sample(&mut collector, 1);

    assert_eq!(after.interfaces.len(), 1);
    assert!(
        after.find("wlan0").is_none(),
        "a vanished interface must not linger"
    );
    assert!(after.find("eth0").is_some());
}

#[test]
fn an_appearing_interface_starts_from_zero() {
    let dir = tempfile::tempdir().expect("temp dir");
    let (path, mut collector) =
        collector_for("  eth0: 100 1 0 0 0 0 0 0 100 1 0 0 0 0 0 0\n", &dir);
    sample(&mut collector, 1);
    std::fs::write(
        &path,
        "  eth0: 100 1 0 0 0 0 0 0 100 1 0 0 0 0 0 0\n docker0: 500 5 0 0 0 0 0 0 250 2 0 0 0 0 0 0\n",
    )
    .expect("write");
    let snapshot = sample(&mut collector, 1);

    let docker = snapshot
        .find("docker0")
        .expect("the new interface must appear");
    assert_eq!(docker.receive_rate, 0.0, "a new interface has no rate yet");
    assert_eq!(docker.total_received, 500);
}

#[test]
fn loopback_is_hidden_by_default_but_can_be_requested() {
    let dir = tempfile::tempdir().expect("temp dir");
    let (_path, mut collector) = collector_for(SAMPLE_A, &dir);
    let snapshot = sample(&mut collector, 1);
    assert!(
        snapshot.find("lo").is_none(),
        "loopback is noise by default"
    );

    let dir = tempfile::tempdir().expect("temp dir");
    let (_path, mut collector) = collector_for(SAMPLE_A, &dir);
    collector.include_loopback(true);
    let snapshot = sample(&mut collector, 1);
    let lo = snapshot
        .find("lo")
        .expect("loopback must be present when requested");
    assert!(lo.is_loopback);
}

#[test]
fn an_interface_filter_restricts_monitoring() {
    let dir = tempfile::tempdir().expect("temp dir");
    let path = dir.path().join("net_dev");
    std::fs::write(&path, SAMPLE_A).expect("write");
    let mut collector = NetworkCollector::with_path(&path, vec!["wlan0".to_string()]);
    let snapshot = sample(&mut collector, 1);

    assert_eq!(snapshot.interfaces.len(), 1);
    assert_eq!(snapshot.interfaces[0].name, "wlan0");
}

#[test]
fn a_missing_proc_net_dev_is_not_fatal() {
    let mut collector = NetworkCollector::with_path("/definitely/not/here", Vec::new());
    let snapshot = collector.sample();
    assert!(snapshot.interfaces.is_empty());
    assert_eq!(snapshot.receive_rate, 0.0);
    assert!(snapshot.receive_rate.is_finite());
}

#[test]
fn a_zero_elapsed_interval_does_not_divide_by_zero() {
    let dir = tempfile::tempdir().expect("temp dir");
    let (path, mut collector) = collector_for(SAMPLE_A, &dir);
    sample(&mut collector, 1);
    std::fs::write(&path, SAMPLE_A.replace("500000", "600000")).expect("write");

    let snapshot = sample(&mut collector, 0);
    for interface in &snapshot.interfaces {
        assert!(
            interface.receive_rate.is_finite(),
            "{} rx is not finite",
            interface.name
        );
        assert!(
            interface.transmit_rate.is_finite(),
            "{} tx is not finite",
            interface.name
        );
    }
}

#[test]
fn aggregate_rates_sum_the_interfaces() {
    let dir = tempfile::tempdir().expect("temp dir");
    let (path, mut collector) = collector_for(SAMPLE_A, &dir);
    sample(&mut collector, 1);
    let next = SAMPLE_A
        .replace("eth0: 500000", "eth0: 600000")
        .replace("wlan0:  20000", "wlan0:  40000");
    std::fs::write(&path, next).expect("write");
    let snapshot = sample(&mut collector, 1);

    let summed: f64 = snapshot.interfaces.iter().map(|i| i.receive_rate).sum();
    assert!(
        (snapshot.receive_rate - summed).abs() < 0.001,
        "aggregate {} != sum {}",
        snapshot.receive_rate,
        summed
    );
}

#[test]
fn the_running_system_exposes_a_primary_interface() {
    let snapshot = NetworkCollector::new(Vec::new()).sample();
    assert!(
        !snapshot.interfaces.is_empty(),
        "a running host always has a network interface"
    );
    for interface in &snapshot.interfaces {
        assert!(!interface.name.is_empty());
        assert!(interface.receive_rate >= 0.0);
        assert!(interface.transmit_rate >= 0.0);
    }
}

#[test]
fn interface_stats_can_be_constructed_for_the_ui() {
    // The UI builds rows from this struct; a regression here breaks the network panel.
    let interface = InterfaceStats {
        name: "eth0".to_string(),
        total_received: 100,
        total_transmitted: 50,
        receive_rate: 1.5,
        transmit_rate: 0.5,
        counters: InterfaceCounters::default(),
        mac_address: None,
        is_up: true,
        is_loopback: false,
    };
    assert_eq!(interface.total_traffic(), 150);
}
