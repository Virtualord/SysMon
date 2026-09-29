//! Keyboard, tick and resize events, plus the background monitoring thread.
//!
//! The main thread owns the terminal and does nothing but draw and read input. All
//! metric collection happens on a worker thread that pushes [`Snapshot`]s over a
//! bounded channel, so a slow terminal can never stall data collection and a burst of
//! input can never make the collector block.

use std::io;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc::{Receiver, SyncSender, TrySendError, sync_channel};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use crossterm::event::{
    self, Event as CrosstermEvent, KeyEvent, KeyEventKind, KeyModifiers, MouseEventKind,
};

use crate::app::{Action, App, StatusMessage, View};
use crate::collector::{Collector, CollectorConfig, Snapshot};
use crate::config::Config;

/// Maximum number of snapshots that may be queued before new ones are dropped.
///
/// Dropping is deliberate: stale metrics are worthless, and blocking the collector
/// would make the refresh interval meaningless.
const SNAPSHOT_CHANNEL_CAPACITY: usize = 4;

/// Largest time slice spent waiting for a key press when no explicit timeout is
/// given. Callers that care about CPU usage pass their own deadline instead.
const DEFAULT_INPUT_POLL_INTERVAL: Duration = Duration::from_millis(50);

/// How long [`poll_events`] keeps draining after the first event arrives.
///
/// Long enough to coalesce whatever the terminal already buffered into one repaint,
/// short enough that a single key press still feels immediate. Terminal key repeat
/// starts around 25 ms, so this deliberately sits below that: a held arrow key should
/// produce one frame per repeat, not one frame at the end of the burst.
const INPUT_BURST_WINDOW: Duration = Duration::from_millis(8);

/// Number of frames the "force refresh" key advances the tick without new data.
const REFRESH_DEADLINE_FRAMES: u8 = 20;

/// A normalized key press, independent of the backend's modifier encoding.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Key {
    /// A printable character, already lowercased unless shifted.
    Char(char),
    /// Return.
    Enter,
    /// Escape.
    Esc,
    /// Backspace.
    Backspace,
    /// Delete.
    Delete,
    /// Tab, forward.
    Tab,
    /// Shift+Tab, backward.
    BackTab,
    /// Up arrow.
    Up,
    /// Down arrow.
    Down,
    /// Left arrow.
    Left,
    /// Right arrow.
    Right,
    /// Page up.
    PageUp,
    /// Page down.
    PageDown,
    /// Home.
    Home,
    /// End.
    End,
    /// Ctrl+C.
    CtrlC,
    /// Any key with no binding.
    Unbound,
}

impl Key {
    /// Converts a crossterm key event into a [`Key`].
    pub fn from_crossterm(event: KeyEvent) -> Self {
        // Terminals with "key event" reporting enabled send both Press and Release.
        // Handling both would double every keystroke, so only Press is mapped.
        if event.kind != KeyEventKind::Press {
            return Self::Unbound;
        }
        if event.modifiers.contains(KeyModifiers::CONTROL) {
            return match event.code {
                event::KeyCode::Char('c') | event::KeyCode::Char('C') => Self::CtrlC,
                _ => Self::Unbound,
            };
        }
        let shift = event.modifiers.contains(KeyModifiers::SHIFT);
        match event.code {
            event::KeyCode::Char(' ') => Self::Char(' '),
            event::KeyCode::Char(ch) if shift => Self::Char(ch),
            event::KeyCode::Char(ch) => Self::Char(ch.to_ascii_lowercase()),
            event::KeyCode::Enter => Self::Enter,
            event::KeyCode::Esc => Self::Esc,
            event::KeyCode::Backspace => Self::Backspace,
            event::KeyCode::Delete => Self::Delete,
            event::KeyCode::Tab => Self::Tab,
            event::KeyCode::BackTab => Self::BackTab,
            event::KeyCode::Up => Self::Up,
            event::KeyCode::Down => Self::Down,
            event::KeyCode::Left => Self::Left,
            event::KeyCode::Right => Self::Right,
            event::KeyCode::PageUp => Self::PageUp,
            event::KeyCode::PageDown => Self::PageDown,
            event::KeyCode::Home => Self::Home,
            event::KeyCode::End => Self::End,
            _ => Self::Unbound,
        }
    }

