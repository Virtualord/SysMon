//! Integration tests for the `/proc` parsers.
//!
//! Each parser is fed both a realistic sample and a set of malformed inputs, because
//! a monitor that panics on an unexpected kernel format is worse than one that shows
//! `N/A`.

use syswatch::collector::cpu::{self, CpuJiffies, CpuTimesTracker, PROC_STAT};
use syswatch::collector::memory::{self, MemInfo};
use syswatch::collector::storage::{self, MountEntry};
use syswatch::collector::system::{self, OsRelease};

/// A trimmed but structurally exact copy of a real `/proc/stat` header.
const PROC_STAT_SAMPLE: &str = "\
cpu  961474 80 177150 6807801 22789 47439 19969 0 0 0
cpu0 480737 40 88575 3403900 11394 23719 9984 0 0 0
cpu1 480737 40 88575 3403901 11395 23720 9985 0 0 0
intr 123456789
ctxt 987654321
btime 1700000000
processes 12345
procs_running 2
procs_blocked 0
";

const MEMINFO_SAMPLE: &str = "\
MemTotal:       16031548 kB
MemFree:          211396 kB
MemAvailable:    8411620 kB
Buffers:          302516 kB
Cached:          9902108 kB
SwapCached:        28928 kB
Active:          5713284 kB
Inactive:        4770612 kB
SwapTotal:       2097148 kB
SwapFree:        2097148 kB
Dirty:               364 kB
Shmem:            163896 kB
SReclaimable:     517788 kB
HugePages_Total:       0
HugePages_Free:        0
";

const MOUNTS_SAMPLE: &str = "\
/dev/nvme0n1p2 / xfs rw,relatime,attr2 0 0
/dev/nvme0n1p1 /boot/efi vfat rw,relatime 0 0
tmpfs /run tmpfs rw,nosuid,nodev,mode=755 0 0
proc /proc proc rw,nosuid,nodev,noexec 0 0
server:/export /mnt/net nfs4 rw,relatime 0 0
none /mnt/my\\040disk ext4 rw 0 0
";

const OS_RELEASE_SAMPLE: &str = r#"PRETTY_NAME="Void Linux 20250202 (rolling)"
NAME="Void Linux"
ID=void
VERSION_ID=20250202
HOME_URL="https://voidlinux.org/"
BUG_REPORT_URL="https://github.com/void-linux/void-packages/issues"
VARIANT_ID=glibc
"#;

// ---------------------------------------------------------------- CPU

#[test]
fn cpu_aggregate_line_matches_the_kernel_format() {
    let jiffies = cpu::parse_cpu_line(PROC_STAT_SAMPLE.lines().next().expect("first line"))
        .expect("the aggregate line must parse");
    assert_eq!(jiffies.user, 961_474);
    assert_eq!(jiffies.nice, 80);
    assert_eq!(jiffies.system, 177_150);
    assert_eq!(jiffies.idle, 6_807_801);
    assert_eq!(jiffies.iowait, 22_789);
    assert_eq!(jiffies.steal, 0);
    assert!(jiffies.total() >= jiffies.busy());
}

#[test]
fn cpu_per_core_lines_parse() {
    let core0 =
        cpu::parse_cpu_line("cpu0 480737 40 88575 3403900 11394 23719 9984 0 0 0").expect("core 0");
    let core1 =
        cpu::parse_cpu_line("cpu1 480737 40 88575 3403901 11395 23720 9985 0 0 0").expect("core 1");
    assert_eq!(core0.idle, 3_403_900);
    assert_eq!(core1.idle, 3_403_901);
    assert_eq!(core0.steal, 0);
}

#[test]
fn non_cpu_lines_are_rejected() {
    for line in [
        "intr 1 2 3",
        "ctxt 99",
        "btime 1700000000",
        "procs_running 2",
        "",
        "   ",
        "cpufreq 1 2 3 4",
    ] {
        assert!(
            cpu::parse_cpu_line(line).is_none(),
            "{line:?} must not parse as a CPU line"
        );
    }
}

