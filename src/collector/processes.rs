//! Process table collection, searching, sorting and signalling.
//!
//! CPU usage is reported by `sysinfo` as the share of one CPU spent in the process
//! during the last sampling interval, so a process pinned to two cores can reach
//! `200%`. Values are normalized against the number of logical cores when the user
//! asks for a "total"-style percentage, which is why the table can show either
//! notation.
//!
//! Processes can disappear between two samples; the collector never assumes that a
//! previously known PID is still alive, and every operation that touches a PID can
//! fail with [`KillError`] instead of panicking.

use std::collections::HashMap;
use std::fmt;
use std::path::PathBuf;

use sysinfo::{Pid, ProcessRefreshKind, ProcessesToUpdate, System, UpdateKind, Users};

/// Maximum number of entries kept in the cached user name table.
const MAX_USERS: usize = 4096;

/// A single row of the process table.
#[derive(Debug, Clone, PartialEq)]
pub struct ProcessInfo {
    /// Process id.
    pub pid: Pid,
    /// Executable name, truncated by the kernel to 15 characters for comm.
    pub name: String,
    /// Full command line, when the process has one (kthreads have none).
    pub cmd: Option<String>,
    /// Absolute path of the executable, when the caller may read it.
    pub exe: Option<PathBuf>,
    /// Name of the owning user, or the numeric uid when it cannot be resolved.
    pub user: String,
    /// Raw numeric uid.
    pub uid: Option<u32>,
    /// Single letter process state from `/proc/<pid>/stat`, e.g. `R` or `S`.
    pub state: String,
    /// CPU usage in percent; may exceed 100 for multi threaded processes.
    pub cpu_usage: f32,
    /// Resident set size in bytes.
    pub memory: u64,
    /// Parent process id.
    pub parent: Option<Pid>,
    /// Number of threads, when the kernel exposes it.
    pub threads: Option<usize>,
}

impl ProcessInfo {
    /// Memory usage in bytes as reported by the kernel.
    pub fn memory_bytes(&self) -> u64 {
        self.memory
    }
}

/// The collected process table plus bookkeeping for disappearing processes.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct ProcessList {
    /// All processes found in the last sample, unsorted.
    pub processes: Vec<ProcessInfo>,
    /// Number of processes that vanished between the previous and the current sample.
    pub vanished: usize,
}

impl ProcessList {
    /// Looks a process up by pid.
    pub fn find(&self, pid: Pid) -> Option<&ProcessInfo> {
        self.processes.iter().find(|process| process.pid == pid)
    }
}

/// Column the process table is sorted by.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum SortKey {
    /// CPU usage.
    #[default]
    Cpu,
    /// Resident memory.
    Memory,
    /// Process id.
    Pid,
    /// Process name, alphabetically.
    Name,
}

impl SortKey {
    /// Short label used in the status bar, e.g. `CPU%`.
    pub fn label(&self) -> &'static str {
        match self {
            Self::Cpu => "CPU%",
            Self::Memory => "MEM",
            Self::Pid => "PID",
            Self::Name => "NAME",
        }
    }
}

/// Which part of a process the search query is matched against.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum SearchField {
    /// Match the process name (default).
    #[default]
    Name,
    /// Match the numeric pid.
    Pid,
    /// Match name, command line and executable path.
    All,
}

/// Which processes the table shows.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ProcessFilter {
    /// Everything, including kernel threads.
    #[default]
    All,
    /// Only processes owned by the current user.
    User,
    /// Only processes with a non-empty command line (excludes most kthreads).
    UserTasks,
}

/// A search request: free text plus the field it applies to.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SearchQuery {
    /// Raw text typed by the user.
    pub text: String,
    /// Field the text is matched against.
    pub field: SearchField,
}

impl SearchQuery {
    /// Whether the query is empty and therefore matches everything.
    pub fn is_empty(&self) -> bool {
        self.text.trim().is_empty()
    }
}

