//! Physical disk inventory and health, read from sysfs.
//!
//! ## What is available without root
//!
//! The kernel exposes a surprising amount about a drive through sysfs, and all of it
//! is world readable:
//!
//! | File | Meaning |
//! | --- | --- |
//! | `/sys/block/<dev>/queue/rotational` | `1` for a spinning disk, `0` for solid state |
//! | `/sys/block/<dev>/size` | capacity in 512-byte sectors |
//! | `/sys/block/<dev>/device/model` | marketing name, trimmed of its padding |
//! | `/sys/block/<dev>/device/firmware_rev` | firmware revision |
//! | `/sys/block/<dev>/device/serial` | serial number |
//! | `/sys/block/<dev>/device/state` | NVMe controller state, e.g. `live` |
//! | `/sys/class/hwmon/hwmon*/temp1_input` | temperature in millidegrees Celsius |
//! | `/sys/class/hwmon/hwmon*/temp1_crit` | critical threshold, the thermal shutdown point |
//!
//! ## What is not, and why this module stops here
//!
//! The interesting wear numbers — percentage used, media errors, power-on hours,
//! data units written — live in the **NVMe SMART log page**, which is *not* a file.
//! It is an NVMe admin command (`NVME_IOCTL_ADMIN_CMD`) issued through a raw ioctl on
//! `/dev/nvme*`, and opening that device requires root. There is no sysfs file for it.
//!
//! Getting those numbers would mean one of:
//!
//! * shelling out to `nvme smart-log`, which needs root or a setuid helper and breaks
//!   this program's promise never to run an external command,
//! * hand-rolling the admin ioctl, which adds `unsafe` code around a 512-byte
//!   protocol buffer and still needs root,
//! * linking `libsmartctl`, which adds a C dependency.
//!
//! None of that is worth it for a dashboard, so this module reports what the kernel
//! publishes and `scripts/disk-health.sh` covers the rest on demand. The one number
//! here that genuinely predicts a failure is the critical temperature threshold: it
//! is the point at which the drive will throttle itself or shut down to protect its
//! data.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

/// Sectors are always reported in 512-byte units by the kernel, regardless of the
/// drive's physical sector size.
const SECTOR_BYTES: u64 = 512;

/// Default location of the block device class.
const SYS_BLOCK: &str = "/sys/block";
/// Default location of the hwmon class.
const SYS_HWMON: &str = "/sys/class/hwmon";

/// Name prefixes for block devices that are not physical disks.
///
/// `/sys/block` only lists whole disks, never partitions, so partitions need no
/// filtering here. Virtual devices do appear there though, and showing a zram or a
/// loop device in a disk health table is noise at best and alarming at worst.
const VIRTUAL_PREFIXES: &[&str] = &["loop", "zram", "ram", "dm-", "sr", "md", "fd"];

/// One temperature channel reported by a hwmon device.
#[derive(Debug, Clone, PartialEq)]
pub struct SensorReading {
    /// Channel name from `tempN_label`, for example `Composite`.
    pub label: Option<String>,
    /// Current temperature in degrees Celsius.
    pub celsius: f64,
    /// Maximum operating temperature from `tempN_max`.
    pub max_celsius: Option<f64>,
    /// Critical temperature from `tempN_crit`, the thermal shutdown point.
    pub critical_celsius: Option<f64>,
}

/// A physical disk and the health the kernel publishes about it.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct DiskDevice {
    /// Kernel device name, for example `nvme0n1` or `sda`.
    pub name: String,
    /// Marketing name, e.g. `INTEL HBRPEKNX0202A`. The kernel pads this with spaces.
    pub model: Option<String>,
    /// Firmware revision.
    pub firmware: Option<String>,
    /// Serial number.
    pub serial: Option<String>,
    /// Bus, for NVMe this is `pcie`, `sata` or `usb`.
    pub transport: Option<String>,
    /// `false` for solid state, `true` for spinning rust.
    pub rotational: bool,
    /// Capacity in bytes.
    pub size_bytes: u64,
    /// NVMe controller state, e.g. `live`.
    pub state: Option<String>,
    /// Every temperature channel the drive exposes, in driver order.
    pub temperatures: Vec<SensorReading>,
}