#[test]
fn the_tracker_finds_the_aggregate_line_in_a_real_proc_stat() {
    // The separator in `/proc/stat` is two spaces; a naive `strip_prefix("cpu ")` would
    // mis-parse the line and silently disable the times breakdown.
    let dir = tempfile::tempdir().expect("temp dir");
    let path = dir.path().join("stat");
    std::fs::write(&path, PROC_STAT_SAMPLE).expect("write");

    let mut tracker = CpuTimesTracker::new();
    assert!(
        !tracker.update_from_file(&path).valid,
        "the first sample has no predecessor"
    );

    // Second sample, one second later on an 8-core host: 201 jiffies of user time
    // (8 cores * ~25 Hz user) and 500 of system time, nothing else busy.
    // Percentages are therefore deltas over the delta total: user 201/701,
    // system 500/701, everything else zero.
    let mut lines: Vec<&str> = PROC_STAT_SAMPLE.lines().collect();
    lines[0] = "cpu  961675 80 177650 6807801 22789 47439 19969 0 0 0";
    std::fs::write(&path, lines.join("\n")).expect("write");
    let times = tracker.update_from_file(&path);

    assert!(
        times.valid,
        "the aggregate line must be found despite the padding"
    );
    let user = 201.0 / 701.0 * 100.0;
    let system = 500.0 / 701.0 * 100.0;
    assert!(
        (times.user - user).abs() < 0.05,
        "user {} expected {user}",
        times.user
    );
    assert!(
        (times.system - system).abs() < 0.05,
        "system {} expected {system}",
        times.system
    );
    assert!(times.idle < 0.01, "idle was unchanged, got {}", times.idle);
    assert!(
        times.iowait < 0.01,
        "iowait was unchanged, got {}",
        times.iowait
    );
    assert!(times.user <= 100.0);
}

#[test]
fn the_running_system_exposes_a_usable_aggregate_line() {
    let jiffies =
        cpu::read_aggregate(std::path::Path::new(PROC_STAT)).expect("/proc/stat must be readable");
    assert!(
        jiffies.idle > 0,
        "a running system has accumulated idle time"
    );
    assert!(jiffies.total() > jiffies.idle);
}

#[test]
fn percentages_are_computed_between_samples_not_from_boot() {
    // A single sample must not be reported as a percentage of the boot totals.
    let mut tracker = CpuTimesTracker::new();
    let jiffies = cpu::read_aggregate(std::path::Path::new(PROC_STAT)).expect("read");
    let first = tracker.update(jiffies);
    assert!(!first.valid, "boot-time counters are not a percentage");
    assert_eq!(first.busy(), 0.0);
}

#[test]
fn a_counter_reset_does_not_produce_negative_percentages() {
    let mut tracker = CpuTimesTracker::new();
    tracker.update(CpuJiffies {
        user: 1_000,
        idle: 1_000,
        ..CpuJiffies::default()
    });
    // A wrapped or reset counter is smaller than the previous one.
    let times = tracker.update(CpuJiffies {
        user: 10,
        idle: 10,
        ..CpuJiffies::default()
    });
    for value in [
        times.user,
        times.system,
        times.idle,
        times.iowait,
        times.interrupt,
        times.steal,
    ] {
        assert!(
            (0.0..=100.0).contains(&value),
            "percentage out of range: {value}"
        );
    }
}

// ------------------------------------------------------------- Memory

#[test]
fn meminfo_fields_are_converted_to_bytes() {
    let info = memory::parse_meminfo(MEMINFO_SAMPLE);
    assert_eq!(info.total, 16_031_548 * 1024);
    assert_eq!(info.free, 211_396 * 1024);
    assert_eq!(info.available, 8_411_620 * 1024);
    assert_eq!(info.buffers, 302_516 * 1024);
    assert_eq!(info.cached, 9_902_108 * 1024);
    assert_eq!(info.shmem, 163_896 * 1024);
    assert_eq!(info.reclaimable, 517_788 * 1024);
    assert_eq!(info.swap_total, 2_097_148 * 1024);
    assert_eq!(info.swap_free, 2_097_148 * 1024);
}

#[test]
fn used_memory_excludes_reclaimable_cache() {
    // The whole point of using MemAvailable: a system with a large page cache is not
    // out of memory.
    let info = memory::parse_meminfo(MEMINFO_SAMPLE);
    let used = info.used();
    assert_eq!(used, (16_031_548 - 8_411_620) * 1024);
    assert!(
        used < info.total / 2,
        "used {} should be under half of {}",
        used,
        info.total
    );
    assert_eq!(info.swap_used(), 0);
}

