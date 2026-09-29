//! Storage sampling.
//!
//! `statvfs` cannot tell a spinning disk from a squashfs image or a network mount, so
//! the filesystems are classified by device name and filesystem type. The storage
//! panel shows physical disks first and marks everything else as virtual, network or
//! pseudo, because a 100% "full" `/snap` squashfs mount is not a hardware problem.

use std::collections::HashSet;
use std::path::{Path, PathBuf};

use nix::sys::statvfs::statvfs;

/// Path of the mount table used to enumerate filesystems.
const PROC_MOUNTS: &str = "/proc/mounts";

/// Filesystem types that are never backed by a physical disk.
const PSEUDO_FILESYSTEMS: &[&str] = &[
    "autofs",
    "binfmt_misc",
    "bpf",
    "cgroup",
    "cgroup2",
    "configfs",
    "debugfs",
    "devpts",
    "devtmpfs",
    "efivarfs",
    "fuse.gvfsd-fuse",
    "fuse.portal",
    "fusectl",
    "hugetlbfs",
    "mqueue",
    "overlay",
    "proc",
    "pstore",
    "ramfs",
    "rpc_pipefs",
    "securityfs",
    "selinuxfs",
    "squashfs",
    "sysfs",
    "tmpfs",
    "tracefs",
];

/// Filesystem types that live on a remote host.
const NETWORK_FILESYSTEMS: &[&str] = &[
    "afs",
    "ceph",
    "cifs",
    "coda",
    "fuse.sshfs",
    "fuse.glusterfs",
    "fuse.s3fs",
    "gfs",
    "gfs2",
    "ncp",
    "nfs",
    "nfs4",
    "smb3",
    "smbfs",
    "9p",
    "virtiofs",
];

/// Device name prefixes used by virtual machines and container runtimes.
const VIRTUAL_DEVICES: &[&str] = &[
    "/dev/loop",
    "/dev/ram",
    "/dev/zram",
    "/dev/mapper/",
    "/dev/dm-",
    "/dev/md",
    "/dev/vd",
    "/dev/xvd",
];

/// What is backing a mount point.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum FilesystemKind {
    /// Block device or plain file image.
    Physical,
    /// Virtual machine disk, loopback or device mapper target.
    Virtual,
    /// Mounted from a remote host.
    Network,
    /// Kernel pseudo filesystem.
    Pseudo,
    /// Could not be classified.
    #[default]
    Unknown,
}

impl FilesystemKind {
    /// Short label rendered in the storage table.
    pub fn label(&self) -> &'static str {
        match self {
            Self::Physical => "physical",
            Self::Virtual => "virtual",
            Self::Network => "network",
            Self::Pseudo => "pseudo",
            Self::Unknown => "unknown",
        }
    }
}

/// One entry of the mount table, as read from `/proc/mounts`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MountEntry {
    /// Device name, e.g. `/dev/nvme0n1p2` or `tmpfs`.
    pub device: String,
    /// Mount point, e.g. `/home`.
    pub mount_point: PathBuf,
    /// Filesystem type, e.g. `ext4`.
    pub file_system: String,
    /// Mount options, comma separated.
    pub options: String,
}

/// One mounted filesystem.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FilesystemInfo {
    /// Device name as reported by the mount table, e.g. `/dev/nvme0n1p2`.
    pub name: String,
    /// Mount point, e.g. `/home`.
    pub mount_point: PathBuf,
    /// Filesystem type, e.g. `ext4`.
    pub file_system: String,
    /// Classification derived from the device name and the type.
    pub kind: FilesystemKind,
    /// Total capacity in bytes.
    pub total: u64,
    /// Used space in bytes (`total - available`).
    pub used: u64,
    /// Space available to unprivileged users in bytes.
    pub available: u64,
    /// Whether the filesystem is mounted read-only.
    pub read_only: bool,
}

impl FilesystemInfo {
    /// Used percentage, 0.0 - 100.0.
    pub fn used_percent(&self) -> f64 {
        if self.total == 0 {
            0.0
        } else {
            (self.used as f64 / self.total as f64 * 100.0).clamp(0.0, 100.0)
        }
    }