impl DiskDevice {
    /// Whether the kernel reports this as a non-rotational device.
    pub fn is_ssd(&self) -> bool {
        !self.rotational
    }

    /// The primary temperature, preferring the channel the driver labels `Composite`.
    ///
    /// An NVMe drive usually reports a single composite sensor; a SATA SSD exposes one
    /// per device. `Composite` is the one that reflects the whole device, so it wins
    /// over the alternatives.
    pub fn primary_temperature(&self) -> Option<&SensorReading> {
        self.temperatures
            .iter()
            .find(|reading| {
                reading
                    .label
                    .as_deref()
                    .is_some_and(|label| label.eq_ignore_ascii_case("composite"))
            })
            .or_else(|| self.temperatures.first())
    }

    /// A one word summary for the storage table.
    pub fn kind_label(&self) -> &'static str {
        if self.rotational { "HDD" } else { "SSD" }
    }
}

/// Parses `queue/rotational`.
///
/// The kernel writes `0` or `1`. Anything else is treated as unknown rather than
/// guessed at, because claiming a spinning disk is an SSD is the wrong way to be
/// wrong.
pub fn parse_rotational(text: &str) -> Option<bool> {
    match text.trim() {
        "0" => Some(false),
        "1" => Some(true),
        _ => None,
    }
}

/// Parses `size`, which the kernel reports in 512-byte sectors.
pub fn parse_sector_count(text: &str) -> Option<u64> {
    let sectors: u64 = text.trim().parse().ok()?;
    sectors.checked_mul(SECTOR_BYTES)
}

/// Parses a `tempN_input` value, which the kernel reports in millidegrees Celsius.
pub fn parse_millidegrees(text: &str) -> Option<f64> {
    let value: f64 = text.trim().parse().ok()?;
    // A driver reporting thousands of degrees is broken, not a hot disk; refusing the
    // value keeps the UI from showing nonsense.
    (-100.0..=300.0)
        .contains(&(value / 1000.0))
        .then(|| value / 1000.0)
}

/// Trims the space the kernel pads `model` and `firmware_rev` with.
pub fn clean_label(text: &str) -> Option<String> {
    let trimmed = text.trim();
    (!trimmed.is_empty()).then(|| trimmed.to_string())
}

/// Whether a `/sys/block` entry is a physical disk rather than a virtual device.
pub fn is_physical_disk(name: &str) -> bool {
    !name.is_empty()
        && !VIRTUAL_PREFIXES
            .iter()
            .any(|prefix| name.starts_with(prefix))
}

/// Reads the temperature channels of one hwmon device.
fn read_hwmon_temperatures(hwmon: &Path) -> Vec<SensorReading> {
    let mut readings = Vec::new();
    let Ok(entries) = std::fs::read_dir(hwmon) else {
        return readings;
    };
    let mut names: Vec<String> = entries
        .filter_map(|entry| entry.ok())
        .map(|entry| entry.file_name().to_string_lossy().into_owned())
        .filter(|name| name.starts_with("temp") && name.ends_with("_input"))
        .collect();
    // temp10_input would sort before temp2_input lexicographically; numeric order is
    // what a driver listing implies, and it keeps the display stable.
    names.sort_by_key(|name| {
        name.trim_start_matches("temp")
            .trim_end_matches("_input")
            .parse::<u32>()
            .unwrap_or(u32::MAX)
    });

    for input in names {
        let Ok(text) = std::fs::read_to_string(hwmon.join(&input)) else {
            continue;
        };
        let Some(celsius) = parse_millidegrees(&text) else {
            continue;
        };
        let channel = input.trim_start_matches("temp").trim_end_matches("_input");
        let label = std::fs::read_to_string(hwmon.join(format!("temp{channel}_label")))
            .ok()
            .and_then(|text| clean_label(&text));
        let max_celsius = read_optional(hwmon.join(format!("temp{channel}_max")))
            .and_then(|text| parse_millidegrees(&text));
        let critical_celsius = read_optional(hwmon.join(format!("temp{channel}_crit")))
            .and_then(|text| parse_millidegrees(&text));
        readings.push(SensorReading {
            label,
            celsius,
            max_celsius,
            critical_celsius,
        });
    }
    readings
}

