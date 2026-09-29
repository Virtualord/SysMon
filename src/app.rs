//! Application state: the single source of truth the UI renders.
//!
//! [`App`] owns the latest [`Snapshot`], the graph history, the process table
//! projection (search + sort), the dialog state and the configuration. It is mutated
//! only on the main thread, so no locking is required.

use std::time::{Duration, SystemTime};

use crate::collector::Snapshot;
use crate::collector::processes::{self, ProcessInfo, SearchQuery, SortKey};
use crate::config::Config;
use crate::history::Series;

/// The dashboards syswatch can show.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum View {
    /// CPU utilization, per-core detail and history.
    #[default]
    Cpu,
    /// Memory and swap.
    Memory,
    /// Sortable, searchable process table.
    Processes,
    /// Mounted filesystems.
    Storage,
    /// Network interfaces and throughput.
    Network,
    /// Static system information.
    System,
}

impl View {
    /// Every view, in tab order.
    pub const ALL: [View; 6] = [
        View::Cpu,
        View::Memory,
        View::Processes,
        View::Storage,
        View::Network,
        View::System,
    ];

    /// Position of the view in the tab bar, starting at 1.
    pub fn index(&self) -> usize {
        View::ALL.iter().position(|view| view == self).unwrap_or(0)
    }

    /// Title shown in the tab bar and the status bar.
    pub fn title(&self) -> &'static str {
        match self {
            Self::Cpu => "CPU",
            Self::Memory => "Memory",
            Self::Processes => "Processes",
            Self::Storage => "Storage",
            Self::Network => "Network",
            Self::System => "System",
        }
    }

    /// The view selected by the number keys 1-6.
    ///
    /// `0` maps to the first view so that a clamped key press still lands somewhere
    /// sensible; anything beyond `6` returns `None`.
    pub fn from_index(index: usize) -> Option<Self> {
        match index {
            0 => Some(View::Cpu),
            other => View::ALL.get(other - 1).copied(),
        }
    }
}

/// Which pane has keyboard focus.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Focus {
    /// The main panel of the current view.
    #[default]
    Main,
    /// A process row inside the process table.
    ProcessRow,
    /// The storage table.
    StorageRow,
    /// The network table.
    NetworkRow,
}

/// Which modal is on top of the dashboard.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Dialog {
    /// Keyboard shortcut reference.
    Help,
    /// Full details of the selected process.
    ProcessDetails,
    /// Confirmation before sending a signal to a process.
    ConfirmKill {
        /// Target process id.
        pid: u32,
        /// Target process name.
        name: String,
        /// Signal that will be sent.
        signal: processes::TerminateSignal,
    },
}

impl Dialog {
    /// Short title used for the dialog border.
    pub fn title(&self) -> String {
        match self {
            Self::Help => "Keyboard shortcuts".to_string(),
            Self::ProcessDetails => "Process details".to_string(),
            Self::ConfirmKill { signal, .. } => {
                format!("Confirm termination — {label}", label = signal.label())
            }
        }
    }
}

/// A transient message shown in the status bar.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StatusMessage {
    /// Text to display.
    pub text: String,
    /// Whether the message represents a failure.
    pub is_error: bool,
    /// When the message was created, used to expire it.
    pub created: SystemTime,
    /// How long the message stays visible.
    pub ttl: Duration,
}

impl StatusMessage {
    /// Creates an informational message that lives for five seconds.
    pub fn info(text: impl Into<String>) -> Self {
        Self {
            text: text.into(),
            is_error: false,
            created: SystemTime::now(),
            ttl: Duration::from_secs(5),
        }
    }

    /// Creates an error message that lives for ten seconds.
    pub fn error(text: impl Into<String>) -> Self {
        Self {
            text: text.into(),
            is_error: true,
            created: SystemTime::now(),
            ttl: Duration::from_secs(10),
        }
    }

    /// Whether the message has outlived its time to live.
    pub fn is_expired(&self) -> bool {
        self.created
            .elapsed()
            .map(|age| age >= self.ttl)
            .unwrap_or(false)
    }
}