/// Returns whether `process` matches `query` (case insensitive).
pub fn matches(process: &ProcessInfo, query: &SearchQuery) -> bool {
    if query.is_empty() {
        return true;
    }
    let needle = query.text.trim().to_lowercase();
    if needle.is_empty() {
        return true;
    }
    match query.field {
        SearchField::Name => process.name.to_lowercase().contains(&needle),
        SearchField::Pid => {
            let numeric: u32 = match needle.parse() {
                Ok(value) => value,
                Err(_) => return false,
            };
            process.pid.as_u32() == numeric
        }
        SearchField::All => {
            process.name.to_lowercase().contains(&needle)
                || process
                    .cmd
                    .as_deref()
                    .is_some_and(|cmd| cmd.to_lowercase().contains(&needle))
                || process
                    .exe
                    .as_deref()
                    .is_some_and(|exe| exe.to_string_lossy().to_lowercase().contains(&needle))
                || process.pid.as_u32().to_string() == needle
        }
    }
}

/// Filters and sorts a process list in place.
///
/// Sorting is stable with respect to the secondary key (pid) so that rows do not
/// jump around between refreshes when the primary value is identical.
pub fn sort_processes(processes: &mut [ProcessInfo], key: SortKey, descending: bool) {
    processes.sort_by(|a, b| {
        let ordering = match key {
            SortKey::Cpu => a.cpu_usage.total_cmp(&b.cpu_usage),
            SortKey::Memory => a.memory.cmp(&b.memory),
            SortKey::Pid => a.pid.as_u32().cmp(&b.pid.as_u32()),
            SortKey::Name => a.name.to_lowercase().cmp(&b.name.to_lowercase()),
        };
        let ordering = if descending {
            ordering.reverse()
        } else {
            ordering
        };
        ordering.then_with(|| a.pid.as_u32().cmp(&b.pid.as_u32()))
    });
}

/// Applies a search query to a slice of processes.
pub fn filter_processes<'a>(
    processes: &'a [ProcessInfo],
    query: &SearchQuery,
) -> Vec<&'a ProcessInfo> {
    processes
        .iter()
        .filter(|process| matches(process, query))
        .collect()
}

/// The signal sent to a process when the user terminates it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TerminateSignal {
    /// `SIGTERM` – ask the process to shut down cleanly.
    Term,
    /// `SIGKILL` – terminate immediately, no cleanup possible.
    Kill,
}

impl fmt::Display for TerminateSignal {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.label())
    }
}

impl TerminateSignal {
    /// Short label shown in the confirmation dialog.
    pub fn label(&self) -> &'static str {
        match self {
            Self::Term => "SIGTERM",
            Self::Kill => "SIGKILL",
        }
    }

    /// The `nix` signal number.
    fn as_nix(self) -> nix::sys::signal::Signal {
        match self {
            Self::Term => nix::sys::signal::Signal::SIGTERM,
            Self::Kill => nix::sys::signal::Signal::SIGKILL,
        }
    }
}

/// Failure while signalling a process.
#[derive(Debug, thiserror::Error)]
pub enum KillError {
    /// The kernel refused the signal, most likely because of permissions.
    #[error("not permitted to send {signal} to process {pid} ({name})")]
    PermissionDenied {
        /// Target process id.
        pid: u32,
        /// Target process name.
        name: String,
        /// Signal that was requested.
        signal: &'static str,
    },
    /// The process exited between listing it and signalling it.
    #[error("process {pid} ({name}) is no longer running")]
    NoSuchProcess {
        /// Target process id.
        pid: u32,
        /// Target process name.
        name: String,
    },
    /// The syscall failed for another reason.
    #[error("failed to send {signal} to process {pid} ({name}): {source}")]
    Other {
        /// Target process id.
        pid: u32,
        /// Target process name.
        name: String,
        /// Signal that was requested.
        signal: &'static str,
        /// Underlying `errno`.
        #[source]
        source: nix::errno::Errno,
    },
}