    /// Whether the filesystem should be shown by default.
    pub fn is_interesting(&self) -> bool {
        self.kind != FilesystemKind::Pseudo
    }
}

/// Classifies a mount point from its device name and filesystem type.
pub fn classify(device: &str, file_system: &str) -> FilesystemKind {
    let fs = file_system.to_ascii_lowercase();
    if PSEUDO_FILESYSTEMS.contains(&fs.as_str()) {
        return FilesystemKind::Pseudo;
    }
    if NETWORK_FILESYSTEMS.contains(&fs.as_str()) {
        return FilesystemKind::Network;
    }
    if VIRTUAL_DEVICES
        .iter()
        .any(|prefix| device.starts_with(prefix))
    {
        return FilesystemKind::Virtual;
    }
    if device.starts_with("/dev/") {
        return FilesystemKind::Physical;
    }
    if fs == "fuseblk" || fs.starts_with("fuse.") {
        return FilesystemKind::Virtual;
    }
    FilesystemKind::Unknown
}

/// Parses the `/proc/mounts` format: `device mount_point type options dump pass`.
///
/// The mount point is octal-escaped by the kernel (spaces become `\040`), so it is
/// unescaped before use. Malformed lines are skipped rather than aborting the parse.
pub fn parse_proc_mounts(text: &str) -> Vec<MountEntry> {
    let mut entries = Vec::new();
    for line in text.lines() {
        let mut fields = line.split_whitespace();
        let (Some(device), Some(mount_point), Some(file_system), Some(options)) =
            (fields.next(), fields.next(), fields.next(), fields.next())
        else {
            continue;
        };
        entries.push(MountEntry {
            device: device.to_string(),
            mount_point: PathBuf::from(unescape_mount_path(mount_point)),
            file_system: file_system.to_string(),
            options: options.to_string(),
        });
    }
    entries
}

/// Undoes the octal escaping the kernel applies to mount paths.
fn unescape_mount_path(path: &str) -> String {
    if !path.contains('\\') {
        return path.to_string();
    }
    let bytes = path.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] == b'\\' && index + 3 < bytes.len() {
            let digits = &path[index + 1..index + 4];
            if let Ok(value) = u8::from_str_radix(digits, 8) {
                out.push(value);
                index += 4;
                continue;
            }
        }
        out.push(bytes[index]);
        index += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// Reads the capacity of a mount point with `statvfs(3)`.
///
/// Returns `(total, available)` in bytes. `available` is the space an unprivileged
/// process may still use, i.e. it excludes the root reserve.
pub fn capacity_of(mount_point: &Path) -> Option<(u64, u64)> {
    let stats = statvfs(mount_point).ok()?;
    let fragment_size = if stats.fragment_size() == 0 {
        stats.block_size()
    } else {
        stats.fragment_size()
    } as u64;
    // `fsblkcnt_t` is unsigned on Linux, so the counts can never be negative.
    let total = stats.blocks() as u64 * fragment_size;
    let available = stats.blocks_available() as u64 * fragment_size;
    Some((total, available))
}

/// Collects the list of mounted filesystems.
///
/// `/proc/mounts` plus `statvfs` is used rather than `sysinfo` because the mount table
/// is the only source that lists *every* mount, including the pseudo filesystems that
/// `sysinfo` filters out. Being able to show (and classify) those is what makes the
/// storage view honest.
pub struct StorageCollector {
    show_pseudo: bool,
    proc_mounts: PathBuf,
    seen: HashSet<PathBuf>,
}

impl Default for StorageCollector {
    fn default() -> Self {
        Self::new(false)
    }
}

impl StorageCollector {
    /// Creates a collector. `show_pseudo` keeps kernel pseudo filesystems in the list.
    pub fn new(show_pseudo: bool) -> Self {
        Self {
            show_pseudo,
            proc_mounts: PathBuf::from(PROC_MOUNTS),
            seen: HashSet::new(),
        }
    }

    /// Creates a collector reading a different mount table, which keeps tests hermetic.
    pub fn with_mounts(proc_mounts: impl Into<PathBuf>, show_pseudo: bool) -> Self {
        Self {
            show_pseudo,
            proc_mounts: proc_mounts.into(),
            seen: HashSet::new(),
        }
    }