/// What the user asked the application to do.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Action {
    /// Leave the application.
    Quit,
    /// Redraw the current frame.
    Redraw,
    /// Ask the monitoring thread for an immediate sample.
    Refresh,
    /// Switch to the next view.
    NextView,
    /// Switch to the previous view.
    PreviousView,
    /// Activate a view by tab index (1-6).
    SelectView(usize),
    /// Open the help overlay.
    ToggleHelp,
    /// Close the topmost dialog.
    CloseDialog,
    /// Begin searching processes.
    StartSearch,
    /// Confirm the pending search input.
    CommitSearch,
    /// Delete the last character of the search input.
    SearchBackspace,
    /// Move the selection up.
    SelectPrevious,
    /// Move the selection down.
    SelectNext,
    /// Page the selection up.
    PageUp,
    /// Page the selection down.
    PageDown,
    /// Jump to the first row.
    SelectFirst,
    /// Jump to the last row.
    SelectLast,
    /// Open the details of the selected row.
    OpenDetails,
    /// Open the termination dialog for the selected process.
    StartKill,
    /// Toggle the signal used by the termination dialog between TERM and KILL.
    ToggleKillSignal,
    /// Execute the confirmed termination.
    ConfirmKill,
    /// Sort the process table by CPU usage.
    SortByCpu,
    /// Sort the process table by memory.
    SortByMemory,
    /// Sort the process table by pid.
    SortByPid,
    /// Sort the process table by name.
    SortByName,
    /// Reverse the sort order.
    ToggleSortDirection,
    /// Cycle which process filter is active.
    CycleFilter,
    /// Cycle which field the search matches against.
    CycleSearchField,
    /// Toggle between Unicode block and ASCII graph characters.
    ToggleGraphStyle,
    /// Move focus to the next focusable pane.
    FocusNext,
    /// Move focus to the previous focusable pane.
    FocusPrevious,
    /// Apply a literal character to the search input.
    InsertChar(char),
    /// Ignore this key.
    None,
}

/// Central application state.
pub struct App {
    /// The active configuration.
    pub config: Config,
    /// The most recent snapshot received from the monitoring thread.
    pub snapshot: Option<Snapshot>,
    /// The dashboard currently displayed.
    pub view: View,
    /// Which pane has keyboard focus.
    pub focus: Focus,
    /// The modal currently on top, if any.
    pub dialog: Option<Dialog>,
    /// Whether the search prompt is currently accepting input.
    pub search_active: bool,
    /// The process search query.
    pub search: SearchQuery,
    /// Which process filter is applied.
    pub filter: processes::ProcessFilter,
    /// The column the process table is sorted by.
    pub sort_key: SortKey,
    /// Whether the sort order is descending.
    pub sort_descending: bool,
    /// Index of the selected row within the filtered process list.
    pub selected_process: usize,
    /// Number of visible rows in the process table, used for paging.
    pub visible_rows: usize,
    /// CPU utilization history (0-100).
    pub cpu_history: Series,
    /// Per core utilization history, indexed like `cpu_history`.
    pub core_history: Vec<Series>,
    /// Memory utilization history (0-100).
    pub memory_history: Series,
    /// Network receive throughput history (bytes per second).
    pub network_rx_history: Series,
    /// Network transmit throughput history (bytes per second).
    pub network_tx_history: Series,
    /// Transient message shown in the status bar.
    pub message: Option<StatusMessage>,
    /// Whether the application should keep running.
    pub should_quit: bool,
    /// Whether the last requested refresh has been served.
    pub pending_refresh: bool,
    /// Whether colours are enabled (`--no-color` turns this off).
    pub color_enabled: bool,
}

impl App {
    /// Creates the application state from a configuration.
    pub fn new(config: Config) -> Self {
        let history_length = config.graph_history;
        let sort = config.process_sort;
        Self {
            view: config.default_view.into(),
            sort_key: sort.key.into(),
            sort_descending: sort.descending,
            cpu_history: Series::new(history_length),
            core_history: Vec::new(),
            memory_history: Series::new(history_length),
            network_rx_history: Series::new(history_length),
            network_tx_history: Series::new(history_length),
            config,
            snapshot: None,
            focus: Focus::Main,
            dialog: None,
            search_active: false,
            search: SearchQuery::default(),
            filter: processes::ProcessFilter::All,
            selected_process: 0,
            visible_rows: 1,
            message: None,
            should_quit: false,
            pending_refresh: false,
            color_enabled: true,
        }
    }