    /// A printable character, if the key is one.
    pub fn as_char(&self) -> Option<char> {
        match self {
            Self::Char(ch) => Some(*ch),
            _ => None,
        }
    }
}

/// A message produced by the monitoring thread.
#[derive(Debug)]
pub enum MonitorMessage {
    /// A fresh set of metrics.
    Snapshot(Box<Snapshot>),
    /// The collector could not be created or has died.
    Failed(String),
}

/// Handle to the background monitoring thread.
///
/// Dropping the handle stops the thread: the collector loop watches a shutdown flag
/// that is set when the channel is disconnected, which happens as soon as the
/// receiver is dropped.
pub struct Monitor {
    receiver: Receiver<MonitorMessage>,
    shutdown: Arc<AtomicBool>,
    refresh: Arc<AtomicU64>,
    handle: Option<JoinHandle<()>>,
    ticks: u64,
    last_sequence: u64,
}

impl Monitor {
    /// Spawns the monitoring thread for the given configuration.
    pub fn spawn(config: &Config) -> Self {
        let collector_config = CollectorConfig {
            interval: config.interval(),
            slow_refresh_every: 5,
            show_pseudo_filesystems: config.show_pseudo_filesystems,
            network_interfaces: config.network_interfaces.clone(),
            collect_processes: true,
            collect_disks: config.show_disk_health,
        };
        let (sender, receiver) = sync_channel(SNAPSHOT_CHANNEL_CAPACITY);
        let refresh = Arc::new(AtomicU64::new(0));
        let shutdown = Arc::new(AtomicBool::new(false));

        let handle = std::thread::Builder::new()
            .name("syswatch-collector".to_string())
            .spawn({
                let refresh = Arc::clone(&refresh);
                let shutdown = Arc::clone(&shutdown);
                move || run(collector_config, sender, refresh, shutdown)
            })
            .ok();

        Self {
            receiver,
            shutdown,
            refresh,
            handle,
            ticks: 0,
            last_sequence: 0,
        }
    }

    /// Requests an immediate refresh outside the regular interval.
    pub fn request_refresh(&self) {
        self.refresh.fetch_add(1, Ordering::SeqCst);
    }

    /// Number of frames elapsed since the last snapshot.
    pub fn frames_since_snapshot(&self) -> u8 {
        self.ticks.min(u64::from(REFRESH_DEADLINE_FRAMES)) as u8
    }

    /// Non-blocking drain of every queued message.
    ///
    /// Only the newest snapshot is returned: intermediate samples would be drawn and
    /// immediately replaced, and skipping them keeps the render path cheap even when a
    /// slow terminal lags behind a fast refresh interval.
    pub fn drain(&mut self) -> Vec<MonitorMessage> {
        let mut newest: Option<MonitorMessage> = None;
        let mut failures: Vec<MonitorMessage> = Vec::new();
        while let Ok(message) = self.receiver.try_recv() {
            match message {
                MonitorMessage::Snapshot(snapshot) => {
                    // Sequence numbers are monotonic, so a late or duplicated sample is
                    // discarded rather than overwriting fresher data.
                    if snapshot.sequence > self.last_sequence {
                        self.last_sequence = snapshot.sequence;
                        newest = Some(MonitorMessage::Snapshot(snapshot));
                    }
                }
                other => failures.push(other),
            }
        }
        self.ticks = self.ticks.saturating_add(1);
        failures.extend(newest);
        failures
    }

    /// Whether the collector thread is still running.
    ///
    /// A `JoinHandle` reports completion through `is_finished`, which is exactly the
    /// signal needed to notice a collector that died unexpectedly.
    pub fn is_running(&self) -> bool {
        self.handle
            .as_ref()
            .is_some_and(|handle| !handle.is_finished())
    }

    /// Signals the collector thread to stop and waits for it to finish.
    pub fn shutdown(&mut self) {
        self.shutdown.store(true, Ordering::SeqCst);
        if let Some(handle) = self.handle.take() {
            // The thread checks the flag at least every 20 ms, so the join is quick.
            let _ = handle.join();
        }
    }
}

impl Drop for Monitor {
    fn drop(&mut self) {
        self.shutdown();
    }
}