#[test]
fn a_kernel_without_mem_available_gets_an_estimate() {
    let old_kernel =
        "MemTotal: 1000000 kB\nMemFree: 200000 kB\nBuffers: 50000 kB\nCached: 300000 kB\n";
    let info = memory::parse_meminfo(old_kernel);
    assert!(!info.available_present);
    assert_eq!(info.available, 550_000 * 1024);
    assert_eq!(info.used(), 450_000 * 1024);
}

#[test]
fn malformed_meminfo_lines_are_skipped() {
    let garbage = "\
nonsense
MemTotal: not-a-number kB
MemTotal: 2000000 kB
MemFree:
MemAvailable: 1000000 kB
";
    let info = memory::parse_meminfo(garbage);
    assert_eq!(info.total, 2_000_000 * 1024, "the last valid MemTotal wins");
    assert_eq!(info.free, 0);
    assert_eq!(info.available, 1_000_000 * 1024);
}

#[test]
fn a_planted_meminfo_file_is_read_without_touching_the_system() {
    let dir = tempfile::tempdir().expect("temp dir");
    let path = dir.path().join("meminfo");
    std::fs::write(&path, MEMINFO_SAMPLE).expect("write");
    let info: MemInfo = memory::read_meminfo(&path).expect("read planted file");
    assert_eq!(info.total, 16_031_548 * 1024);
    assert!(memory::read_meminfo(std::path::Path::new("/definitely/not/here")).is_none());
}

// ------------------------------------------------------------ Storage

#[test]
fn mounts_parse_with_escaped_paths() {
    let entries = storage::parse_proc_mounts(MOUNTS_SAMPLE);
    assert_eq!(entries.len(), 6);

    let escaped = entries
        .iter()
        .find(|entry| entry.file_system == "ext4")
        .expect("the ext4 mount");
    assert_eq!(escaped.mount_point, std::path::Path::new("/mnt/my disk"));
    assert_eq!(escaped.device, "none");
}

#[test]
fn malformed_mount_lines_are_skipped() {
    let entries = storage::parse_proc_mounts(
        "garbage\n/dev/sda1 / ext4 rw 0 0\nonly three fields\n/dev/sdb1\n",
    );
    assert_eq!(entries.len(), 1);
    assert_eq!(entries[0].mount_point, std::path::Path::new("/"));
}

#[test]
fn mounts_are_classified() {
    for (device, fs, expected) in [
        ("/dev/nvme0n1p2", "xfs", storage::FilesystemKind::Physical),
        ("/dev/sda1", "ext4", storage::FilesystemKind::Physical),
        ("/dev/loop0", "ext4", storage::FilesystemKind::Virtual),
        (
            "/dev/mapper/vg-root",
            "ext4",
            storage::FilesystemKind::Virtual,
        ),
        ("server:/export", "nfs4", storage::FilesystemKind::Network),
        ("//server/share", "cifs", storage::FilesystemKind::Network),
        ("proc", "proc", storage::FilesystemKind::Pseudo),
        ("tmpfs", "tmpfs", storage::FilesystemKind::Pseudo),
        ("shm", "overlay", storage::FilesystemKind::Pseudo),
        ("gizmo", "unheard-of", storage::FilesystemKind::Unknown),
    ] {
        assert_eq!(
            storage::classify(device, fs),
            expected,
            "device {device:?} fs {fs:?}"
        );
    }
}

#[test]
fn duplicate_mount_points_collapse_to_the_visible_mount() {
    // An overmount hides the first entry; showing both would double count the space.
    let entries: Vec<MountEntry> =
        storage::parse_proc_mounts("/dev/sda1 /mnt ext4 rw 0 0\ntmpfs /mnt tmpfs rw 0 0\n");
    let mut collector = storage::StorageCollector::with_mounts("/definitely/not/here", true);
    let filesystems = collector.from_mounts(&entries);
    assert_eq!(filesystems.len(), 1);
    assert_eq!(filesystems[0].name, "/dev/sda1");
}

#[test]
fn read_only_mounts_are_flagged() {
    let entries = storage::parse_proc_mounts("/dev/sda2 /mnt/ro ext4 ro,relatime 0 0\n");
    let mut collector = storage::StorageCollector::with_mounts("/definitely/not/here", true);
    let filesystems = collector.from_mounts(&entries);
    assert!(
        filesystems[0].read_only,
        "a read-only mount must be flagged"
    );
}