    /// Applies a new configuration, resizing the history buffers if needed.
    pub fn set_config(&mut self, config: Config) {
        let history_length = config.graph_history;
        self.cpu_history.set_capacity(history_length);
        self.memory_history.set_capacity(history_length);
        self.network_rx_history.set_capacity(history_length);
        self.network_tx_history.set_capacity(history_length);
        self.config = config;
    }

    /// Merges a fresh snapshot into the state and appends to the history buffers.
    ///
    /// The snapshot is stored *before* the selection is clamped: clamping against the
    /// previous list would leave the cursor on a row that no longer exists.
    pub fn update(&mut self, snapshot: Snapshot) {
        let logical_cores = snapshot.cpu.cores.len().max(1);
        if self.core_history.len() != logical_cores {
            self.core_history = (0..logical_cores)
                .map(|_| Series::new(self.config.graph_history))
                .collect();
        }
        self.cpu_history.push(f64::from(snapshot.cpu.usage));
        for (series, core) in self.core_history.iter_mut().zip(snapshot.cpu.cores.iter()) {
            series.push(f64::from(core.usage));
        }
        self.memory_history.push(snapshot.memory.used_percent());
        self.network_rx_history.push(snapshot.network.receive_rate);
        self.network_tx_history.push(snapshot.network.transmit_rate);

        self.snapshot = Some(snapshot);
        self.visible_rows = 1;
        self.clamp_selection();
    }

    /// The process list of the latest snapshot, or an empty slice.
    pub fn process_list(&self) -> &[ProcessInfo] {
        self.snapshot
            .as_ref()
            .map_or(&[], |snapshot| snapshot.processes.processes.as_slice())
    }

    /// Number of processes that died since the previous refresh.
    pub fn vanished_processes(&self) -> usize {
        self.snapshot
            .as_ref()
            .map_or(0, |snapshot| snapshot.processes.vanished)
    }

    /// The processes that pass the active search and filter, sorted for display.
    ///
    /// The rows are cloned out of the snapshot so the result is an owned `Vec` that a
    /// caller may keep while it mutates `self` (moving the selection, taking a fresh
    /// snapshot). A monitor shows at most a few thousand processes, so the copy is far
    /// cheaper than the borrow gymnastics of handing out references.
    pub fn visible_processes(&self) -> Vec<ProcessInfo> {
        let current_uid = u32::from(nix::unistd::Uid::current());
        let mut visible: Vec<ProcessInfo> = self
            .process_list()
            .iter()
            .filter(|process| match self.filter {
                processes::ProcessFilter::All => true,
                processes::ProcessFilter::User => process.uid == Some(current_uid),
                processes::ProcessFilter::UserTasks => {
                    process.cmd.as_deref().is_some_and(|cmd| !cmd.is_empty())
                }
            })
            .filter(|process| processes::matches(process, &self.search))
            .cloned()
            .collect();
        processes::sort_processes(&mut visible, self.sort_key, self.sort_descending);
        visible
    }

    /// The process currently selected in the table, if any.
    pub fn selected_process_info(&self) -> Option<ProcessInfo> {
        self.visible_processes()
            .into_iter()
            .nth(self.selected_process)
    }

    /// Keeps the selection inside the bounds of the current list.
    pub fn clamp_selection(&mut self) {
        let len = self.visible_processes().len();
        if len == 0 {
            self.selected_process = 0;
        } else if self.selected_process >= len {
            self.selected_process = len - 1;
        }
    }

    /// Moves the selection by `delta`, clamped to the list bounds.
    pub fn move_selection(&mut self, delta: isize) {
        let len = self.visible_processes().len();
        if len == 0 {
            self.selected_process = 0;
            return;
        }
        let current = self.selected_process as isize;
        let next = (current + delta).clamp(0, len as isize - 1);
        self.selected_process = next as usize;
    }