/// The collector loop.
fn run(
    config: CollectorConfig,
    sender: SyncSender<MonitorMessage>,
    refresh: Arc<AtomicU64>,
    shutdown: Arc<AtomicBool>,
) {
    let interval = config.interval;
    let mut collector = Collector::new(config);
    let mut next_deadline = Instant::now();
    let mut seen_refresh = 0u64;

    loop {
        if shutdown.load(Ordering::SeqCst) {
            break;
        }
        let snapshot = collector.sample();
        match sender.try_send(MonitorMessage::Snapshot(Box::new(snapshot))) {
            Ok(()) => {}
            // The UI has not caught up; dropping the sample is the correct behaviour,
            // and a full queue means the receiver is still alive.
            Err(TrySendError::Full(_)) => {}
            // The receiver is gone, so the application is shutting down.
            Err(TrySendError::Disconnected(_)) => break,
        }

        // Sleep until the next deadline, waking early if a refresh is requested.
        loop {
            if shutdown.load(Ordering::SeqCst) {
                return;
            }
            let requested = refresh.load(Ordering::SeqCst);
            if requested != seen_refresh {
                seen_refresh = requested;
                break;
            }
            let now = Instant::now();
            if now >= next_deadline {
                break;
            }
            let remaining = next_deadline - now;
            std::thread::sleep(remaining.min(Duration::from_millis(20)));
        }
        next_deadline = Instant::now() + interval;
    }
}

/// Everything that happened since the last call.
#[derive(Debug, Default)]
pub struct Batch {
    /// Snapshots that arrived, oldest first (at most one is ever kept).
    pub snapshots: Vec<Snapshot>,
    /// The terminal was resized.
    pub resized: bool,
    /// The application should exit.
    pub should_quit: bool,
    /// A key was pressed; the app already applied it.
    pub key_pressed: bool,
}

/// Waits up to `timeout` for events and folds them into the application state.
///
/// The shape of this function *is* the input latency. It is tempting to keep polling
/// for the whole timeout so that a burst of key repeats is handled in one pass, but
/// that delays the repaint until the timeout expires — pressing `Tab` would take a
/// full second to switch dashboards even though the state changed immediately.
///
/// So the wait and the work are separated:
///
/// 1. Block in `poll` until the *first* event arrives or the timeout passes. A
///    timeout means nothing happened, so there is nothing to draw.
/// 2. Process that event.
/// 3. Drain whatever else is already queued, for at most [`INPUT_BURST_WINDOW`].
///
/// Step 3 coalesces events the terminal delivered in the same read into a single
/// repaint, so holding a key or pasting does not queue a frame per character, while
/// step 2 keeps a single key press responsive.
pub fn poll_events(app: &mut App, timeout: Duration) -> io::Result<Batch> {
    let mut batch = Batch::default();

    // Step 1: nothing to do means no redraw is needed.
    if !event::poll(timeout.max(DEFAULT_INPUT_POLL_INTERVAL))? {
        return Ok(batch);
    }

    // Step 2 and 3: handle the first event, then whatever is already buffered.
    let burst_deadline = Instant::now() + INPUT_BURST_WINDOW;
    loop {
        let event = event::read()?;
        let quit = handle_event(app, &mut batch, event)?;
        if quit {
            batch.should_quit = true;
            return Ok(batch);
        }

        let remaining = burst_deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() || !event::poll(remaining)? {
            break;
        }
    }
    Ok(batch)
}

/// Folds one terminal event into the application state. Returns whether to stop.
fn handle_event(app: &mut App, batch: &mut Batch, event: CrosstermEvent) -> io::Result<bool> {
    match event {
        CrosstermEvent::Key(key) if key.kind == KeyEventKind::Press => {
            let key = Key::from_crossterm(key);
            batch.key_pressed = true;
            let action = handle_key(app, key);
            apply(app, action);
            tracing::debug!(?action, ?app.dialog, should_quit = app.should_quit, "action applied");
            // A modal or the search prompt owns the keyboard: stop immediately so its
            // overlay is drawn before any further input is consumed.
            Ok(app.should_quit || app.dialog.is_some() || app.search_owns_input())
        }
        CrosstermEvent::Resize(_, _) => {
            batch.resized = true;
            Ok(false)
        }
        CrosstermEvent::Mouse(mouse) => {
            if matches!(mouse.kind, MouseEventKind::ScrollUp) {
                apply(app, Action::SelectPrevious);
                batch.key_pressed = true;
            } else if matches!(mouse.kind, MouseEventKind::ScrollDown) {
                apply(app, Action::SelectNext);
                batch.key_pressed = true;
            }
            Ok(app.should_quit)
        }
        // Focus changes, pasted text and any event kind this crossterm version adds
        // later need no action, but must be matched to keep the match exhaustive.
        _ => Ok(false),
    }
}