    /// Returns a fresh list of mounted filesystems.
    ///
    /// The mount table is re-read on every call: mounts can appear or disappear at
    /// runtime (hotplug, external drives, container overlays).
    pub fn sample(&mut self) -> Vec<FilesystemInfo> {
        let Ok(text) = std::fs::read_to_string(&self.proc_mounts) else {
            tracing::debug!(path = %self.proc_mounts.display(), "cannot read mount table");
            return Vec::new();
        };
        self.from_mounts(&parse_proc_mounts(&text))
    }

    /// Converts mount table entries into snapshots, skipping duplicates and
    /// (optionally) pseudo filesystems.
    pub fn from_mounts(&mut self, entries: &[MountEntry]) -> Vec<FilesystemInfo> {
        let mut filesystems = Vec::new();
        let mut seen: HashSet<PathBuf> = HashSet::new();
        for entry in entries {
            let kind = classify(&entry.device, &entry.file_system);
            if !self.show_pseudo && kind == FilesystemKind::Pseudo {
                continue;
            }
            // The same mount point can appear several times (overmounts); the first
            // entry in the table is the one the user actually sees.
            if !seen.insert(entry.mount_point.clone()) {
                continue;
            }
            let (total, available) = capacity_of(&entry.mount_point).unwrap_or((0, 0));
            filesystems.push(FilesystemInfo {
                kind,
                name: entry.device.clone(),
                mount_point: entry.mount_point.clone(),
                file_system: entry.file_system.clone(),
                total,
                used: total.saturating_sub(available),
                available,
                read_only: is_read_only(entry),
            });
        }
        self.seen = seen;
        filesystems.sort_by(|a, b| {
            a.kind
                .sort_key()
                .cmp(&b.kind.sort_key())
                .then_with(|| a.mount_point.cmp(&b.mount_point))
        });
        filesystems
    }
}

/// Whether the mount options mark the filesystem as read-only.
///
/// A read-only mount is a reasonable proxy for "not a real, writable data disk" in
/// the storage table: it is shown, but flagged so the user is not alarmed by a
/// partition that can never fill up.
fn is_read_only(entry: &MountEntry) -> bool {
    entry.options.split(',').any(|option| option == "ro")
}

impl FilesystemKind {
    /// Ordering weight so physical disks are listed before the noise.
    fn sort_key(&self) -> u8 {
        match self {
            Self::Physical => 0,
            Self::Virtual => 1,
            Self::Network => 2,
            Self::Unknown => 3,
            Self::Pseudo => 4,
        }
    }
}