/// Reads a sysfs attribute, treating an absent one as `None`.
fn read_optional(path: PathBuf) -> Option<String> {
    std::fs::read_to_string(path).ok()
}

/// Resolves a device symlink to a canonical path so a block device and its hwmon
/// sensor can be matched.
///
/// Both sides have to be canonicalised: `/sys/class/hwmon/hwmon0/device` resolves to
/// `/sys/class/nvme/nvme0`, which is itself a symlink into `/sys/devices/...`, while
/// `/sys/block/nvme0n1/device` goes straight there. Only the fully resolved paths are
/// guaranteed to match.
fn canonical_device(link: &Path) -> Option<PathBuf> {
    let device = link.join("device");
    if !device.exists() {
        return None;
    }
    std::fs::canonicalize(&device).ok()
}

/// Collects disk inventory and health.
pub struct DiskCollector {
    sys_block: PathBuf,
    sys_hwmon: PathBuf,
}

impl Default for DiskCollector {
    fn default() -> Self {
        Self::new(SYS_BLOCK, SYS_HWMON)
    }
}

impl DiskCollector {
    /// Creates a collector with explicit sysfs roots, which keeps the tests hermetic.
    pub fn new(sys_block: impl Into<PathBuf>, sys_hwmon: impl Into<PathBuf>) -> Self {
        Self {
            sys_block: sys_block.into(),
            sys_hwmon: sys_hwmon.into(),
        }
    }

    /// Reads every physical disk the kernel knows about.
    pub fn sample(&self) -> Vec<DiskDevice> {
        let sensors = self.read_sensors();
        let mut disks: Vec<DiskDevice> = self
            .read_entries()
            .into_iter()
            .filter_map(|entry| self.describe(&entry, &sensors))
            .collect();

        // Bus path is the same for every disk behind one controller, so it is read from
        // the sysfs tree rather than from a per-device attribute.
        for disk in &mut disks {
            disk.transport = self.transport_for(&disk.name);
        }
        // Newest and largest first: on a workstation the NVMe drives are the
        // interesting ones and they are not necessarily nvme0.
        disks.sort_by(|a, b| {
            b.size_bytes
                .cmp(&a.size_bytes)
                .then_with(|| natural_order(&a.name, &b.name))
        });
        disks
    }

    /// The `/sys/block` entries that are physical disks.
    ///
    /// `/sys/block` lists whole disks only — partitions live in `/sys/class/block` —
    /// so there is no partition filtering to do here.
    fn read_entries(&self) -> Vec<PathBuf> {
        let Ok(entries) = std::fs::read_dir(&self.sys_block) else {
            tracing::debug!(path = %self.sys_block.display(), "cannot read block devices");
            return Vec::new();
        };
        entries
            .filter_map(|entry| entry.ok())
            .map(|entry| entry.path())
            .filter(|path| {
                let name = path
                    .file_name()
                    .map(|name| name.to_string_lossy().into_owned());
                name.is_some_and(|name| is_physical_disk(&name))
            })
            .collect()
    }