/// Routes a key press, mutating the search buffer when the prompt is active.
///
/// The search prompt is the one place where a key press is *not* translated into an
/// action: the character goes straight into the query. Everything else funnels through
/// [`map_key`] so the bindings stay in one table.
pub fn handle_key(app: &mut App, key: Key) -> Action {
    tracing::debug!(?key, dialog = ?app.dialog, search = app.search_owns_input(), "key press");
    if app.search_owns_input() {
        // Tab leaves the prompt but keeps the query, Esc cancels it entirely.
        if !app.handle_search_key(key) {
            return Action::None;
        }
        return Action::None;
    }
    map_key(app, key)
}

/// Translates a key into an [`Action`], honouring the current dialog state.
///
/// Context sensitivity lives here: a modal swallows most keys, and the search prompt is
/// handled by [`handle_key`] before this function is reached. This is what keeps the
/// letter bindings usable while the process table is filtered.
pub fn map_key(app: &App, key: Key) -> Action {
    if key == Key::CtrlC {
        return Action::Quit;
    }

    // Modal dialogs: only the keys the dialog documents reach the app, so a stray
    // `k` in the process table can never terminate anything while a dialog is open.
    if let Some(dialog) = app.dialog.clone() {
        return match (dialog, key) {
            (crate::app::Dialog::ConfirmKill { .. }, Key::Esc | Key::Char('n')) => {
                Action::CloseDialog
            }
            (crate::app::Dialog::ConfirmKill { .. }, Key::Enter | Key::Char('y')) => {
                Action::ConfirmKill
            }
            (crate::app::Dialog::ConfirmKill { .. }, Key::Char('k')) => Action::ToggleKillSignal,
            (_, Key::Esc | Key::Char('q') | Key::Char('?')) => Action::CloseDialog,
            (_, Key::Tab) => Action::ToggleHelp,
            _ => Action::None,
        };
    }

    match key {
        Key::Esc => Action::CloseDialog,
        Key::Tab => Action::NextView,
        Key::BackTab => Action::PreviousView,
        Key::Up => Action::SelectPrevious,
        Key::Down => Action::SelectNext,
        Key::PageUp => Action::PageUp,
        Key::PageDown => Action::PageDown,
        Key::Home => Action::SelectFirst,
        Key::End => Action::SelectLast,
        Key::Enter => Action::OpenDetails,
        Key::Char('q') => Action::Quit,
        Key::Char('?') => Action::ToggleHelp,
        Key::Char('/') => Action::StartSearch,
        Key::Char('c') => Action::SortByCpu,
        Key::Char('m') => Action::SortByMemory,
        Key::Char('p') => Action::SortByPid,
        Key::Char('n') => Action::SortByName,
        Key::Char('k') => Action::StartKill,
        Key::Char('r') => Action::Refresh,
        Key::Char('s') => Action::ToggleSortDirection,
        Key::Char('f') => Action::CycleFilter,
        Key::Char('i') => Action::CycleSearchField,
        Key::Char('g') => Action::ToggleGraphStyle,
        Key::Char(ch @ '1'..='6') => Action::SelectView(ch as usize - '0' as usize),
        _ => Action::None,
    }
}