/// Returns the mount point of the filesystem containing `path`, if any.
///
/// The longest matching mount point wins, which is what the kernel itself does when
/// resolving a path.
pub fn mount_point_of(path: &Path) -> Option<PathBuf> {
    let text = std::fs::read_to_string(PROC_MOUNTS).ok()?;
    parse_proc_mounts(&text)
        .iter()
        .filter(|entry| path.starts_with(&entry.mount_point))
        .max_by_key(|entry| entry.mount_point.components().count())
        .map(|entry| entry.mount_point.clone())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classifies_pseudo_filesystems() {
        assert_eq!(classify("proc", "proc"), FilesystemKind::Pseudo);
        assert_eq!(classify("sysfs", "sysfs"), FilesystemKind::Pseudo);
        assert_eq!(classify("tmpfs", "tmpfs"), FilesystemKind::Pseudo);
        assert_eq!(classify("overlay", "overlay"), FilesystemKind::Pseudo);
    }

    #[test]
    fn classifies_network_filesystems() {
        assert_eq!(classify("/dev/sda1", "nfs4"), FilesystemKind::Network);
        assert_eq!(classify("//server/share", "cifs"), FilesystemKind::Network);
    }

    #[test]
    fn classifies_virtual_devices() {
        assert_eq!(classify("/dev/loop0", "ext4"), FilesystemKind::Virtual);
        assert_eq!(
            classify("/dev/mapper/cryptroot", "ext4"),
            FilesystemKind::Virtual
        );
        assert_eq!(classify("/dev/vda1", "xfs"), FilesystemKind::Virtual);
        assert_eq!(classify("archive", "fuseblk"), FilesystemKind::Virtual);
    }

    #[test]
    fn classifies_physical_devices() {
        assert_eq!(classify("/dev/nvme0n1p2", "ext4"), FilesystemKind::Physical);
        assert_eq!(classify("/dev/sda1", "ext4"), FilesystemKind::Physical);
    }

    #[test]
    fn classifies_unknown_mounts() {
        assert_eq!(
            classify("myserver:/export", "unknownfs"),
            FilesystemKind::Unknown
        );
    }

    #[test]
    fn used_percent_handles_zero_total() {
        let info = FilesystemInfo {
            name: "x".to_string(),
            mount_point: PathBuf::from("/"),
            file_system: "ext4".to_string(),
            kind: FilesystemKind::Physical,
            total: 0,
            used: 0,
            available: 0,
            read_only: false,
        };
        assert_eq!(info.used_percent(), 0.0);
        assert!(info.is_interesting());
    }

    #[test]
    fn kind_labels_are_stable() {
        assert_eq!(FilesystemKind::Physical.label(), "physical");
        assert_eq!(FilesystemKind::Network.label(), "network");
    }

    #[test]
    fn parses_proc_mounts_format() {
        let text = "/dev/nvme0n1p2 / xfs rw,relatime 0 0\nproc /proc proc rw,nosuid 0 0\n";
        let entries = parse_proc_mounts(text);
        assert_eq!(entries.len(), 2);
        assert_eq!(entries[0].device, "/dev/nvme0n1p2");
        assert_eq!(entries[0].mount_point, PathBuf::from("/"));
        assert_eq!(entries[0].file_system, "xfs");
        assert_eq!(entries[1].file_system, "proc");
    }

    #[test]
    fn unescapes_mount_paths() {
        let entries = parse_proc_mounts("/dev/sdb1 /mnt/my\\040disk ext4 rw 0 0\n");
        assert_eq!(entries[0].mount_point, PathBuf::from("/mnt/my disk"));
    }

    #[test]
    fn skips_malformed_mount_lines() {
        let entries = parse_proc_mounts("garbage\n/dev/sda1 / ext4 rw 0 0\nonly three\n");
        assert_eq!(entries.len(), 1);
    }

    #[test]
    fn pseudo_filesystems_are_filtered_by_default() {
        let mut collector = StorageCollector::new(false);
        let filesystems = collector.sample();
        assert!(
            !filesystems.is_empty(),
            "a running Linux system always has at least / mounted"
        );
        assert!(
            filesystems
                .iter()
                .any(|fs| fs.mount_point == Path::new("/"))
        );
        assert!(
            filesystems
                .iter()
                .all(|fs| fs.kind != FilesystemKind::Pseudo)
        );
    }

    #[test]
    fn pseudo_filesystems_can_be_included() {
        let mut collector = StorageCollector::new(true);
        let filesystems = collector.sample();
        assert!(
            filesystems
                .iter()
                .any(|fs| fs.kind == FilesystemKind::Pseudo),
            "expected a pseudo filesystem"
        );
    }

    #[test]
    fn duplicate_mount_points_are_collapsed() {
        let entries = parse_proc_mounts("/dev/sda1 /mnt ext4 rw 0 0\ntmpfs /mnt tmpfs rw 0 0\n");
        let mut collector = StorageCollector::new(true);
        let filesystems = collector.from_mounts(&entries);
        assert_eq!(filesystems.len(), 1);
        assert_eq!(filesystems[0].name, "/dev/sda1");
    }

    #[test]
    fn root_filesystem_reports_capacity() {
        let (total, available) = capacity_of(Path::new("/")).expect("statvfs on / must succeed");
        assert!(total > 0, "the root filesystem must report a capacity");
        assert!(available <= total);
    }

    #[test]
    fn mount_point_of_root_is_root() {
        assert_eq!(
            mount_point_of(Path::new("/")).as_deref(),
            Some(Path::new("/"))
        );
    }
}