    /// Builds one [`DiskDevice`] from a sysfs entry.
    fn describe(
        &self,
        entry: &Path,
        sensors: &HashMap<PathBuf, Vec<SensorReading>>,
    ) -> Option<DiskDevice> {
        let name = entry.file_name()?.to_string_lossy().into_owned();
        let device = entry.join("device");
        let rotational = read_optional(entry.join("queue/rotational"))
            .and_then(|text| parse_rotational(&text))
            .unwrap_or(false);
        let size_bytes = read_optional(entry.join("size"))
            .and_then(|text| parse_sector_count(&text))
            .unwrap_or(0);

        // A device with neither a size nor a model is not something we can say
        // anything useful about.
        if size_bytes == 0 && !device.join("model").exists() {
            return None;
        }

        let temperatures = canonical_device(entry)
            .and_then(|key| sensors.get(&key).cloned())
            .unwrap_or_default();

        Some(DiskDevice {
            model: read_optional(device.join("model"))
                .as_deref()
                .and_then(clean_label),
            firmware: read_optional(device.join("firmware_rev"))
                .as_deref()
                .and_then(clean_label),
            serial: read_optional(device.join("serial"))
                .as_deref()
                .and_then(clean_label),
            state: read_optional(device.join("state"))
                .as_deref()
                .and_then(clean_label),
            transport: None,
            temperatures,
            name,
            rotational,
            size_bytes,
        })
    }

    /// The bus a drive is attached to, for example `pcie` for NVMe.
    ///
    /// There is no per-block-device attribute for this, so it is derived from the
    /// canonical device path: an NVMe controller is a `nvme` node under the PCI tree,
    /// a SATA drive sits under `ata`, and so on.
    fn transport_for(&self, block_name: &str) -> Option<String> {
        // The kernel name covers the common case without depending on a driver having
        // built a particular directory tree.
        if block_name.starts_with("nvme") {
            return Some("nvme".to_string());
        }
        if block_name.starts_with("mmcblk") {
            return Some("mmc".to_string());
        }
        // Otherwise look for the bus node above the drive: an `ata`, `usb` or `sas`
        // controller sits between the PCI tree and the block device.
        let canonical = canonical_device(&self.sys_block.join(block_name))?;
        let components: Vec<String> = canonical
            .components()
            .map(|c| c.as_os_str().to_string_lossy().into_owned())
            .collect();
        ["usb", "ata", "sas", "mmc"]
            .into_iter()
            .find(|kind| components.iter().any(|component| component == kind))
            .map(str::to_string)
    }

    /// Maps a canonical device path to its temperature channels.
    fn read_sensors(&self) -> HashMap<PathBuf, Vec<SensorReading>> {
        let mut sensors = HashMap::new();
        let Ok(entries) = std::fs::read_dir(&self.sys_hwmon) else {
            tracing::debug!(path = %self.sys_hwmon.display(), "cannot read hwmon devices");
            return sensors;
        };
        for entry in entries.filter_map(|entry| entry.ok()) {
            let path = entry.path();
            // Only devices that belong to something countable; CPU and chassis
            // sensors are reported by the system panel already.
            let Some(key) = canonical_device(&path) else {
                continue;
            };
            let readings = read_hwmon_temperatures(&path);
            if !readings.is_empty() {
                sensors.insert(key, readings);
            }
        }
        sensors
    }
}