/// Applies an action to the application state.
pub fn apply(app: &mut App, action: Action) {
    match action {
        Action::Quit => app.should_quit = true,
        Action::Redraw => {}
        Action::Refresh => {
            app.pending_refresh = true;
            app.notify(StatusMessage::info("refreshing metrics"));
        }
        Action::NextView => {
            app.next_view();
            app.focus = crate::app::Focus::Main;
        }
        Action::PreviousView => {
            app.previous_view();
            app.focus = crate::app::Focus::Main;
        }
        Action::SelectView(index) => {
            if let Some(view) = View::from_index(index) {
                app.view = view;
                app.focus = crate::app::Focus::Main;
            }
        }
        Action::ToggleHelp => {
            app.dialog = if app.dialog.is_some() {
                None
            } else {
                Some(crate::app::Dialog::Help)
            };
        }
        Action::CloseDialog => {
            if app.search_active {
                app.clear_search();
            } else {
                app.dialog = None;
            }
        }
        Action::StartSearch => {
            app.start_search();
        }
        Action::CommitSearch => {
            app.search_active = false;
        }
        Action::SearchBackspace => {
            app.search_backspace();
        }
        Action::InsertChar(_) => {}
        Action::SelectPrevious => app.move_selection(-1),
        Action::SelectNext => app.move_selection(1),
        Action::PageUp => app.move_selection(-(app.visible_rows as isize)),
        Action::PageDown => app.move_selection(app.visible_rows as isize),
        Action::SelectFirst => app.selected_process = 0,
        Action::SelectLast => {
            let rows = app.process_rows();
            app.selected_process = rows.saturating_sub(1);
        }
        Action::OpenDetails => {
            if let Some(process) = app.selected_process_info() {
                if app.view == View::Processes {
                    app.dialog = Some(crate::app::Dialog::ProcessDetails);
                    app.notify(StatusMessage::info(format!(
                        "details for {} ({})",
                        process.name,
                        process.pid.as_u32()
                    )));
                } else {
                    app.next_view();
                }
            }
        }
        Action::StartKill => {
            if app.view != View::Processes {
                app.view = View::Processes;
            }
            if let Some(process) = app.selected_process_info() {
                app.dialog = Some(crate::app::Dialog::ConfirmKill {
                    pid: process.pid.as_u32(),
                    name: process.name.clone(),
                    signal: crate::collector::processes::TerminateSignal::Term,
                });
            } else {
                app.notify(StatusMessage::error("no process selected"));
            }
        }
        Action::ToggleKillSignal => {
            if let Some(crate::app::Dialog::ConfirmKill { signal, .. }) = &mut app.dialog {
                *signal = match signal {
                    crate::collector::processes::TerminateSignal::Term => {
                        crate::collector::processes::TerminateSignal::Kill
                    }
                    crate::collector::processes::TerminateSignal::Kill => {
                        crate::collector::processes::TerminateSignal::Term
                    }
                };
            }
        }
        Action::ConfirmKill => app.confirm_kill(),
        Action::SortByCpu => app.set_sort(crate::collector::processes::SortKey::Cpu),
        Action::SortByMemory => app.set_sort(crate::collector::processes::SortKey::Memory),
        Action::SortByPid => app.set_sort(crate::collector::processes::SortKey::Pid),
        Action::SortByName => app.set_sort(crate::collector::processes::SortKey::Name),
        Action::ToggleSortDirection => app.toggle_sort_direction(),
        Action::CycleFilter => {
            let next = next_filter(app.filter());
            app.set_filter(next);
        }
        Action::CycleSearchField => {
            let next = next_search_field(app.search().field);
            app.set_search_field(next);
        }
        Action::ToggleGraphStyle => {
            app.config.unicode_graphs = !app.config.unicode_graphs;
            let style = if app.config.unicode_graphs {
                "unicode"
            } else {
                "ascii"
            };
            app.notify(StatusMessage::info(format!("graph style: {style}")));
        }
        Action::FocusNext => app.focus = next_focus(app.view, app.focus),
        Action::FocusPrevious => app.focus = previous_focus(app.view, app.focus),
        Action::None => {}
    }
}

/// The next filter in the cycle.
fn next_filter(
    current: crate::collector::processes::ProcessFilter,
) -> crate::collector::processes::ProcessFilter {
    use crate::collector::processes::ProcessFilter;
    match current {
        ProcessFilter::All => ProcessFilter::User,
        ProcessFilter::User => ProcessFilter::UserTasks,
        ProcessFilter::UserTasks => ProcessFilter::All,
    }
}

/// The next search field in the cycle.
fn next_search_field(
    current: crate::collector::processes::SearchField,
) -> crate::collector::processes::SearchField {
    use crate::collector::processes::SearchField;
    match current {
        SearchField::Name => SearchField::Pid,
        SearchField::Pid => SearchField::All,
        SearchField::All => SearchField::Name,
    }
}