#[test]
fn a_missing_mount_table_yields_no_filesystems() {
    let mut collector = storage::StorageCollector::with_mounts("/definitely/not/here", true);
    assert!(collector.sample().is_empty());
}

#[test]
fn the_running_system_has_a_root_filesystem() {
    let filesystems = storage::StorageCollector::new(false).sample();
    assert!(
        filesystems
            .iter()
            .any(|fs| fs.mount_point == std::path::Path::new("/"))
    );
    assert!(
        filesystems
            .iter()
            .all(|fs| fs.kind != storage::FilesystemKind::Pseudo)
    );
}

// ------------------------------------------------------------- System

#[test]
fn os_release_parses_a_realistic_document() {
    let info = system::parse_os_release(OS_RELEASE_SAMPLE);
    assert_eq!(info.id.as_deref(), Some("void"));
    assert_eq!(info.name.as_deref(), Some("Void Linux"));
    // Void publishes VERSION_ID but not VERSION.
    assert_eq!(info.version, None);
    assert_eq!(info.version_id.as_deref(), Some("20250202"));
    assert_eq!(
        info.describe().as_deref(),
        Some("Void Linux 20250202 (rolling)")
    );
}

#[test]
fn os_release_handles_the_distributions_syswatch_targets() {
    let arch = system::parse_os_release("NAME=\"Arch Linux\"\nID=arch\nBUILD_ID=rolling\n");
    assert_eq!(arch.id.as_deref(), Some("arch"));
    assert_eq!(arch.describe().as_deref(), Some("Arch Linux"));

    let fedora = system::parse_os_release(
        "NAME=\"Fedora Linux\"\nVERSION=\"41 (Workstation Edition)\"\nID=fedora\nVERSION_ID=41\n",
    );
    assert_eq!(fedora.id.as_deref(), Some("fedora"));
    assert_eq!(
        fedora.describe().as_deref(),
        Some("Fedora Linux 41 (Workstation Edition)")
    );
}

#[test]
fn os_release_tolerates_junk() {
    for document in [
        "",
        "# only a comment\n",
        "NO_EQUALS_SIGN\n",
        "ID=\n",
        "ID=\"\"",
    ] {
        let info: OsRelease = system::parse_os_release(document);
        assert_eq!(
            info.describe(),
            None,
            "document {document:?} should describe nothing"
        );
    }
}

#[test]
fn loadavg_task_counts_come_from_the_fourth_field() {
    let dir = tempfile::tempdir().expect("temp dir");
    let path = dir.path().join("loadavg");

    std::fs::write(&path, "0.42 0.31 0.20 1/234 5678\n").expect("write");
    assert_eq!(system::read_loadavg_tasks(&path), (Some(1), Some(234)));

    std::fs::write(&path, "0.00 0.00 0.00\n").expect("write");
    assert_eq!(system::read_loadavg_tasks(&path), (None, None));

    assert_eq!(
        system::read_loadavg_tasks(std::path::Path::new("/definitely/not/here")),
        (None, None)
    );
}

#[test]
fn the_running_system_reports_its_own_identity() {
    let mut system = sysinfo::System::new();
    system.refresh_cpu_usage();
    system.refresh_memory();
    let info = system::SystemCollector::default().static_info(&system);

    assert!(!info.hostname.is_empty(), "the hostname must resolve");
    assert!(!info.kernel.is_empty(), "the kernel version must resolve");
    assert!(!info.architecture.is_empty());
    assert!(info.logical_cores >= 1);
    assert!(info.total_memory > 0);
}

#[test]
fn temperatures_are_optional_and_never_required() {
    let mut components = sysinfo::Components::new_with_refreshed_list();
    let info = system::SystemCollector::default().dynamic_info(&mut components);
    // The point of the assertion: no sensor is fine, and the values that exist are
    // physically plausible.
    for temperature in &info.temperatures {
        assert!(
            (-50.0..150.0).contains(&temperature.celsius),
            "implausible reading: {temperature:?}"
        );
    }
    assert!(info.load_one.is_finite());
    assert!(info.uptime > 0);
}