/// Compares two kernel names by their trailing number.
///
/// Plain lexicographic order puts `nvme10n1` before `nvme2n1`, which is not what a
/// reader expects.
fn natural_order(a: &str, b: &str) -> std::cmp::Ordering {
    let split = |name: &str| -> (String, u64) {
        let digits_start = name.find(|c: char| c.is_ascii_digit());
        match digits_start {
            Some(index) => {
                let prefix = name[..index].to_string();
                let digits: String = name[index..]
                    .chars()
                    .take_while(char::is_ascii_digit)
                    .collect();
                (prefix, digits.parse().unwrap_or(u64::MAX))
            }
            None => (name.to_string(), u64::MAX),
        }
    };
    let (prefix_a, number_a) = split(a);
    let (prefix_b, number_b) = split(b);
    prefix_a
        .cmp(&prefix_b)
        .then_with(|| number_a.cmp(&number_b))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    /// Builds a fake sysfs tree: a block device with the given attributes, and an
    /// hwmon device wired to it through a real symlink.
    struct FakeSysfs {
        dir: tempfile::TempDir,
    }

    impl FakeSysfs {
        fn new() -> Self {
            let dir = tempfile::tempdir().expect("temp dir");
            std::fs::create_dir_all(dir.path().join("block")).expect("create block");
            std::fs::create_dir_all(dir.path().join("hwmon")).expect("create hwmon");
            Self { dir }
        }

        fn write(&self, relative: &str, contents: &str) {
            let path = self.dir.path().join(relative);
            std::fs::create_dir_all(path.parent().expect("parent")).expect("create dir");
            let mut file = std::fs::File::create(&path).expect("create file");
            file.write_all(contents.as_bytes()).expect("write");
        }

        fn symlink(&self, target: &str, link: &str) {
            let link = self.dir.path().join(link);
            std::fs::create_dir_all(link.parent().expect("parent")).expect("create dir");
            let _ = std::os::unix::fs::symlink(self.dir.path().join(target), &link);
        }

        fn block_path(&self) -> PathBuf {
            self.dir.path().join("block")
        }

        fn hwmon_path(&self) -> PathBuf {
            self.dir.path().join("hwmon")
        }

        fn collector(&self) -> DiskCollector {
            DiskCollector::new(self.block_path(), self.hwmon_path())
        }
    }

    /// Wires a whole-disk sysfs entry with an NVMe-style hwmon sensor.
    fn add_nvme(fixture: &FakeSysfs, disk: &str, model: &str, rotational: &str, sectors: &str) {
        fixture.write(&format!("block/{disk}/queue/rotational"), rotational);
        fixture.write(&format!("block/{disk}/size"), sectors);
        fixture.write(&format!("block/{disk}/device/model"), model);
        fixture.write(&format!("block/{disk}/device/firmware_rev"), "G001");
        fixture.write(&format!("block/{disk}/device/serial"), "BTTE908303VG");
        fixture.write(&format!("block/{disk}/device/state"), "live");
        // The hwmon device points at the same place the block device does: the real
        // /sys/class/hwmon/hwmonN/device and /sys/block/<dev>/device both resolve to
        // the controller node, and only the fully resolved paths are comparable.
        fixture.symlink(&format!("block/{disk}/device"), "hwmon/hwmon0/device");
        fixture.write("hwmon/hwmon0/name", "nvme");
        fixture.write("hwmon/hwmon0/temp1_input", "30900");
        fixture.write("hwmon/hwmon0/temp1_label", "Composite\n");
        fixture.write("hwmon/hwmon0/temp1_crit", "80000");
        fixture.write("hwmon/hwmon0/temp1_max", "70000");
    }

    #[test]
    fn parses_the_rotational_flag() {
        assert_eq!(
            parse_rotational("0"),
            Some(false),
            "0 is a solid state drive"
        );
        assert_eq!(parse_rotational("1"), Some(true));
        assert_eq!(parse_rotational(" 1 \n"), Some(true));
        assert_eq!(
            parse_rotational("2"),
            None,
            "an unknown value is not guessed at"
        );
        assert_eq!(parse_rotational(""), None);
    }

    #[test]
    fn sector_counts_convert_to_bytes() {
        assert_eq!(parse_sector_count("1000215216"), Some(1_000_215_216 * 512));
        assert_eq!(parse_sector_count(" 512 \n"), Some(512 * 512));
        assert_eq!(parse_sector_count("-1"), None);
        assert_eq!(parse_sector_count("huge"), None);
        assert_eq!(
            parse_sector_count("18446744073709551615"),
            None,
            "overflow is rejected"
        );
    }

    #[test]
    fn millidegrees_convert_to_celsius() {
        assert_eq!(parse_millidegrees("30900"), Some(30.9));
        assert_eq!(parse_millidegrees("0"), Some(0.0));
        assert_eq!(parse_millidegrees("-5000"), Some(-5.0));
        assert_eq!(
            parse_millidegrees("999999"),
            None,
            "an absurd reading is refused"
        );
        assert_eq!(parse_millidegrees("nonsense"), None);
    }

    #[test]
    fn labels_are_trimmed_and_empty_rejected() {
        assert_eq!(
            clean_label("INTEL HBRPEKNX0202A     \n"),
            Some("INTEL HBRPEKNX0202A".to_string())
        );
        assert_eq!(clean_label("   "), None);
    }

    #[test]
    fn virtual_devices_are_filtered_out() {
        for virtual_device in ["loop0", "zram0", "ram0", "dm-0", "sr0", "md0", "fd0"] {
            assert!(
                !is_physical_disk(virtual_device),
                "{virtual_device} is not a disk"
            );
        }
        for disk in ["nvme0n1", "sda", "sdb", "vda", "mmcblk0"] {
            assert!(is_physical_disk(disk), "{disk} is a disk");
        }
        assert!(!is_physical_disk(""));
    }

    #[test]
    fn reads_a_whole_disk_with_its_temperature() {
        let fixture = FakeSysfs::new();
        add_nvme(
            &fixture,
            "nvme0n1",
            "INTEL HBRPEKNX0202A",
            "0",
            "1000215216",
        );

        let disks = fixture.collector().sample();
        assert_eq!(disks.len(), 1);
        let disk = &disks[0];
        assert_eq!(disk.name, "nvme0n1");
        assert_eq!(disk.model.as_deref(), Some("INTEL HBRPEKNX0202A"));
        assert_eq!(disk.firmware.as_deref(), Some("G001"));
        assert_eq!(disk.serial.as_deref(), Some("BTTE908303VG"));
        assert_eq!(disk.state.as_deref(), Some("live"));
        assert!(disk.is_ssd());
        assert_eq!(disk.size_bytes, 1_000_215_216 * 512);
        assert_eq!(disk.transport.as_deref(), Some("nvme"));

        let reading = disk
            .primary_temperature()
            .expect("temperature must be attributed");
        assert_eq!(reading.label.as_deref(), Some("Composite"));
        assert!((reading.celsius - 30.9).abs() < 0.001);
        assert!((reading.critical_celsius.expect("critical") - 80.0).abs() < 0.001);
        assert!((reading.max_celsius.expect("max") - 70.0).abs() < 0.001);
    }

    #[test]
    fn attributes_each_sensor_to_its_own_drive() {
        let fixture = FakeSysfs::new();
        add_nvme(
            &fixture,
            "nvme0n1",
            "INTEL HBRPEKNX0202A",
            "0",
            "1000215216",
        );

        // A second drive, with its own sensor at a different temperature.
        fixture.write("block/nvme1n1/queue/rotational", "0");
        fixture.write("block/nvme1n1/size", "57149440");
        fixture.write("block/nvme1n1/device/model", "INTEL HBRPEKNX0202AO");
        fixture.symlink("block/nvme1n1/device", "hwmon/hwmon1/device");
        fixture.write("hwmon/hwmon1/name", "nvme");
        fixture.write("hwmon/hwmon1/temp1_input", "41800");
        fixture.write("hwmon/hwmon1/temp1_label", "Composite");

        let disks = fixture.collector().sample();
        assert_eq!(disks.len(), 2);

        let first = disks
            .iter()
            .find(|d| d.name == "nvme0n1")
            .expect("first drive");
        let second = disks
            .iter()
            .find(|d| d.name == "nvme1n1")
            .expect("second drive");
        let first_temp = first.primary_temperature().expect("first temperature");
        let second_temp = second.primary_temperature().expect("second temperature");
        assert!(
            (first_temp.celsius - 30.9).abs() < 0.001,
            "the drives must not share a sensor"
        );
        assert!((second_temp.celsius - 41.8).abs() < 0.001);
        assert_eq!(
            second
                .primary_temperature()
                .expect("critical")
                .critical_celsius,
            None
        );
    }

    #[test]
    fn drives_are_listed_largest_first() {
        let fixture = FakeSysfs::new();
        add_nvme(&fixture, "nvme1n1", "SMALL", "0", "57149440");
        add_nvme(&fixture, "nvme0n1", "LARGE", "0", "1000215216");
        let disks = fixture.collector().sample();
        assert_eq!(disks.len(), 2);
        assert_eq!(disks[0].name, "nvme0n1", "the larger drive comes first");
    }

    #[test]
    fn spinning_disks_are_reported_as_such() {
        let fixture = FakeSysfs::new();
        fixture.write("block/sda/queue/rotational", "1");
        fixture.write("block/sda/size", "1953525168");
        fixture.write("block/sda/device/model", "SAMSUNG HDD");
        let disks = fixture.collector().sample();
        assert_eq!(disks.len(), 1);
        assert!(disks[0].rotational);
        assert!(!disks[0].is_ssd());
        assert_eq!(disks[0].kind_label(), "HDD");
        // A drive with no sensor simply has no temperatures.
        assert!(disks[0].primary_temperature().is_none());
    }

    #[test]
    fn virtual_devices_never_appear() {
        let fixture = FakeSysfs::new();
        add_nvme(&fixture, "nvme0n1", "REAL", "0", "1000215216");
        fixture.write("block/zram0/queue/rotational", "0");
        fixture.write("block/zram0/size", "8388608");
        fixture.write("block/zram0/device/model", "zram");
        fixture.write("block/loop0/queue/rotational", "0");
        fixture.write("block/loop0/size", "100000");
        fixture.write("block/loop0/device/model", "loop");

        let disks = fixture.collector().sample();
        assert_eq!(disks.len(), 1);
        assert_eq!(disks[0].name, "nvme0n1");
    }

    #[test]
    fn a_missing_sysfs_is_not_fatal() {
        let collector = DiskCollector::new("/definitely/not/here", "/definitely/not/here/");
        assert!(collector.sample().is_empty());
    }

    #[test]
    fn composite_wins_over_other_channels() {
        let disk = DiskDevice {
            temperatures: vec![
                SensorReading {
                    label: Some("Sensor 1".into()),
                    celsius: 40.0,
                    max_celsius: None,
                    critical_celsius: None,
                },
                SensorReading {
                    label: Some("Composite".into()),
                    celsius: 35.0,
                    max_celsius: None,
                    critical_celsius: None,
                },
            ],
            ..DiskDevice::default()
        };
        let primary = disk.primary_temperature().expect("a reading");
        assert_eq!(primary.label.as_deref(), Some("Composite"));
    }

    #[test]
    fn a_drive_without_a_composite_falls_back_to_the_first_channel() {
        let disk = DiskDevice {
            temperatures: vec![SensorReading {
                label: Some("Sensor 1".into()),
                celsius: 40.0,
                max_celsius: None,
                critical_celsius: None,
            }],
            ..DiskDevice::default()
        };
        assert_eq!(disk.primary_temperature().expect("a reading").celsius, 40.0);
    }

    #[test]
    fn natural_order_sorts_by_trailing_number() {
        let mut names = vec!["nvme10n1", "nvme2n1", "nvme1n1", "sda", "sdb"];
        names.sort_by(|a, b| natural_order(a, b));
        assert_eq!(names, vec!["nvme1n1", "nvme2n1", "nvme10n1", "sda", "sdb"]);
    }

    #[test]
    fn the_running_system_reports_its_real_disks() {
        let disks = DiskCollector::default().sample();
        // A machine with no /sys at all is the only acceptable way to get nothing.
        if std::path::Path::new(SYS_BLOCK).exists() {
            assert!(
                !disks.is_empty(),
                "a running Linux host always has a block device"
            );
        }
        for disk in &disks {
            assert!(
                is_physical_disk(&disk.name),
                "{} is not a physical disk",
                disk.name
            );
            assert!(disk.size_bytes > 0, "{} reported no capacity", disk.name);
            // Everything here comes from an unprivileged read, so nothing may be root
            // owned in a way that would make it unreadable.
            for reading in &disk.temperatures {
                assert!(
                    (-100.0..=300.0).contains(&reading.celsius),
                    "{} has a broken reading",
                    disk.name
                );
            }
        }
    }
}