/// The next focusable pane of the current view.
fn next_focus(view: View, current: crate::app::Focus) -> crate::app::Focus {
    use crate::app::Focus;
    match view {
        View::Processes => match current {
            Focus::ProcessRow => Focus::Main,
            _ => Focus::ProcessRow,
        },
        View::Storage => match current {
            Focus::StorageRow => Focus::Main,
            _ => Focus::StorageRow,
        },
        View::Network => match current {
            Focus::NetworkRow => Focus::Main,
            _ => Focus::NetworkRow,
        },
        _ => Focus::Main,
    }
}

/// The previous focusable pane of the current view.
fn previous_focus(view: View, current: crate::app::Focus) -> crate::app::Focus {
    use crate::app::Focus;
    match view {
        View::Processes => match current {
            Focus::ProcessRow => Focus::Main,
            _ => Focus::ProcessRow,
        },
        View::Storage => match current {
            Focus::StorageRow => Focus::Main,
            _ => Focus::StorageRow,
        },
        View::Network => match current {
            Focus::NetworkRow => Focus::Main,
            _ => Focus::NetworkRow,
        },
        _ => Focus::Main,
    }
}

/// Re-exported so `main.rs` does not need a direct crossterm dependency for errors.
pub type IoResult<T> = std::io::Result<T>;

#[cfg(test)]
mod tests {
    use super::*;
    use crossterm::event::KeyCode;

    fn key(code: KeyCode) -> Key {
        Key::from_crossterm(KeyEvent::new(code, KeyModifiers::NONE))
    }
    fn key_mod(code: KeyCode, modifiers: KeyModifiers) -> Key {
        Key::from_crossterm(KeyEvent::new_with_kind(
            code,
            modifiers,
            KeyEventKind::Press,
        ))
    }
    fn app() -> App {
        App::new(Config::default())
    }

    #[test]
    fn maps_characters_to_lowercase() {
        assert_eq!(key(KeyCode::Char('c')), Key::Char('c'));
        assert_eq!(key(KeyCode::Char('C')), Key::Char('c'));
    }

    #[test]
    fn maps_control_c_to_quit() {
        assert_eq!(
            map_key(&app(), key_mod(KeyCode::Char('c'), KeyModifiers::CONTROL)),
            Action::Quit
        );
    }

    #[test]
    fn ignores_key_releases() {
        // Terminals with key-event reporting send both press and release; handling
        // both would double every keystroke.
        let release = KeyEvent::new_with_kind(
            KeyCode::Char('q'),
            KeyModifiers::NONE,
            KeyEventKind::Release,
        );
        assert_eq!(Key::from_crossterm(release), Key::Unbound);
    }

    #[test]
    fn maps_number_keys_to_views() {
        assert_eq!(map_key(&app(), Key::Char('1')), Action::SelectView(1));
        assert_eq!(map_key(&app(), Key::Char('6')), Action::SelectView(6));
    }

    #[test]
    fn maps_navigation_keys() {
        assert_eq!(map_key(&app(), Key::Up), Action::SelectPrevious);
        assert_eq!(map_key(&app(), Key::Down), Action::SelectNext);
        assert_eq!(map_key(&app(), Key::Tab), Action::NextView);
        assert_eq!(map_key(&app(), Key::BackTab), Action::PreviousView);
    }

    #[test]
    fn dialogs_swallow_unrelated_keys() {
        let mut application = app();
        application.dialog = Some(crate::app::Dialog::Help);
        assert_eq!(map_key(&application, Key::Char('c')), Action::None);
        assert_eq!(map_key(&application, Key::Char('q')), Action::CloseDialog);
    }

    #[test]
    fn confirm_dialog_only_accepts_yes_and_no() {
        let mut application = app();
        application.dialog = Some(crate::app::Dialog::ConfirmKill {
            pid: 1,
            name: "init".to_string(),
            signal: crate::collector::processes::TerminateSignal::Term,
        });
        assert_eq!(map_key(&application, Key::Enter), Action::ConfirmKill);
        assert_eq!(map_key(&application, Key::Char('y')), Action::ConfirmKill);
        assert_eq!(map_key(&application, Key::Char('n')), Action::CloseDialog);
        assert_eq!(
            map_key(&application, Key::Char('k')),
            Action::ToggleKillSignal
        );
    }