/// Sends `signal` to `pid`.
///
/// This never escalates privileges and never retries: a failure is reported to the
/// user so they can decide what to do. `syswatch` is not a process supervisor and
/// must never be able to kill something the user did not explicitly confirm.
pub fn terminate(pid: Pid, name: &str, signal: TerminateSignal) -> Result<(), KillError> {
    let nix_pid = nix::unistd::Pid::from_raw(pid.as_u32() as i32);
    match nix::sys::signal::kill(nix_pid, signal.as_nix()) {
        Ok(()) => Ok(()),
        Err(nix::errno::Errno::EPERM) => Err(KillError::PermissionDenied {
            pid: pid.as_u32(),
            name: name.to_string(),
            signal: signal.label(),
        }),
        Err(nix::errno::Errno::ESRCH) => Err(KillError::NoSuchProcess {
            pid: pid.as_u32(),
            name: name.to_string(),
        }),
        Err(source) => Err(KillError::Other {
            pid: pid.as_u32(),
            name: name.to_string(),
            signal: signal.label(),
            source,
        }),
    }
}

/// Collects the process table.
pub struct ProcessCollector {
    refresh_kind: ProcessRefreshKind,
    users: Users,
    user_names: HashMap<u32, String>,
    current_uid: u32,
    last_pids: Vec<Pid>,
}

impl ProcessCollector {
    /// Creates a collector, priming `system` so the first CPU values are meaningful.
    pub fn new(_config: &crate::collector::CollectorConfig) -> Self {
        // `without_tasks()` matters a lot: `sysinfo` treats every *thread* as a
        // process by default, so the default walks `/proc/<pid>/task/*/stat` for the
        // whole system. On a busy host that is several times more work than listing
        // the processes themselves, and the process table has no use for the extra
        // rows.
        let refresh_kind = ProcessRefreshKind::nothing()
            .with_cpu()
            .with_memory()
            .with_exe(UpdateKind::OnlyIfNotSet)
            .with_cmd(UpdateKind::OnlyIfNotSet)
            .with_user(UpdateKind::OnlyIfNotSet)
            .without_tasks();
        let users = Users::new_with_refreshed_list();
        // The uid syswatch runs as; used by the "my processes" filter.
        let current_uid = u32::from(nix::unistd::Uid::current());
        let mut collector = Self {
            refresh_kind,
            users,
            user_names: HashMap::new(),
            current_uid,
            last_pids: Vec::new(),
        };
        // Populate the name map immediately so the first sample resolves users.
        collector.refresh_user_names(true);
        collector
    }

    /// The uid syswatch itself runs as.
    pub fn current_uid(&self) -> u32 {
        self.current_uid
    }

    /// Refreshes and returns the process table.
    ///
    /// `refresh_users` is false on most ticks: the uid-to-name map is derived from
    /// `/etc/passwd`, which does not change while syswatch is running, and re-reading
    /// it every second is pure overhead. The caller passes true on the slow cadence.
    pub fn sample(&mut self, system: &mut System, refresh_users: bool) -> ProcessList {
        system.refresh_processes_specifics(ProcessesToUpdate::All, true, self.refresh_kind);

        let current: Vec<Pid> = system.processes().keys().copied().collect();
        let vanished = self
            .last_pids
            .iter()
            .filter(|pid| !current.contains(pid))
            .count();
        self.last_pids = current;

        self.refresh_user_names(refresh_users);
        let mut processes = Vec::with_capacity(system.processes().len());
        for (pid, process) in system.processes() {
            let uid = process.user_id().map(numeric_uid);
            processes.push(ProcessInfo {
                pid: *pid,
                name: process.name().to_string_lossy().into_owned(),
                cmd: process
                    .cmd()
                    .iter()
                    .map(|arg| arg.to_string_lossy().into_owned())
                    .collect::<Vec<_>>()
                    .join(" ")
                    .into(),
                exe: process.exe().map(PathBuf::from),
                user: uid
                    .and_then(|uid| self.user_names.get(&uid).cloned())
                    .or_else(|| uid.map(|uid| uid.to_string()))
                    .unwrap_or_else(|| "?".to_string()),
                uid,
                state: process.status().to_string(),
                cpu_usage: process.cpu_usage(),
                memory: process.memory(),
                parent: process.parent(),
                threads: None,
            });
        }
        ProcessList {
            processes,
            vanished,
        }
    }