    /// Number of rows in the current process list.
    pub fn process_rows(&self) -> usize {
        self.visible_processes().len()
    }

    /// Total number of processes, before filtering.
    pub fn total_processes(&self) -> usize {
        self.process_list().len()
    }

    /// Sets the message shown in the status bar.
    pub fn notify(&mut self, message: StatusMessage) {
        self.message = Some(message);
    }

    /// Clears an expired message. Called once per frame.
    pub fn expire_message(&mut self) {
        if self.message.as_ref().is_some_and(StatusMessage::is_expired) {
            self.message = None;
        }
    }

    /// Switches to the next view, wrapping around.
    pub fn next_view(&mut self) {
        self.view = View::ALL[(self.view.index() + 1) % View::ALL.len()];
    }

    /// Switches to the previous view, wrapping around.
    pub fn previous_view(&mut self) {
        let index = self.view.index();
        self.view = View::ALL[(index + View::ALL.len() - 1) % View::ALL.len()];
    }

    /// Applies a sort change.
    ///
    /// Pressing the key of the active column reverses the order, which is what a user
    /// expects from a table header. Pressing a *different* column switches to it with
    /// the natural order: usage columns descending, identifiers ascending.
    pub fn set_sort(&mut self, key: SortKey) {
        if self.sort_key == key {
            self.sort_descending = !self.sort_descending;
        } else {
            self.sort_key = key;
            self.sort_descending = matches!(key, SortKey::Cpu | SortKey::Memory | SortKey::Name);
        }
        self.selected_process = 0;
    }

    /// Clears the search query and deactivates the prompt.
    pub fn clear_search(&mut self) {
        self.search = SearchQuery::default();
        self.search_active = false;
        self.selected_process = 0;
    }

    /// Handles a search prompt keystroke. Returns whether the key was consumed.
    pub fn handle_search_key(&mut self, key: crate::events::Key) -> bool {
        use crate::events::Key;
        match key {
            Key::Esc => {
                self.clear_search();
                true
            }
            Key::Enter => {
                self.search_active = false;
                self.selected_process = 0;
                true
            }
            Key::Backspace => {
                self.search.text.pop();
                self.selected_process = 0;
                true
            }
            Key::Tab => {
                self.search_active = false;
                false
            }
            Key::Char(ch) => {
                self.search.text.push(ch);
                self.selected_process = 0;
                true
            }
            _ => false,
        }
    }

    /// Whether printable characters should be routed to the search prompt.
    pub fn search_owns_input(&self) -> bool {
        self.search_active
    }

    /// Opens the search prompt with an empty query.
    pub fn start_search(&mut self) {
        self.search_active = true;
        self.search.text.clear();
        self.search.field = processes::SearchField::Name;
        self.selected_process = 0;
    }

    /// Runs the confirmed termination of a process.
    ///
    /// This is the only place in the program that sends a signal, and it is only
    /// reachable after the user explicitly confirmed the [`Dialog::ConfirmKill`] dialog.
    pub fn confirm_kill(&mut self) {
        let Some(Dialog::ConfirmKill { pid, name, signal }) = self.dialog.clone() else {
            return;
        };
        let pid = sysinfo::Pid::from_u32(pid);
        match processes::terminate(pid, &name, signal) {
            Ok(()) => self.notify(StatusMessage::info(format!(
                "sent {} to {} ({})",
                signal.label(),
                pid.as_u32(),
                name
            ))),
            Err(err) => self.notify(StatusMessage::error(err.to_string())),
        }
        self.dialog = None;
        self.pending_refresh = true;
    }
}

/// Convenience accessor used by the UI.
impl App {
    /// The latest snapshot, if one has arrived.
    pub fn snapshot(&self) -> Option<&Snapshot> {
        self.snapshot.as_ref()
    }