    #[test]
    fn search_prompt_consumes_typed_characters() {
        let mut application = app();
        application.start_search();
        // The character goes into the query instead of triggering the `c` binding.
        assert_eq!(handle_key(&mut application, Key::Char('c')), Action::None);
        assert_eq!(application.search().text, "c");
        assert_eq!(
            application.sort_key(),
            crate::collector::processes::SortKey::Cpu
        );
    }

    #[test]
    fn escape_exits_the_search_prompt_and_clears_it() {
        let mut application = app();
        application.start_search();
        handle_key(&mut application, Key::Char('a'));
        assert_eq!(handle_key(&mut application, Key::Esc), Action::None);
        assert!(!application.search_active);
        assert!(application.search().text.is_empty());
    }

    #[test]
    fn enter_keeps_the_query_and_closes_the_prompt() {
        let mut application = app();
        application.start_search();
        handle_key(&mut application, Key::Char('a'));
        handle_key(&mut application, Key::Enter);
        assert!(!application.search_active);
        assert_eq!(application.search().text, "a");
    }

    #[test]
    fn tab_leaves_the_prompt_but_keeps_the_query() {
        let mut application = app();
        application.start_search();
        handle_key(&mut application, Key::Char('a'));
        assert_eq!(handle_key(&mut application, Key::Tab), Action::None);
        assert!(!application.search_active);
        assert_eq!(application.search().text, "a");
    }

    #[test]
    fn toggle_kill_signal_switches_between_term_and_kill() {
        use crate::collector::processes::TerminateSignal;
        let mut application = app();
        application.dialog = Some(crate::app::Dialog::ConfirmKill {
            pid: 1,
            name: "x".to_string(),
            signal: TerminateSignal::Term,
        });
        apply(&mut application, Action::ToggleKillSignal);
        let crate::app::Dialog::ConfirmKill { signal, .. } = application.dialog.expect("dialog")
        else {
            panic!("expected the kill dialog");
        };
        assert_eq!(signal, TerminateSignal::Kill);
    }

    #[test]
    fn refresh_action_sets_the_pending_flag() {
        let mut application = app();
        apply(&mut application, Action::Refresh);
        assert!(application.pending_refresh);
    }

    #[test]
    fn toggling_graph_style_flips_the_config() {
        let mut application = app();
        let before = application.config.unicode_graphs;
        apply(&mut application, Action::ToggleGraphStyle);
        assert_eq!(application.config.unicode_graphs, !before);
    }

    #[test]
    fn cycling_filters_returns_to_the_start() {
        use crate::collector::processes::ProcessFilter;
        let mut current = ProcessFilter::All;
        for _ in 0..3 {
            current = next_filter(current);
        }
        assert_eq!(current, ProcessFilter::All);
    }

    #[test]
    fn cycling_search_fields_returns_to_the_start() {
        use crate::collector::processes::SearchField;
        let mut current = SearchField::Name;
        for _ in 0..3 {
            current = next_search_field(current);
        }
        assert_eq!(current, SearchField::Name);
    }

    #[test]
    fn monitor_delivers_snapshots_and_stops_cleanly() {
        let config = Config {
            refresh_interval: 0.1,
            ..Config::default()
        };
        let mut monitor = Monitor::spawn(&config);
        let mut received = None;
        for _ in 0..100 {
            for message in monitor.drain() {
                if let MonitorMessage::Snapshot(snapshot) = message {
                    received = Some(*snapshot);
                }
            }
            if received.is_some() {
                break;
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        assert!(
            received.is_some(),
            "the monitor thread must deliver a snapshot"
        );
        monitor.shutdown();
        assert!(!monitor.is_running());
    }

    #[test]
    fn monitor_request_refresh_does_not_block() {
        let config = Config {
            refresh_interval: 60.0,
            ..Config::default()
        };
        let monitor = Monitor::spawn(&config);
        monitor.request_refresh();
        // Returns immediately even though the interval is a minute.
        let _ = monitor.frames_since_snapshot();
    }
}