    /// Refreshes the uid to name mapping, keeping the cache bounded.
    ///
    /// Skipped entirely on fast ticks; the map is filled once at construction so the
    /// very first sample already resolves names.
    fn refresh_user_names(&mut self, refresh: bool) {
        if !refresh && !self.user_names.is_empty() {
            return;
        }
        self.users.refresh();
        if self.user_names.len() <= MAX_USERS {
            for user in self.users.list() {
                self.user_names
                    .insert(numeric_uid(user.id()), user.name().to_string());
            }
        }
    }
}

/// Returns the numeric value of a `sysinfo` user id.
///
/// `sysinfo::Uid` derefs to the platform's `uid_t`, which is a `u32` on Linux. On
/// other platforms syswatch simply reports `0` and falls back to the user name.
fn numeric_uid(uid: &sysinfo::Uid) -> u32 {
    #[cfg(unix)]
    {
        **uid
    }
    #[cfg(not(unix))]
    {
        let _ = uid;
        0
    }
}

impl fmt::Display for SortKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.label())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn process(pid: u32, name: &str, cpu: f32, memory: u64) -> ProcessInfo {
        ProcessInfo {
            pid: Pid::from_u32(pid),
            name: name.to_string(),
            cmd: None,
            exe: None,
            user: "root".to_string(),
            uid: Some(0),
            state: "S".to_string(),
            cpu_usage: cpu,
            memory,
            parent: None,
            threads: None,
        }
    }

    #[test]
    fn search_matches_name_case_insensitively() {
        let procs = vec![process(1, "systemd", 0.0, 0), process(2, "bash", 0.0, 0)];
        let query = SearchQuery {
            text: "SYS".to_string(),
            field: SearchField::Name,
        };
        assert_eq!(filter_processes(&procs, &query).len(), 1);
    }

    #[test]
    fn search_by_pid_is_numeric_only() {
        let procs = vec![process(42, "systemd", 0.0, 0), process(7, "bash", 0.0, 0)];
        let query = SearchQuery {
            text: "42".to_string(),
            field: SearchField::Pid,
        };
        let found = filter_processes(&procs, &query);
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].pid.as_u32(), 42);

        let bad = SearchQuery {
            text: "bash".to_string(),
            field: SearchField::Pid,
        };
        assert!(filter_processes(&procs, &bad).is_empty());
    }

    #[test]
    fn search_by_all_covers_cmdline_and_exe() {
        let mut proc = process(9, "sh", 0.0, 0);
        proc.cmd = Some("/bin/sh -c ls".to_string());
        proc.exe = Some(PathBuf::from("/usr/bin/bash"));
        let procs = vec![proc];
        for needle in ["sh", "ls", "usr/bin"] {
            let query = SearchQuery {
                text: needle.to_string(),
                field: SearchField::All,
            };
            assert_eq!(
                filter_processes(&procs, &query).len(),
                1,
                "expected {needle} to match"
            );
        }
    }

    #[test]
    fn empty_query_matches_everything() {
        let procs = vec![process(1, "a", 0.0, 0), process(2, "b", 0.0, 0)];
        assert_eq!(filter_processes(&procs, &SearchQuery::default()).len(), 2);
    }

    #[test]
    fn sorts_by_cpu_descending() {
        let mut procs = vec![
            process(1, "a", 1.0, 100),
            process(2, "b", 50.0, 10),
            process(3, "c", 25.0, 50),
        ];
        sort_processes(&mut procs, SortKey::Cpu, true);
        assert_eq!(
            procs.iter().map(|p| p.pid.as_u32()).collect::<Vec<_>>(),
            vec![2, 3, 1]
        );
    }

    #[test]
    fn sorts_by_memory_ascending() {
        let mut procs = vec![
            process(1, "a", 0.0, 300),
            process(2, "b", 0.0, 100),
            process(3, "c", 0.0, 200),
        ];
        sort_processes(&mut procs, SortKey::Memory, false);
        assert_eq!(
            procs.iter().map(|p| p.memory).collect::<Vec<_>>(),
            vec![100, 200, 300]
        );
    }

    #[test]
    fn sorts_by_pid_ascending() {
        let mut procs = vec![
            process(30, "a", 0.0, 0),
            process(10, "b", 0.0, 0),
            process(20, "c", 0.0, 0),
        ];
        sort_processes(&mut procs, SortKey::Pid, false);
        assert_eq!(
            procs.iter().map(|p| p.pid.as_u32()).collect::<Vec<_>>(),
            vec![10, 20, 30]
        );
    }

    #[test]
    fn equal_keys_fall_back_to_pid_for_stability() {
        let mut procs = vec![process(5, "a", 10.0, 10), process(3, "b", 10.0, 10)];
        sort_processes(&mut procs, SortKey::Cpu, true);
        assert_eq!(
            procs.iter().map(|p| p.pid.as_u32()).collect::<Vec<_>>(),
            vec![3, 5]
        );
    }

    #[test]
    fn collects_the_running_processes() {
        let config = crate::collector::CollectorConfig::default();
        let mut collector = ProcessCollector::new(&config);
        let mut system = System::new();
        let list = collector.sample(&mut system, true);
        assert!(
            list.processes
                .iter()
                .any(|process| process.pid == Pid::from_u32(std::process::id()))
        );
    }

    #[test]
    fn detects_processes_that_vanished() {
        let config = crate::collector::CollectorConfig::default();
        let mut collector = ProcessCollector::new(&config);
        let mut system = System::new();
        let first = collector.sample(&mut system, true);
        assert_eq!(first.vanished, 0);
        let second = collector.sample(&mut system, true);
        assert!(second.vanished <= first.processes.len());
    }

    #[test]
    fn user_names_resolve_on_the_first_sample_without_a_user_refresh() {
        // The name map is populated at construction, so a fast tick that skips the
        // passwd re-read still reports real user names rather than bare uids.
        let config = crate::collector::CollectorConfig::default();
        let mut collector = ProcessCollector::new(&config);
        let mut system = System::new();
        let list = collector.sample(&mut system, false);
        let me = list
            .processes
            .iter()
            .find(|p| p.pid == Pid::from_u32(std::process::id()));
        if let Some(me) = me {
            assert_ne!(
                me.user, "?",
                "the current process must resolve to a user name"
            );
            assert!(!me.user.is_empty());
        }
    }

    #[test]
    fn terminate_reports_missing_process() {
        // A child that has already been reaped by `wait()` is a reliable ESRCH
        // source; signalling pid 0 would hit the whole process group instead.
        let mut child = std::process::Command::new("/bin/sh")
            .args(["-c", "exit 0"])
            .spawn()
            .expect("spawn child");
        child.wait().expect("wait for child");
        let pid = Pid::from_u32(child.id());

        let err =
            terminate(pid, "exited-child", TerminateSignal::Term).expect_err("expected failure");
        assert!(
            matches!(err, KillError::NoSuchProcess { .. }),
            "unexpected error: {err}"
        );
    }

    #[test]
    fn terminate_reports_permission_errors_for_foreign_processes() {
        // PID 1 is owned by the init process; signalling it as an unprivileged user
        // must be refused rather than silently succeeding.
        match terminate(Pid::from_u32(1), "init", TerminateSignal::Term) {
            Err(KillError::PermissionDenied { .. }) | Err(KillError::NoSuchProcess { .. }) => {}
            Err(other) => panic!("unexpected error: {other}"),
            Ok(()) => panic!("syswatch must never be able to signal pid 1"),
        }
    }

    #[test]
    fn signal_labels_are_stable() {
        assert_eq!(TerminateSignal::Term.label(), "SIGTERM");
        assert_eq!(TerminateSignal::Kill.label(), "SIGKILL");
    }
}