    /// Whether a modal is open.
    pub fn has_dialog(&self) -> bool {
        self.dialog.is_some()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::collector::memory::MemorySnapshot;
    use crate::collector::processes::ProcessList;
    use crate::history::Series;

    fn snapshot_with_processes(count: u32) -> Snapshot {
        let processes: Vec<ProcessInfo> = (0..count)
            .map(|index| ProcessInfo {
                pid: sysinfo::Pid::from_u32(index + 1),
                name: format!("proc{index}"),
                cmd: Some(format!("/bin/proc{index}")),
                exe: Some(std::path::PathBuf::from(format!("/bin/proc{index}"))),
                user: "user".to_string(),
                uid: Some(1000),
                state: "S".to_string(),
                cpu_usage: (count - index) as f32,
                memory: u64::from(count - index) * 1024,
                parent: None,
                threads: Some(1),
            })
            .collect();
        Snapshot {
            sequence: 1,
            interval: Duration::from_secs(1),
            timestamp_millis: 0,
            cpu: Default::default(),
            memory: MemorySnapshot {
                total: 1000,
                used: 500,
                ..MemorySnapshot::default()
            },
            processes: ProcessList {
                processes,
                vanished: 0,
            },
            storage: Vec::new(),
            network: Default::default(),
            system: Default::default(),
            dynamic: Default::default(),
            warnings: Vec::new(),
        }
    }

    fn app_with_processes(count: u32) -> App {
        let mut app = App::new(Config::default());
        app.update(snapshot_with_processes(count));
        app
    }

    #[test]
    fn view_indices_round_trip() {
        for (index, view) in View::ALL.iter().enumerate() {
            assert_eq!(view.index(), index);
            assert_eq!(View::from_index(index + 1), Some(*view));
        }
        // Out of range values must not wrap around into a valid view.
        assert_eq!(View::from_index(0), Some(View::Cpu));
        assert_eq!(View::from_index(7), None);
        assert_eq!(View::from_index(99), None);
    }

    #[test]
    fn next_and_previous_view_wrap_around() {
        let mut app = App::new(Config::default());
        app.view = View::System;
        app.next_view();
        assert_eq!(app.view, View::Cpu);
        app.previous_view();
        assert_eq!(app.view, View::System);
    }

    #[test]
    fn update_populates_history_buffers() {
        let mut app = App::new(Config::default());
        app.update(snapshot_with_processes(0));
        app.update(snapshot_with_processes(0));
        assert_eq!(app.cpu_history.len(), 2);
        assert_eq!(app.memory_history.len(), 2);
        assert_eq!(app.memory_history.last(), Some(50.0));
        assert_eq!(app.network_rx_history.len(), 2);
    }

    #[test]
    fn core_history_is_resized_with_the_core_count() {
        let mut app = App::new(Config::default());
        let mut snapshot = snapshot_with_processes(0);
        snapshot.cpu.cores = vec![
            crate::collector::cpu::CpuCore {
                usage: 10.0,
                ..Default::default()
            },
            crate::collector::cpu::CpuCore {
                usage: 20.0,
                ..Default::default()
            },
        ];
        app.update(snapshot);
        assert_eq!(app.core_history.len(), 2);
        assert_eq!(app.core_history[1].last(), Some(20.0));
    }

    #[test]
    fn processes_are_sorted_by_cpu_descending_by_default() {
        let app = app_with_processes(5);
        let visible = app.visible_processes();
        assert_eq!(visible.len(), 5);
        assert_eq!(visible[0].pid.as_u32(), 1);
        assert!(
            visible
                .windows(2)
                .all(|pair| pair[0].cpu_usage >= pair[1].cpu_usage)
        );
    }

    #[test]
    fn search_filters_by_name() {
        let mut app = app_with_processes(5);
        app.search.text = "proc3".to_string();
        let visible = app.visible_processes();
        assert_eq!(visible.len(), 1);
        assert_eq!(visible[0].pid.as_u32(), 4);
    }

    #[test]
    fn search_by_pid_field_matches_exactly() {
        let mut app = app_with_processes(5);
        app.search.text = "2".to_string();
        app.search.field = processes::SearchField::Pid;
        assert_eq!(app.visible_processes().len(), 1);
    }

    #[test]
    fn selection_moves_and_clamps() {
        let mut app = app_with_processes(3);
        app.move_selection(1);
        assert_eq!(app.selected_process, 1);
        app.move_selection(-5);
        assert_eq!(app.selected_process, 0);
        app.move_selection(100);
        assert_eq!(app.selected_process, 2);
    }

    #[test]
    fn selection_is_clamped_when_processes_disappear() {
        let mut app = app_with_processes(5);
        app.move_selection(4);
        assert_eq!(app.selected_process, 4);
        app.update(snapshot_with_processes(1));
        assert_eq!(app.selected_process, 0);
    }

    #[test]
    fn sorting_the_active_column_reverses_the_order() {
        let mut app = app_with_processes(3);
        // The default configuration already sorts by CPU, so the first press reverses
        // the order rather than selecting a new column.
        assert_eq!(app.sort_key, SortKey::Cpu);
        assert!(app.sort_descending);
        app.set_sort(SortKey::Cpu);
        assert!(!app.sort_descending);
        app.set_sort(SortKey::Cpu);
        assert!(app.sort_descending);
    }

    #[test]
    fn changing_the_sort_column_uses_the_natural_order() {
        let mut app = App::new(Config::default());
        app.set_sort(SortKey::Pid);
        assert_eq!(app.sort_key, SortKey::Pid);
        assert!(!app.sort_descending, "identifiers read best ascending");
        app.set_sort(SortKey::Memory);
        assert!(app.sort_descending, "usage reads best largest first");
    }

    #[test]
    fn search_key_handling() {
        let mut app = App::new(Config::default());
        app.start_search();
        assert!(app.search_active);
        assert!(app.handle_search_key(crate::events::Key::Char('a')));
        assert!(app.handle_search_key(crate::events::Key::Char('b')));
        assert_eq!(app.search.text, "ab");
        assert!(app.handle_search_key(crate::events::Key::Backspace));
        assert_eq!(app.search.text, "a");
        assert!(app.handle_search_key(crate::events::Key::Enter));
        assert!(!app.search_active);
    }

    #[test]
    fn escape_clears_the_search() {
        let mut app = app_with_processes(3);
        app.search.text = "proc".to_string();
        assert!(app.handle_search_key(crate::events::Key::Esc));
        assert!(app.search.text.is_empty());
        assert!(!app.search_active);
    }

    #[test]
    fn confirm_kill_without_a_dialog_is_a_no_op() {
        let mut app = app_with_processes(1);
        app.confirm_kill();
        assert!(app.message.is_none());
    }

    #[test]
    fn confirm_kill_reports_failures_instead_of_panicking() {
        let mut app = app_with_processes(1);
        app.dialog = Some(Dialog::ConfirmKill {
            pid: 1,
            name: "init".to_string(),
            signal: processes::TerminateSignal::Term,
        });
        app.confirm_kill();
        assert!(app.dialog.is_none());
        let message = app.message.expect("a message must be set");
        // Either the signal was refused or pid 1 is gone; both are reported, never
        // silently ignored.
        assert!(!message.text.is_empty());
    }

    #[test]
    fn status_messages_expire() {
        let message = StatusMessage::info("hello");
        assert!(!message.is_expired());
        let expired = StatusMessage {
            ttl: Duration::from_secs(0),
            ..StatusMessage::info("x")
        };
        assert!(expired.is_expired());
    }

    #[test]
    fn history_resizes_with_the_configuration() {
        let config = Config {
            graph_history: 32,
            ..Config::default()
        };
        let mut app = App::new(config);
        for _ in 0..64 {
            app.update(snapshot_with_processes(0));
        }
        assert_eq!(app.cpu_history.capacity(), 32);
        assert_eq!(app.cpu_history.len(), 32);
    }

    #[test]
    fn core_history_tracks_configured_capacity() {
        let config = Config {
            graph_history: 16,
            ..Config::default()
        };
        let mut app = App::new(config);
        let mut snapshot = snapshot_with_processes(0);
        snapshot.cpu.cores = vec![crate::collector::cpu::CpuCore::default()];
        for _ in 0..40 {
            app.update(snapshot.clone());
        }
        assert!(app.core_history[0].capacity() <= Series::new(16).capacity());
        assert!(app.core_history[0].len() <= 16);
    }
}
