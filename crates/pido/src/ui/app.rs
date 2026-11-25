//! The root view: Ely's FocusScope wrapping a sidebar and the session pane.

use ely_gpui_component::forms::{Enter, InputEvent, TextInput};
use ely_gpui_component::layout::SplitPane;
use ely_gpui_component::primitives::FocusScope;
use ely_gpui_component::theme::{ActiveTheme, ContainerSize, Mode, Theme};
use gpui::{
    App, AppContext, Context, FocusHandle, Focusable, InteractiveElement, IntoElement,
    ParentElement, Render, StatefulInteractiveElement, Styled, Window, actions, div,
};

use crate::i18n;
use crate::state::{AppState, Conn, PaletteAction, PaletteItem, PaletteKind, TranscriptItem};
use crate::ui::{chat, composer, settings, sidebar, welcome};

actions!(
    pido,
    [
        Stop,
        PinSession,
        RenameSession,
        ForkSession,
        ArchiveSession,
        Find,
        Palette
    ]
);

pub const HEADER_HEIGHT: f32 = 52.;

/// The sidebar's default width, in logical pixels.
const SIDEBAR_DEFAULT: f32 = 264.0;
/// The sidebar drags within the range the README promises.
const SIDEBAR_RANGE: std::ops::RangeInclusive<f32> = 200.0..=480.0;
/// The right panel's width range, in logical pixels.
const RIGHT_RANGE: std::ops::RangeInclusive<f32> = 200.0..=900.0;

/// Keep a pane's width inside its range; dragging past a bound holds the
/// value there, and the next render reseeds the split back into range.
fn clamp_width(width: f32, range: &std::ops::RangeInclusive<f32>) -> f32 {
    width.clamp(*range.start(), *range.end())
}

/// One backing session: its state entity plus the view state that rides
/// along with it. Everything here dies with the session; the shell's own
/// fields (pane widths, dialogs over projects, the right panel's chrome)
/// outlive it. M0 keeps exactly one slot — later milestones grow the Vec
/// so a switched-away session keeps its pi process running.
pub struct SessionSlot {
    /// Identifies the slot across Vec reshuffles; observers capture it.
    id: u64,
    pub state: gpui::Entity<AppState>,
    /// The composer's multi-line field, owned here so it survives re-renders.
    pub draft: Option<gpui::Entity<TextInput>>,
    /// Whether the composer has been focused since the session went live.
    composer_focused: bool,
    /// The dialog's text field (input/editor), per pending request id.
    dialog_field: Option<gpui::Entity<TextInput>>,
    /// The request id `dialog_field` was built for.
    dialog_field_for: Option<String>,
    /// The session-rename dialog: open, with its own field, and the session
    /// it names (None means the open session without a file yet).
    rename_open: bool,
    rename_field: Option<gpui::Entity<TextInput>>,
    rename_target: Option<String>,
    /// The right panel's diff pick and layout, per session.
    pub diff_selected: Option<String>,
    pub diff_layout: ely_gpui_component::git::DiffLayout,
    /// The panel's terminal, spawned on first use and kept while the slot
    /// lives.
    pub terminal: Option<gpui::Entity<ely_gpui_component::terminal::Terminal>>,
    /// The transcript's scroll state, owned here so search can jump to a row
    /// and the tail-follow badge survives re-renders. Rebuilt whenever
    /// `list_epoch` falls behind the state's `session_epoch`.
    pub transcript_list: Option<gpui::ListState>,
    pub list_epoch: u64,
    /// The tail-follow badge state, owned separately from the view so the
    /// list's paint pass can read it without re-entering the view.
    pub transcript_scroll: Option<gpui::Entity<TranscriptScroll>>,
    /// The transcript find bar, while Ctrl+F has it open.
    pub find: Option<FindState>,
    /// The command palette, while Ctrl+K has it open.
    pub palette: Option<PaletteState>,
    /// Bumped each time the palette opens; keys its picker state so a stale
    /// dismissal never sticks.
    palette_gen: u64,
    /// The palette's highlighted row, over the filtered list.
    palette_selected: usize,
    /// The session epoch the find bar and palette were opened for; changing
    /// sessions drops both so their indices cannot outlive the transcript.
    session_seen: u64,
    /// The shell's observe registration for this slot's state; dropped with
    /// the slot so a closed session stops talking to the shell.
    subscription: Option<gpui::Subscription>,
    /// Whether the slot's session was running at the last observation: the
    /// transition to idle marks it unread while it is in the background.
    last_running: bool,
    /// The session finished work while the window showed another slot.
    pub unread: bool,
}

impl SessionSlot {
    fn new(id: u64, state: gpui::Entity<AppState>) -> Self {
        Self {
            id,
            state,
            draft: None,
            composer_focused: false,
            dialog_field: None,
            dialog_field_for: None,
            rename_open: false,
            rename_field: None,
            rename_target: None,
            diff_selected: None,
            diff_layout: ely_gpui_component::git::DiffLayout::Unified,
            terminal: None,
            transcript_list: None,
            list_epoch: 0,
            transcript_scroll: None,
            find: None,
            palette: None,
            palette_gen: 0,
            palette_selected: 0,
            session_seen: 0,
            subscription: None,
            last_running: false,
            unread: false,
        }
    }
}

pub struct PidoApp {
    /// The panes' widths in logical pixels, always clamped to their ranges.
    /// Shared with the resize handler, which cannot reach the view: dragging
    /// past a limit holds the value at the limit, and the next render reseeds
    /// the split back into range.
    left_px: std::rc::Rc<std::cell::Cell<f32>>,
    right_px: std::rc::Rc<std::cell::Cell<f32>>,
    /// Set by the resize handler when a drag leaves the range, cleared by the
    /// render that has reseeded the split back in: while it is fresh, the
    /// seed alternates by a hair so the split keeps pulling the pane back.
    snap_since: std::rc::Rc<std::cell::Cell<Option<std::time::Instant>>>,
    snap_flip: std::rc::Rc<std::cell::Cell<u64>>,
    pub focus: FocusHandle,
    /// The backing sessions. Slot zero exists from startup; opening a
    /// session that no slot holds grows the Vec, and the sidebar switches
    /// between them.
    slots: Vec<SessionSlot>,
    /// Which slot the window shows. Always valid: slots are never removed
    /// while it points there.
    active: usize,
    /// The session scanner every slot shares, created here and handed to
    /// each new `AppState`.
    scanner: std::sync::Arc<std::sync::Mutex<pi_rpc::sessions::SessionScanner>>,
    /// Monotonic slot ids for the observers.
    next_slot_id: u64,
    /// The welcome screen's project-directory field, created lazily.
    pub welcome_dir: Option<gpui::Entity<TextInput>>,
    /// Last observed window size, persisted on quit.
    window_size: Option<(f32, f32)>,
    /// Throttles pane-width writes while the split is being dragged.
    pane_save_at: std::rc::Rc<std::cell::Cell<Option<std::time::Instant>>>,
    /// The project-rename dialog: the project path being named. When set it
    /// takes over the same dialog slot as the session rename.
    rename_project: Option<String>,
    /// The remove-project confirmation: which path it is about.
    confirm_remove: Option<String>,
    /// The delete-session confirmation: the file path and its display title.
    confirm_delete: Option<(String, String)>,
    /// The settings window: the page it shows, or `None` when closed.
    pub settings_open: Option<settings::SettingsPage>,
    /// What the settings window shows, read once per open.
    pub settings_snapshot: Option<settings::SettingsSnapshot>,
    /// The settings overlay's keyboard focus. The card has no text fields, but
    /// without this the composer behind the scrim keeps the focus and typing
    /// lands in the chat draft.
    pub settings_focus: FocusHandle,
    /// Edit fields for pi's startup defaults and the add-extension row;
    /// created when the window opens, dropped when it closes.
    pub settings_provider_input: Option<gpui::Entity<TextInput>>,
    pub settings_model_input: Option<gpui::Entity<TextInput>>,
    pub settings_ext_input: Option<gpui::Entity<TextInput>>,
    /// Which scope the add-extension row writes to.
    pub ext_add_scope: pi_rpc::resources::Scope,
    /// The provider sign-out confirmation: which provider it is about.
    pub confirm_logout: Option<String>,
    /// Keeps the appearance observer alive so `system` tracks the OS theme.
    appearance_sub: Option<gpui::Subscription>,
    /// Which right-panel tab shows, and whether the panel is open at all.
    pub right_tab: RightTab,
    right_open: bool,
    /// The sidebar's session-search field, created once in setup.
    pub search_field: Option<gpui::Entity<TextInput>>,
    /// The update banner's state; `None` while nothing is worth showing.
    /// Shell-owned: it outlives any one session slot.
    pub update: Option<crate::update::UpdateStatus>,
    /// Set once a manual check asked for visible feedback when there is
    /// nothing new (a background check stays silent).
    update_manual: bool,
}

/// What the sidebar's session rows need to know about the slots: which
/// sessions are running (a spinner on the row), which finished unseen (a
/// dot), and which are open in some slot (menu management entries).
#[derive(Default, Clone)]
pub struct SlotStatus {
    pub running: std::collections::HashSet<String>,
    pub unread: std::collections::HashSet<String>,
    pub open: std::collections::HashSet<String>,
}

/// The transcript find bar: the query field, the rows that match and which
/// match is current. Matches recompute every render.
#[derive(Clone)]
pub struct FindState {
    pub field: gpui::Entity<TextInput>,
    pub query: String,
    pub matches: Vec<usize>,
    pub current: usize,
    /// Transcript shape the matches were computed from; avoids a full rescan
    /// on every frame while the bar is open.
    scanned_count: usize,
    scanned_streaming: bool,
    scanned_at: std::time::Instant,
}

/// The command palette: its query field and an id generation that resets
/// the suggestion picker each time it opens.
#[derive(Clone)]
pub struct PaletteState {
    pub field: gpui::Entity<TextInput>,
    pub open_gen: u64,
}

/// The transcript's tail-follow badge state, in its own entity: the list's
/// paint pass reads it, and reading the view itself there would re-enter
/// the view's update.
#[derive(Debug, Default)]
pub struct TranscriptScroll {
    pub away: bool,
    pub fresh: usize,
}

/// The right panel's three tabs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RightTab {
    /// Reviewing the session's file changes.
    Diff,
    /// A live shell next to the chat.
    Terminal,
    /// The session's branch tree, read-only.
    Tree,
}

impl PidoApp {
    pub fn new(cx: &mut Context<Self>) -> Self {
        let focus = cx.focus_handle();
        // One scanner serves every slot the shell will ever hold.
        let scanner = std::sync::Arc::new(std::sync::Mutex::new(
            pi_rpc::sessions::SessionScanner::default(),
        ));
        let state = AppState::new(cx, std::sync::Arc::clone(&scanner));
        let persisted = crate::settings::Settings::load();
        let left_px = std::rc::Rc::new(std::cell::Cell::new(
            persisted
                .sidebar_width
                .unwrap_or(SIDEBAR_DEFAULT)
                .clamp(*SIDEBAR_RANGE.start(), *SIDEBAR_RANGE.end()),
        ));
        let right_px = std::rc::Rc::new(std::cell::Cell::new(
            persisted
                .right_width
                .unwrap_or(560.0)
                .clamp(*RIGHT_RANGE.start(), *RIGHT_RANGE.end()),
        ));
        let snap_since = std::rc::Rc::new(std::cell::Cell::new(None));
        let snap_flip = std::rc::Rc::new(std::cell::Cell::new(0u64));
        let right_open = persisted.right_open.unwrap_or(false);
        let mut app = Self {
            left_px,
            right_px,
            snap_since,
            snap_flip,
            focus,
            slots: vec![SessionSlot::new(0, state)],
            active: 0,
            scanner,
            next_slot_id: 1,
            welcome_dir: None,
            window_size: None,
            pane_save_at: std::rc::Rc::new(std::cell::Cell::new(None)),
            rename_project: None,
            confirm_remove: None,
            confirm_delete: None,
            settings_open: None,
            settings_snapshot: None,
            settings_focus: cx.focus_handle(),
            settings_provider_input: None,
            settings_model_input: None,
            settings_ext_input: None,
            ext_add_scope: pi_rpc::resources::Scope::User,
            confirm_logout: None,
            appearance_sub: None,
            right_tab: RightTab::Diff,
            right_open,
            search_field: None,
            update: None,
            update_manual: false,
        };
        app.observe_window_for_quit(cx);
        app.attach_slot_observer(0, &app.slot().state.clone(), cx);
        app
    }

    /// Register this slot's state with the shell: epoch transitions reset
    /// the slot's overlays and respawn its terminal, running/idle
    /// transitions maintain the sidebar's unread marks, and a cancelled
    /// action's text returns to the slot's own composer.
    fn attach_slot_observer(
        &mut self,
        id: u64,
        state: &gpui::Entity<AppState>,
        cx: &mut Context<Self>,
    ) {
        let subscription = cx.observe(state, move |this, state, cx| {
            let Some(ix) = this.slots.iter().position(|slot| slot.id == id) else {
                return;
            };
            let right_open = this.right_open;
            let right_tab = this.right_tab;
            let is_active = this.active == ix;
            let epoch = state.read(cx).session_epoch;
            let running = state.read(cx).is_running();
            let slot = &mut this.slots[ix];
            // Find matches and palette rows carry transcript indices; a new
            // session invalidates both.
            let epoch_changed = slot.session_seen != epoch;
            if epoch_changed {
                slot.session_seen = epoch;
                slot.find = None;
                slot.palette = None;
                slot.palette_selected = 0;
            }
            // The session finished its work while the window showed
            // another slot: mark it so the sidebar can point back.
            let became_idle = slot.last_running && !running && !is_active;
            if became_idle {
                slot.unread = true;
            }
            slot.last_running = running;
            let has_terminal = slot.terminal.is_some();
            let open_tree = right_open && right_tab == RightTab::Tree;

            if became_idle || epoch_changed || is_active {
                cx.notify();
            }
            // A shell left over from the previous session still sits in its
            // directory; one that never started spawns in the new cwd when
            // the panel opens.
            if epoch_changed && has_terminal {
                this.restart_terminal_for(ix, cx);
            }
            // The tree tab must not show the old session's branches.
            if epoch_changed && open_tree {
                this.slots[ix]
                    .state
                    .update(cx, |state, cx| state.refresh_session_tree(cx));
            }
            // A cancelled action's text returns to this slot's composer; a
            // slot that has not rendered yet keeps it pending until it has
            // one, so nothing is lost.
            if this.slots[ix].draft.is_some() {
                let restored = state.update(cx, |state, _| state.take_pending_composer_text());
                if let Some(text) = restored {
                    let draft = this.slots[ix].draft.clone().expect("draft checked above");
                    draft.update(cx, |input, cx| {
                        if !input.text().trim().is_empty() {
                            input.insert("\n", cx);
                        }
                        input.insert(&text, cx);
                    });
                }
            }
        });
        let slot = self
            .slots
            .iter_mut()
            .find(|slot| slot.id == id)
            .expect("slot exists");
        slot.subscription = Some(subscription);
    }

    // -- slots ---------------------------------------------------------------

    /// The largest fleet of live pi processes; each costs roughly a hundred
    /// megabytes, and four parallel tasks is already generous.
    const MAX_LIVE_SLOTS: usize = 4;

    /// Open a session file: the slot that already holds it comes forward
    /// (instantly — its transcript, draft and panel state are all in
    /// memory), and otherwise a new slot spawns its own pi for it.
    pub fn open_session(&mut self, path: String, cx: &mut Context<Self>) {
        let key = crate::state::session_key(&path);
        if let Some(ix) = self.slots.iter().position(|slot| {
            slot.state
                .read(cx)
                .session_file
                .as_deref()
                .is_some_and(|file| crate::state::session_key(file) == key)
        }) {
            self.set_active(ix, cx);
            return;
        }
        // The session's recorded project directory is where pi must run; a
        // missing one (moved or deleted project) must not spawn a pi in the
        // wrong place, and must not leave a slot that no lookup can match.
        let Some(cwd) = pi_rpc::sessions::read_session_cwd(std::path::Path::new(&path))
            .map(std::path::PathBuf::from)
            .filter(|dir| dir.is_dir())
        else {
            self.slot().state.update(cx, |s, cx| {
                s.push_notice(
                    crate::state::NoticeKind::Error,
                    i18n::t(cx, "dir_invalid").to_string(),
                );
                cx.notify();
            });
            return;
        };
        self.create_slot(cwd, Some(path), cx);
    }

    /// Back a new session with a fresh slot: a fresh pi, its own composer
    /// draft, and its own view state. Evicts the oldest idle live slot when
    /// the fleet is at capacity; streaming slots are never evicted.
    fn create_slot(
        &mut self,
        cwd: std::path::PathBuf,
        resume: Option<String>,
        cx: &mut Context<Self>,
    ) {
        while self.live_slot_count(cx) >= Self::MAX_LIVE_SLOTS {
            // Never evict the slot the user is looking at: that would drop its
            // composer draft and panel state under their feet. The oldest
            // *background* idle slot gives way instead; when every candidate is
            // the visible one, the caller reports the cap instead.
            let victim = self.slots.iter().enumerate().position(|(ix, slot)| {
                ix != self.active && {
                    let state = slot.state.read(cx);
                    matches!(
                        state.conn,
                        crate::state::Conn::Live
                            | crate::state::Conn::Connecting
                            | crate::state::Conn::Reconnecting { .. }
                    ) && !state.is_running()
                }
            });
            match victim {
                Some(ix) => self.close_slot(ix, cx),
                // Every live slot is busy: say so on the slot the user sees.
                None => {
                    self.slot().state.update(cx, |s, cx| {
                        s.push_notice(
                            crate::state::NoticeKind::Warn,
                            i18n::t(cx, "slot_cap").to_string(),
                        );
                        cx.notify();
                    });
                    return;
                }
            }
        }
        let ix = self.spawn_slot(cwd, resume, cx);
        self.set_active(ix, cx);
    }

    /// Add a slot without touching which one the window shows: used by the
    /// startup restore, which must not steal the foreground from the slot
    /// autoconnect just put there.
    fn spawn_slot(
        &mut self,
        cwd: std::path::PathBuf,
        resume: Option<String>,
        cx: &mut Context<Self>,
    ) -> usize {
        let id = self.next_slot_id;
        self.next_slot_id += 1;
        let state = AppState::new(cx, std::sync::Arc::clone(&self.scanner));
        let slot = SessionSlot::new(id, state.clone());
        self.slots.push(slot);
        let ix = self.slots.len() - 1;
        let state = self.slots[ix].state.clone();
        self.attach_slot_observer(id, &state, cx);
        self.slots[ix].state.update(cx, |s, cx| {
            s.connect(cwd, resume, crate::state::extra_args(), cx)
        });
        ix
    }

    /// How many slots currently hold a pi process.
    fn live_slot_count(&self, cx: &App) -> usize {
        self.slots
            .iter()
            .filter(|slot| {
                matches!(
                    slot.state.read(cx).conn,
                    crate::state::Conn::Live
                        | crate::state::Conn::Connecting
                        | crate::state::Conn::Reconnecting { .. }
                )
            })
            .count()
    }

    /// Show slot `ix`: its view state is already in memory, so this is a
    /// pointer move plus a sidebar resync — the settings-backed data may
    /// have moved while this slot slept.
    fn set_active(&mut self, ix: usize, cx: &mut Context<Self>) {
        self.active = ix;
        self.slots[ix].unread = false;
        // The composer takes focus once per activation. A slot shown before
        // keeps the flag set, and the render-time autofocus only fires on
        // `false`, so without this reset a switch back would leave the
        // window's focus on the previous slot's (now unrendered) field.
        self.slots[ix].composer_focused = false;
        self.slots[ix].state.update(cx, |s, cx| s.sync_sidebar(cx));
        // The sidebar's search field belongs to the window while the query
        // lives in the session: replay the visible text so the rows and the
        // field agree after a switch.
        let query = self
            .search_field
            .as_ref()
            .map(|field| field.read(cx).text().to_string())
            .unwrap_or_default();
        if !query.trim().is_empty() {
            self.slots[ix]
                .state
                .update(cx, |s, cx| s.set_search(&query, cx));
        }
        // The panel's tab is window state but the tree belongs to a session:
        // a switch must fetch the incoming session's branches.
        if self.right_open && self.right_tab == RightTab::Tree {
            self.slots[ix]
                .state
                .update(cx, |s, cx| s.refresh_session_tree(cx));
        }
        cx.notify();
    }

    /// Drop a slot entirely: its `AppState` goes with it, and dropping the
    /// last client handle shuts that pi down.
    ///
    /// Closing the last session replaces the slot with a fresh, disconnected
    /// one instead: an empty `slots` would make `slot()` panic on the next
    /// frame, and the welcome screen is the right place to land.
    fn close_slot(&mut self, ix: usize, cx: &mut Context<Self>) {
        if self.slots.len() == 1 {
            let id = self.slots[0].id;
            let state = AppState::new(cx, std::sync::Arc::clone(&self.scanner));
            self.slots[0] = SessionSlot::new(id, state.clone());
            self.attach_slot_observer(id, &state, cx);
            self.active = 0;
            cx.notify();
            return;
        }
        let slot = self.slots.remove(ix);
        drop(slot);
        if self.active >= self.slots.len() {
            self.active = self.slots.len().saturating_sub(1);
        } else if ix < self.active {
            self.active -= 1;
        }
        cx.notify();
    }

    /// Start a new chat. The active slot gets it when it is idle (the user
    /// is looking at it and it is free); a slot in the middle of a run
    /// hands the new chat to a fresh slot, so nothing is cut off.
    pub fn new_chat(&mut self, cx: &mut Context<Self>) {
        let busy = self.slot().state.read(cx).is_running();
        if !busy {
            self.slot().state.update(cx, |s, cx| s.new_chat(cx));
            return;
        }
        let cwd = self
            .slot()
            .state
            .read(cx)
            .cwd
            .clone()
            .unwrap_or_else(|| dirs::home_dir().unwrap_or_default());
        self.create_slot(cwd, None, cx);
    }

    /// Start (or return to) a session in `project`. The newest idle slot
    /// already in that project comes forward instead of being duplicated;
    /// a project whose slots are all running earns a fresh parallel one.
    pub fn start_in_project(&mut self, project: std::path::PathBuf, cx: &mut Context<Self>) {
        // The button always means "a new chat here". The project's newest
        // idle *live* slot hosts it (a background one comes forward, the
        // active one starts it in place); a project whose slots are all
        // running earns a fresh parallel slot, and a dead one is replaced
        // by a fresh pi rather than reused.
        let candidates: Vec<usize> = self
            .slots
            .iter()
            .enumerate()
            .filter(|(_, slot)| {
                let state = slot.state.read(cx);
                state.cwd.as_deref() == Some(project.as_path())
                    && state.conn == crate::state::Conn::Live
                    && !state.is_running()
            })
            .map(|(ix, _)| ix)
            .collect();
        if let Some(ix) = candidates.into_iter().next_back() {
            if self.active != ix {
                self.set_active(ix, cx);
            }
            self.slots[ix].state.update(cx, |s, cx| s.new_chat(cx));
            return;
        }
        self.create_slot(project, None, cx);
    }

    /// Stop the run in the slot holding `path`, without bringing it forward.
    pub fn stop_session(&mut self, path: String, cx: &mut Context<Self>) {
        let key = crate::state::session_key(&path);
        let Some(ix) = self.slot_index_of_key(&key, cx) else {
            return;
        };
        self.slots[ix].state.update(cx, |s, cx| s.stop(cx));
    }

    /// Close the slot holding `path`: its pi shuts down and its place in
    /// the fleet goes to the next task.
    pub fn close_session(&mut self, path: String, cx: &mut Context<Self>) {
        let key = crate::state::session_key(&path);
        let Some(ix) = self.slot_index_of_key(&key, cx) else {
            return;
        };
        self.close_slot(ix, cx);
    }

    /// Fork a session by path, routed through the slot that holds it.
    ///
    /// `fork_session` reaches a non-open session by re-binding its pi with
    /// `SwitchSession`, so it must run on the pi that already owns that
    /// session. Running it on another slot would leave two processes bound to
    /// one `.jsonl` — interleaved appends and a broken `parentId` chain.
    pub fn fork_session(&mut self, path: String, cx: &mut Context<Self>) {
        let key = crate::state::session_key(&path);
        if let Some(ix) = self.slot_index_of_key(&key, cx) {
            if self.active != ix {
                self.set_active(ix, cx);
            }
            self.slots[ix]
                .state
                .update(cx, |s, cx| s.fork_session(&path, cx));
            return;
        }
        self.slot()
            .state
            .update(cx, |s, cx| s.fork_session(&path, cx));
    }

    /// Flip pi's auto-retry for every live session. pi persists the switch in
    /// its own global settings, so a per-slot write would leave the other
    /// processes disagreeing with the file and with the settings page.
    pub fn set_auto_retry(&mut self, enabled: bool, cx: &mut Context<Self>) {
        for slot in &self.slots {
            slot.state.update(cx, |s, cx| s.set_auto_retry(enabled, cx));
        }
    }

    /// Flip pi's auto-compaction for every live session; same reasoning as
    /// [`Self::set_auto_retry`].
    pub fn set_auto_compaction(&mut self, enabled: bool, cx: &mut Context<Self>) {
        for slot in &self.slots {
            slot.state
                .update(cx, |s, cx| s.set_auto_compaction(enabled, cx));
        }
    }

    fn slot_index_of_key(&self, key: &str, cx: &App) -> Option<usize> {
        self.slots.iter().position(|slot| {
            slot.state
                .read(cx)
                .session_file
                .as_deref()
                .is_some_and(|file| crate::state::session_key(file) == key)
        })
    }

    /// The session the window shows. M0 keeps exactly one slot, so this is
    /// infallible; later milestones switch which index `active` names.
    pub fn slot(&self) -> &SessionSlot {
        self.slots.get(self.active).expect("active slot")
    }

    /// The active slot for mutation; same single-slot guarantee.
    pub fn slot_mut(&mut self) -> &mut SessionSlot {
        self.slots.get_mut(self.active).expect("active slot")
    }

    // -- update --------------------------------------------------------------

    /// Ask GitHub for the newest release. `manual` checks surface a notice
    /// when nothing is new; the startup check stays silent unless there is an
    /// update worth showing. Network failures never interrupt the user unless
    /// the check was manual.
    pub fn check_for_updates(&mut self, manual: bool, cx: &mut Context<Self>) {
        if std::env::var_os("PIDO_NO_UPDATE_CHECK").is_some() {
            return;
        }
        if !manual
            && crate::settings::Settings::load()
                .update_check
                .is_some_and(|enabled| !enabled)
        {
            return;
        }
        // One check at a time; a second would race the first's banner.
        if matches!(
            self.update,
            Some(
                crate::update::UpdateStatus::Downloading { .. }
                    | crate::update::UpdateStatus::ReadyToRestart { .. }
            )
        ) {
            return;
        }
        self.update_manual = manual;
        cx.spawn(async move |this, cx| {
            let fetched = cx
                .background_executor()
                .spawn(async { crate::update::fetch_latest() })
                .await;
            this.update(cx, |s, cx| {
                // Manual-check feedback rides the active session's notice
                // queue, exactly where a v0.4.0 user saw it.
                let announce = |s: &mut Self, cx: &mut Context<Self>, kind, text: String| {
                    s.slot()
                        .state
                        .update(cx, |state, _| state.push_notice(kind, text));
                };
                match fetched {
                    Ok(release) if crate::update::is_newer(&release.tag) => {
                        log::info!(
                            "update: {} is newer than {}",
                            release.tag,
                            crate::update::CURRENT
                        );
                        s.update = Some(crate::update::UpdateStatus::Available {
                            version: release.tag.trim_start_matches('v').to_string(),
                            page: release.page,
                            asset_url: release.asset_url,
                            asset_size: release.asset_size,
                            checksum_url: release.checksum_url,
                            installable: crate::update::appimage_path().is_some(),
                            dismissed: false,
                        });
                    }
                    Ok(release) => {
                        log::info!("update: {} is the latest release", release.tag);
                        if s.update_manual {
                            announce(
                                s,
                                cx,
                                crate::state::NoticeKind::Info,
                                i18n::t_arg(cx, "update_none", "v", crate::update::CURRENT)
                                    .to_string(),
                            );
                        }
                    }
                    Err(error) => {
                        log::info!("update check failed: {error:#}");
                        if s.update_manual {
                            announce(
                                s,
                                cx,
                                crate::state::NoticeKind::Warn,
                                i18n::t_arg(cx, "update_check_failed", "e", &error.to_string())
                                    .to_string(),
                            );
                        }
                    }
                }
                s.update_manual = false;
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    /// Hide the update banner: "not now" on an available release, or "later"
    /// on one already swapped in (it takes effect at the next launch).
    pub fn dismiss_update(&mut self, cx: &mut Context<Self>) {
        match &mut self.update {
            Some(crate::update::UpdateStatus::Available { dismissed, .. }) => {
                *dismissed = true;
                cx.notify();
            }
            Some(
                crate::update::UpdateStatus::Failed { .. }
                | crate::update::UpdateStatus::ReadyToRestart { .. },
            ) => {
                self.update = None;
                cx.notify();
            }
            _ => {}
        }
    }

    /// Fetch the new AppImage beside the running one, verify its checksum and
    /// swap it in. Only meaningful when this build runs as an AppImage.
    pub fn start_update_download(&mut self, cx: &mut Context<Self>) {
        let Some(crate::update::UpdateStatus::Available {
            version,
            asset_url: Some(url),
            checksum_url,
            ..
        }) = &self.update
        else {
            return;
        };
        let (version, url, checksum_url) = (version.clone(), url.clone(), checksum_url.clone());
        let Some(target) = crate::update::appimage_path() else {
            return;
        };
        // `.part` lives beside the AppImage so the final rename is atomic.
        let part = target.with_extension("AppImage.part");
        self.update = Some(crate::update::UpdateStatus::Downloading {
            received: 0,
            total: 0,
        });
        cx.notify();

        // Progress crosses from the blocking download to the UI through
        // plain atomics; the ticker below turns them into banner updates a
        // few times a second.
        let received = std::sync::Arc::new(std::sync::atomic::AtomicU64::new(0));
        let total = std::sync::Arc::new(std::sync::atomic::AtomicU64::new(0));
        let done = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        {
            let (received, total, done) = (received.clone(), total.clone(), done.clone());
            cx.spawn(async move |this, cx| {
                loop {
                    cx.background_executor()
                        .timer(std::time::Duration::from_millis(250))
                        .await;
                    let finished = done.load(std::sync::atomic::Ordering::Relaxed);
                    let (got, size) = (
                        received.load(std::sync::atomic::Ordering::Relaxed),
                        total.load(std::sync::atomic::Ordering::Relaxed),
                    );
                    this.update(cx, |s, cx| {
                        if matches!(
                            s.update,
                            Some(crate::update::UpdateStatus::Downloading { .. })
                        ) {
                            s.update = Some(crate::update::UpdateStatus::Downloading {
                                received: got,
                                total: size,
                            });
                            cx.notify();
                        }
                    })
                    .ok();
                    if finished {
                        break;
                    }
                }
            })
            .detach();
        }
        let part_for_cleanup = part.clone();
        cx.spawn(async move |this, cx| {
            let result = cx
                .background_executor()
                .spawn({
                    let part = part.clone();
                    let (received, total, done) = (received.clone(), total.clone(), done.clone());
                    async move {
                        let outcome = (|| {
                            let checksum = match &checksum_url {
                                Some(url) => {
                                    let text = crate::update::fetch_text(url)?;
                                    crate::update::parse_sha256(&text).ok_or_else(|| {
                                        anyhow::anyhow!("unreadable checksum file")
                                    })?
                                }
                                None => anyhow::bail!("release carries no checksum"),
                            };
                            crate::update::download(&url, &part, |got, size| {
                                received.store(got, std::sync::atomic::Ordering::Relaxed);
                                total.store(size, std::sync::atomic::Ordering::Relaxed);
                            })?;
                            crate::update::verify_sha256(&part, &checksum)?;
                            crate::update::replace_in_place(&part, &target)?;
                            Ok::<(), anyhow::Error>(())
                        })();
                        done.store(true, std::sync::atomic::Ordering::Relaxed);
                        outcome
                    }
                })
                .await;
            this.update(cx, |s, cx| {
                match result {
                    Ok(()) => {
                        s.update = Some(crate::update::UpdateStatus::ReadyToRestart { version });
                    }
                    Err(error) => {
                        let _ = std::fs::remove_file(&part_for_cleanup);
                        s.update = Some(crate::update::UpdateStatus::Failed {
                            message: error.to_string(),
                        });
                    }
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    /// Relaunch the (already replaced) AppImage and quit this process.
    pub fn restart_into_update(&mut self, cx: &mut Context<Self>) {
        let Some(path) = crate::update::appimage_path() else {
            return;
        };
        let mut command = std::process::Command::new(&path);
        if let Some(cwd) = self.slot().state.read(cx).cwd.clone() {
            command.current_dir(cwd);
        }
        match command.spawn() {
            Ok(_) => cx.quit(),
            Err(error) => {
                self.update = Some(crate::update::UpdateStatus::Failed {
                    message: format!("{}: {error}", path.display()),
                });
                cx.notify();
            }
        }
    }

    /// Stash the window size on every render and persist it when the app
    /// quits, so the window reopens where the user left it.
    fn observe_window_for_quit(&self, cx: &mut Context<Self>) {
        cx.on_app_quit(|this, cx| {
            let (left, right, right_open) =
                (this.left_px.get(), this.right_px.get(), this.right_open);
            // The open fleet rides along: the next startup brings every
            // session back in its own slot.
            let open_slots: Vec<String> = this
                .slots
                .iter()
                .filter_map(|slot| slot.state.read(cx).session_file.clone())
                .collect();
            // Which session comes back in the foreground is the one that was
            // in front, not whichever slot happened to connect last: every
            // slot writes `last_session`/`last_project` when it connects, so
            // the quit handler is the only place that can settle the order.
            let (active_session, active_project) = {
                let state = this.slot().state.read(cx);
                (
                    state.session_file.clone(),
                    state
                        .cwd
                        .as_ref()
                        .map(|cwd| cwd.to_string_lossy().into_owned()),
                )
            };
            if let Some((width, height)) = this.window_size {
                crate::settings::save_setting(|s| {
                    s.window = Some(crate::settings::WindowState { width, height });
                    s.sidebar_width = Some(left);
                    s.right_width = Some(right);
                    s.right_open = Some(right_open);
                    s.open_slots = Some(open_slots);
                    s.last_session = active_session;
                    if active_project.is_some() {
                        s.last_project = active_project;
                    }
                });
            }
            async {}
        })
        .detach();
    }

    /// Called once the window is open: fields that need a Window are created
    /// here (see main.rs).
    pub fn setup(&mut self, window: &mut gpui::Window, cx: &mut Context<Self>) {
        self.slot_mut().draft = Some(cx.new(|cx| {
            TextInput::new(window, cx)
                .multi_line(1, 8)
                .placeholder_key("idle_hint")
        }));
        // The sidebar's session search: every keystroke refilters the rows,
        // and a debounced background pass adds content matches.
        let search = cx.new(|cx| TextInput::new(window, cx).placeholder_key("search_sessions"));
        cx.subscribe(&search, move |this, field, event: &InputEvent, cx| {
            if *event == InputEvent::Changed {
                let query = field.read(cx).text().to_string();
                this.slot()
                    .state
                    .update(cx, |s, cx| s.set_search(&query, cx));
            }
        })
        .detach();
        self.search_field = Some(search);
        // A session restored with the terminal tab out gets its shell back.
        if self.right_open && self.right_tab == RightTab::Terminal {
            self.ensure_terminal(cx);
        }
        // Look for a newer release in the background; the banner appears when
        // one exists. A failure stays silent — the user did not ask.
        self.check_for_updates(false, cx);
        cx.notify();
    }

    /// Reconnect at startup so the user lands straight in their session.
    ///
    /// `PIDO_AUTOCONNECT` (used by automated checks) overrides the directory;
    /// otherwise the last project from settings is used, resuming its last
    /// session. First run, or a project directory that has vanished, falls
    /// through to the welcome screen.
    pub fn autoconnect(&mut self, cx: &mut Context<Self>) {
        let (dir, resume): (Option<std::path::PathBuf>, Option<String>) =
            match std::env::var("PIDO_AUTOCONNECT") {
                Ok(dir) => (Some(std::path::PathBuf::from(dir)), None),
                Err(_) => {
                    let settings = crate::settings::Settings::load();
                    log::info!(
                        "autoconnect: last_project={:?} last_session={:?}",
                        settings.last_project,
                        settings.last_session
                    );
                    let dir = settings.last_project.map(std::path::PathBuf::from);
                    let resume = settings
                        .last_session
                        .filter(|session| std::path::Path::new(session).is_file());
                    (dir, resume)
                }
            };
        // PIDO_EXTRA_ARGS passes raw pi flags (e.g. `-e ./my-ext.ts`), for
        // trying extensions without a settings UI.
        let extra: Vec<String> = std::env::var("PIDO_EXTRA_ARGS")
            .map(|args| args.split_whitespace().map(str::to_string).collect())
            .unwrap_or_default();
        if let Some(path) = dir {
            self.slot()
                .state
                .update(cx, |s, cx| s.connect(path, resume.clone(), extra, cx));
        }
        // Bring back the sessions that were open at quit as background
        // slots; the foreground stays where autoconnect put it. Skipped
        // under PIDO_AUTOCONNECT (automated checks must not spawn a fleet),
        // and the fleet keeps one slot of headroom - no eviction at
        // startup.
        if std::env::var_os("PIDO_AUTOCONNECT").is_some() {
            return;
        }
        let open = crate::settings::Settings::load()
            .open_slots
            .unwrap_or_default();
        let resumed = resume.as_deref().map(crate::state::session_key);
        for path in open {
            if !std::path::Path::new(&path).is_file() {
                continue;
            }
            let key = crate::state::session_key(&path);
            if resumed.as_deref() == Some(key.as_str()) {
                continue;
            }
            if self.slots.iter().any(|slot| {
                slot.state
                    .read(cx)
                    .session_file
                    .as_deref()
                    .is_some_and(|file| crate::state::session_key(file) == key)
            }) {
                continue;
            }
            if self.live_slot_count(cx) >= Self::MAX_LIVE_SLOTS - 1 {
                break;
            }
            let Some(cwd) = pi_rpc::sessions::read_session_cwd(std::path::Path::new(&path))
                .map(std::path::PathBuf::from)
            else {
                continue;
            };
            self.spawn_slot(cwd, Some(path), cx);
        }
    }

    /// The active slot's composer field, created on first render — slots
    /// are born without one so a background slot keeps pending text pending.
    fn ensure_draft_for_active(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.slot().draft.is_none() {
            let draft = cx.new(|cx| {
                TextInput::new(window, cx)
                    .multi_line(1, 8)
                    .placeholder_key("idle_hint")
            });
            self.slot_mut().draft = Some(draft);
        }
    }

    fn slot_status(&self, cx: &App) -> SlotStatus {
        let mut status = SlotStatus::default();
        for slot in &self.slots {
            let state = slot.state.read(cx);
            let Some(file) = state.session_file.clone() else {
                continue;
            };
            let key = crate::state::session_key(&file);
            if state.is_running() {
                status.running.insert(key.clone());
            }
            if slot.unread {
                status.unread.insert(key.clone());
            }
            status.open.insert(key);
        }
        status
    }
}

impl Render for PidoApp {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme();
        let (bg, fg) = (theme.colors.bg, theme.colors.fg);
        let column = theme.container_width(ContainerSize::Md);
        let bounds = window.bounds();
        self.window_size = Some((f32::from(bounds.size.width), f32::from(bounds.size.height)));
        // Track the OS appearance once a window exists, so an appearance
        // preference of "system" follows it live.
        if self.appearance_sub.is_none() {
            self.appearance_sub = Some(window.observe_window_appearance(|_, cx| {
                if crate::settings::Settings::load().theme_pref()
                    == crate::settings::ThemePref::System
                {
                    crate::settings::apply_theme_pref(crate::settings::ThemePref::System, cx);
                }
            }));
        }
        self.ensure_draft_for_active(window, cx);
        let slot_status = self.slot_status(cx);
        let state = self.slot().state.clone();
        let live = matches!(state.read(cx).conn, Conn::Live | Conn::Reconnecting { .. });

        // The find bar's matches track its query and the transcript. While a
        // stream is running the text changes every frame, so rescan at a
        // human pace rather than on every redraw.
        if let Some(find) = &mut self.slot_mut().find {
            let query = find.field.read(cx).text().trim().to_string();
            let s = state.read(cx);
            let count = s.transcript.len();
            let streaming = s.streaming;
            let now = std::time::Instant::now();
            let query_changed = query != find.query;
            let due = query_changed
                || count != find.scanned_count
                || streaming != find.scanned_streaming
                || (streaming
                    && now.duration_since(find.scanned_at)
                        >= std::time::Duration::from_millis(250));
            if due {
                if query_changed {
                    find.current = 0;
                }
                find.query = query.clone();
                find.matches = crate::state::find_matches(s.transcript.as_slice(), &query);
                find.scanned_count = count;
                find.scanned_streaming = streaming;
                find.scanned_at = now;
                find.current = if find.matches.is_empty() {
                    0
                } else {
                    find.current.min(find.matches.len() - 1)
                };
            }
        }

        if live && !self.slot().composer_focused {
            if let Some(draft) = &self.slot().draft {
                let focus = draft.read(cx).focus().clone();
                window.focus(&focus, cx);
            }
            self.slot_mut().composer_focused = true;
        } else if !live {
            self.slot_mut().composer_focused = false;
        }

        let main: gpui::AnyElement =
            if live {
                div()
                    .flex_1()
                    .min_w_0()
                    .h_full()
                    .min_h_0()
                    .flex()
                    .flex_col()
                    .child(
                        div()
                            .flex_none()
                            .child(chat::render_header(&state, &cx.entity(), cx)),
                    )
                    .child(div().flex_1().min_h_0().overflow_hidden().child(
                        chat::render_transcript(&state, &cx.entity(), self, window, cx),
                    ))
                    .child(
                        // The composer sits in the same centered column as the
                        // transcript, so the field lines up with the messages.
                        div()
                            .flex_none()
                            .w_full()
                            .flex()
                            .justify_center()
                            .px_4()
                            .pt_2()
                            .pb_2()
                            .child(
                                div()
                                    .w_full()
                                    .max_w(column)
                                    .min_w_0()
                                    .child(composer::render(&state, self, cx)),
                            ),
                    )
                    .into_any_element()
            } else {
                div()
                    .flex_1()
                    .min_w_0()
                    .h_full()
                    .min_h_0()
                    .child(welcome::render(&state, self, window, cx))
                    .into_any_element()
            };

        // Extension and rename dialogs ride at the root, above every pane;
        // the remove-project confirmation joins them, then the settings
        // window, then the palette.
        let overlay = self
            .extension_dialog_overlay(&state, window, cx)
            .or_else(|| self.rename_dialog(cx))
            .or_else(|| self.remove_project_dialog(cx))
            .or_else(|| self.confirm_delete_dialog(cx))
            .or_else(|| self.logout_provider_dialog(cx))
            .or_else(|| {
                let view = cx.entity();
                settings::settings_overlay(self, &view, window, cx)
            })
            .or_else(|| self.palette_overlay(cx));
        // The right panel mounts into the split when open; closing drops the
        // pane, so the two-pane layout gets the full width back.
        let right = self
            .right_open
            .then(|| crate::ui::panel::render(self, &cx.entity(), &state, cx).into_any_element());

        // The FocusScope must carry the window size itself: a percentage
        // height inside an unsized root resolves against auto, so the whole
        // tree would otherwise grow to content height and push the composer
        // below the window.
        FocusScope::new(&self.focus)
            .root()
            .size_full()
            .flex()
            .flex_row()
            .child(
                div()
                    .id("pido-root")
                    .size_full()
                    .min_w_0()
                    .min_h_0()
                    .relative()
                    .flex()
                    .flex_row()
                    .bg(bg)
                    .text_color(fg)
                    // Three panes when the right panel is out, two when it
                    // is tucked in; the sizes come from the clamped widths.
                    .child({
                        let width = f32::from(window.bounds().size.width);
                        let mut sizes = self.pane_sizes(width);
                        // While a drag is out of range, alternate the seed by
                        // a hair each render: use_seeded only replants when
                        // the seed differs from its stored one, and the drag
                        // overwrites the shares after every event — without
                        // the alternation the first clamp would be the last.
                        if let Some(since) = self.snap_since.get()
                            && since.elapsed() < std::time::Duration::from_millis(300)
                        {
                            let flip = self.snap_flip.get();
                            let last = sizes.len() - 1;
                            sizes[last] += if flip.is_multiple_of(2) { 1e-4 } else { -1e-4 };
                        }

                        let left_cell = self.left_px.clone();
                        let right_cell = self.right_px.clone();
                        let snap_since = self.snap_since.clone();
                        let snap_flip = self.snap_flip.clone();
                        let pane_save_at = self.pane_save_at.clone();
                        // Dragging past a limit clamps the stored width, but
                        // the split only reseeds from it on the next render —
                        // so the handler nudges one when a drag left the range.
                        let me = cx.entity().downgrade();
                        SplitPane::new("shell", gpui::Axis::Horizontal, gpui::px(200.))
                            .sizes(&sizes)
                            .pane(sidebar::render(
                                &state,
                                &cx.entity(),
                                self.search_field.clone(),
                                &slot_status,
                                window,
                                cx,
                            ))
                            .pane(main)
                            .children(right)
                            .on_resize(move |shares, _, cx| {
                                // Shares are fractions of the window; each pane
                                // keeps its clamped width.
                                let raw_left = shares[0] * width;
                                let left = clamp_width(raw_left, &SIDEBAR_RANGE);
                                left_cell.set(left);
                                let mut drifted = (raw_left - left).abs() > 0.5;
                                if let Some(share) = shares.get(2) {
                                    let raw_right = share * width;
                                    let right = clamp_width(raw_right, &RIGHT_RANGE);
                                    drifted |= (raw_right - right).abs() > 0.5;
                                    right_cell.set(right);
                                }
                                if drifted {
                                    snap_since.set(Some(std::time::Instant::now()));
                                    snap_flip.set(snap_flip.get().wrapping_add(1));
                                }
                                let (left, right) = (left_cell.get(), right_cell.get());
                                // Dragging emits an event per frame; save at
                                // most a few times a second. The quit handler
                                // stores the final widths.
                                let now = std::time::Instant::now();
                                let due = pane_save_at.get().is_none_or(|last| {
                                    now.duration_since(last)
                                        >= std::time::Duration::from_millis(250)
                                });
                                if due {
                                    pane_save_at.set(Some(now));
                                    crate::settings::save_setting(|s| {
                                        s.sidebar_width = Some(left);
                                        if shares.len() > 2 {
                                            s.right_width = Some(right);
                                        }
                                    });
                                }
                                if drifted && let Err(error) = me.update(cx, |_, cx| cx.notify()) {
                                    log::debug!("split snap-back lost: {error}");
                                }
                            })
                    })
                    .children(overlay)
                    // PromptInput gates its Enter/send on text being
                    // non-empty; when only images are staged, send here so an
                    // image-only message is possible.
                    .capture_action(cx.listener(|this, _: &Enter, window, cx| {
                        let state = this.slot().state.read(cx);
                        if state.pending_images.is_empty() {
                            return;
                        }
                        let Some(draft) = this.slot().draft.clone() else {
                            return;
                        };
                        if !draft.read(cx).focus().contains_focused(window, cx) {
                            return;
                        }
                        if !draft.read(cx).text().trim().is_empty() {
                            return;
                        }
                        this.slot()
                            .state
                            .update(cx, |s, cx| s.send_text(String::new(), cx));
                        cx.stop_propagation();
                    }))
                    // Esc stops a running agent (clear queue + abort, the
                    // protocol's recommended Esc semantics). TextInput does not
                    // consume Escape, so this also works while the composer is
                    // focused.
                    .on_action(cx.listener(|this, _: &Stop, window, cx| {
                        // A dialog owns Escape: cancel it instead of also
                        // stopping the run underneath.
                        if this.slot().rename_open {
                            this.close_rename(cx);
                            return;
                        }
                        if this.confirm_remove.is_some() {
                            this.confirm_remove = None;
                            cx.notify();
                            return;
                        }
                        if this.confirm_delete.is_some() {
                            this.confirm_delete = None;
                            cx.notify();
                            return;
                        }
                        if this.confirm_logout.is_some() {
                            this.confirm_logout = None;
                            cx.notify();
                            return;
                        }
                        if let Some(dialog) = this.slot().state.read(cx).pending_dialog.clone() {
                            this.slot()
                                .state
                                .update(cx, |s, cx| s.cancel_dialog(&dialog.id, cx));
                            return;
                        }
                        if this.settings_open.is_some() {
                            this.close_settings(cx);
                            return;
                        }
                        // In the terminal, Escape is the \x1b key the shell
                        // expects. The global action dispatched first and
                        // consumed the keystroke, so the terminal's own
                        // handler never runs — send the byte here instead.
                        let terminal_focused =
                            this.slot().terminal.as_ref().is_some_and(|terminal| {
                                terminal
                                    .read(cx)
                                    .focus_handle(cx)
                                    .contains_focused(window, cx)
                            });
                        if terminal_focused {
                            if let Some(terminal) = &this.slot().terminal {
                                terminal.update(cx, |terminal, _| terminal.write(b"\x1b".to_vec()));
                            }
                            return;
                        }
                        // Esc backs out of the palette, the find bar and the
                        // right panel before it stops a run.
                        if this.slot().palette.is_some() {
                            this.close_palette(window, cx);
                            return;
                        }
                        if this.slot().find.is_some() {
                            this.close_find(window, cx);
                            return;
                        }
                        if this.right_open {
                            this.close_right(cx);
                            return;
                        }
                        if this.slot().state.read(cx).streaming {
                            this.slot().state.update(cx, |s, cx| s.stop(cx));
                        }
                    }))
                    .on_action(cx.listener(|this, _: &Find, window, cx| {
                        this.toggle_find(window, cx);
                    }))
                    .on_action(cx.listener(|this, _: &Palette, window, cx| {
                        if this.slot().palette.is_some() {
                            this.close_palette(window, cx);
                        } else {
                            this.open_palette(window, cx);
                        }
                    }))
                    .on_action(cx.listener(|this, _: &PinSession, _window, cx| {
                        if this.modal_open(cx) {
                            return;
                        }
                        if let Some(file) = this.slot().state.read(cx).session_file.clone() {
                            this.slot()
                                .state
                                .update(cx, |s, cx| s.toggle_pin(&file, cx));
                        }
                    }))
                    .on_action(cx.listener(|this, _: &RenameSession, window, cx| {
                        if this.modal_open(cx) {
                            return;
                        }
                        if this.slot().state.read(cx).conn == Conn::Live {
                            this.open_rename(window, cx);
                        }
                    }))
                    .on_action(cx.listener(|this, _: &ForkSession, _window, cx| {
                        if this.modal_open(cx) {
                            return;
                        }
                        if let Some(file) = this.slot().state.read(cx).session_file.clone() {
                            this.fork_session(file, cx);
                        }
                    }))
                    .on_action(cx.listener(|this, _: &ArchiveSession, _window, cx| {
                        if this.modal_open(cx) {
                            return;
                        }
                        if let Some(file) = this.slot().state.read(cx).session_file.clone() {
                            this.slot()
                                .state
                                .update(cx, |s, cx| s.toggle_archive(&file, cx));
                        }
                    })),
            )
    }
}

impl PidoApp {
    /// The split's sizes, in shares of the window, from the clamped widths.
    fn pane_sizes(&self, window_width: f32) -> Vec<f32> {
        let width = window_width.max(1.0);
        let mut left = self.left_px.get();
        if self.right_open {
            let mut right = self.right_px.get();
            // Never let the side panes eat the chat column: the right panel
            // gives width back first, then the sidebar.
            const MIN_CHAT: f32 = 360.0;
            let over = left + right + MIN_CHAT - width;
            if over > 0.0 {
                let right_slack = (right - *RIGHT_RANGE.start()).max(0.0);
                let give = over.min(right_slack);
                right -= give;
                let remaining = over - give;
                if remaining > 0.0 {
                    left = (left - remaining).max(*SIDEBAR_RANGE.start());
                }
            }
            let (left, right) = (left / width, right / width);
            vec![left, (1.0 - left - right).max(0.0), right]
        } else {
            let left = (left / width).min(1.0);
            vec![left, (1.0 - left).max(0.0)]
        }
    }

    /// Open the right panel on `tab`; pressing the active tab's entry again
    /// closes the panel, so the buttons double as toggles.
    pub fn open_right(&mut self, tab: RightTab, cx: &mut Context<Self>) {
        if self.right_open && self.right_tab == tab {
            self.close_right(cx);
            return;
        }
        self.right_open = true;
        self.right_tab = tab;
        match tab {
            RightTab::Diff => {
                // Reopen on the newest file rather than a stale pick.
                self.slot_mut().diff_selected = None;
            }
            RightTab::Terminal => {
                self.ensure_terminal(cx);
            }
            RightTab::Tree => {
                // Fresh every open: the tree is one cheap request, and a
                // stale one would misrender the branches.
                self.slot()
                    .state
                    .update(cx, |state, cx| state.refresh_session_tree(cx));
            }
        }
        crate::settings::save_setting(|s| {
            s.right_open = Some(true);
        });
        cx.notify();
    }

    /// Close the right panel; the terminal keeps running in the background.
    pub fn close_right(&mut self, cx: &mut Context<Self>) {
        self.right_open = false;
        crate::settings::save_setting(|s| {
            s.right_open = Some(false);
        });
        cx.notify();
    }

    /// Open the transcript find bar, or refocus it when already open.
    pub fn toggle_find(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.modal_open(cx) {
            return;
        }
        if let Some(find) = &self.slot().find {
            let focus = find.field.read(cx).focus().clone();
            window.focus(&focus, cx);
            cx.notify();
            return;
        }
        if self.slot().state.read(cx).transcript.is_empty() {
            return;
        }
        let field = cx.new(|cx| TextInput::new(window, cx).placeholder_key("find_placeholder"));
        cx.subscribe(&field, move |this, _, event: &InputEvent, cx| {
            if *event == InputEvent::Submit {
                this.find_step(true, cx);
            }
        })
        .detach();
        let focus = field.read(cx).focus().clone();
        window.focus(&focus, cx);
        self.slot_mut().find = Some(FindState {
            field,
            query: String::new(),
            matches: Vec::new(),
            current: 0,
            scanned_count: 0,
            scanned_streaming: false,
            scanned_at: std::time::Instant::now(),
        });
        cx.notify();
    }

    pub fn close_find(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.slot_mut().find.take().is_some() {
            self.focus_composer(window, cx);
            cx.notify();
        }
    }

    /// Put focus back in the composer after a transient overlay closes.
    fn focus_composer(&self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(draft) = &self.slot().draft {
            let focus = draft.read(cx).focus().clone();
            window.focus(&focus, cx);
        }
    }

    /// Step to the next (or previous) match and reveal its row.
    pub fn find_step(&mut self, forward: bool, cx: &mut Context<Self>) {
        let Some(find) = &mut self.slot_mut().find else {
            return;
        };
        if find.matches.is_empty() {
            return;
        }
        let len = find.matches.len();
        find.current = if forward {
            (find.current + 1) % len
        } else {
            (find.current + len - 1) % len
        };
        let ix = find.matches[find.current];
        // The index was computed by an earlier frame. If the transcript
        // shrank (a failed send removed a bubble, say), drop the search
        // instead of slicing out of bounds.
        let len = self.slot().state.read(cx).transcript.len();
        if ix >= len {
            self.slot_mut().find = None;
            cx.notify();
            return;
        }
        // A match can live inside a folded turn -a tool result, most often.
        // Unfold that turn so the row exists to scroll to and tint.
        let turn_ix = self.slot().state.read(cx).transcript[..ix]
            .iter()
            .rposition(|item| matches!(item, TranscriptItem::Turn { .. }));
        if let Some(turn_ix) = turn_ix {
            self.slot().state.update(cx, |s, _| {
                s.expanded_turns.insert(turn_ix as u64);
            });
        }
        let visible = self.slot().state.read(cx).visible_rows();
        // The unfold grew the list: sync its item count now, or the scroll
        // below computes its anchor against the stale, shorter list.
        if let Some(list) = &self.slot().transcript_list {
            let feed_count = list.item_count();
            if visible.len() > feed_count {
                list.splice(feed_count..feed_count, visible.len() - feed_count);
            } else if visible.len() < feed_count {
                list.splice(visible.len()..feed_count, 0);
            }
        }
        let pos = visible.iter().position(|&row| row == ix);
        if let (Some(list), Some(pos)) = (&self.slot().transcript_list, pos) {
            // Rows far from the viewport were never painted, so their cached
            // heights are estimates; remeasure before seeking or the jump
            // lands on a garbage offset.
            list.remeasure();
            list.scroll_to_reveal_item(pos);
        }
        cx.notify();
    }

    /// Close the command palette.
    pub fn close_palette(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.slot_mut().palette.take().is_some() {
            self.focus_composer(window, cx);
            cx.notify();
        }
    }

    /// Open the command palette (Ctrl+K); it closes via the scrim, a pick,
    /// or Ctrl+K again.
    pub fn open_palette(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.modal_open(cx) {
            return;
        }
        log::info!("open_palette: enter");
        self.slot_mut().palette_gen = self.slot().palette_gen.wrapping_add(1);
        self.slot_mut().palette_selected = 0;
        let field = cx.new(|cx| TextInput::new(window, cx).placeholder_key("palette_placeholder"));
        let focus = field.read(cx).focus().clone();
        window.focus(&focus, cx);
        self.slot_mut().palette = Some(PaletteState {
            field,
            open_gen: self.slot().palette_gen,
        });
        cx.notify();
    }

    /// The floating command palette: a query over sessions, models, thinking
    /// levels, pi commands and app actions, with the picker's own keyboard
    /// handling (arrows, Enter, Escape).
    fn palette_overlay(&mut self, cx: &mut Context<Self>) -> Option<gpui::AnyElement> {
        use ely_gpui_component::forms::{Choice, Down, Enter, Up, marked_row};
        use ely_gpui_component::primitives::IconName;

        let palette = self.slot().palette.as_ref()?;
        let field = palette.field.clone();
        let gen_id = palette.open_gen;
        let query = field.read(cx).text().to_string();
        let theme = cx.theme();

        let actions: Vec<(String, PaletteAction)> = vec![
            (i18n::t(cx, "new_chat").to_string(), PaletteAction::NewChat),
            (
                i18n::t(cx, "export_html").to_string(),
                PaletteAction::ExportHtml,
            ),
            (
                i18n::t(cx, "compact_now").to_string(),
                PaletteAction::Compact,
            ),
            (i18n::t(cx, "theme").to_string(), PaletteAction::ToggleTheme),
            (
                i18n::t(cx, "shell_template").to_string(),
                PaletteAction::ShellTemplate,
            ),
            (
                i18n::t(cx, "check_updates").to_string(),
                PaletteAction::CheckUpdate,
            ),
        ];
        let state = self.slot().state.clone();
        let files = state.read(cx).workspace_files.clone();
        let items = state.read(cx).palette_items(&query, &files, &actions);
        let kind_label = |kind: PaletteKind| -> gpui::SharedString {
            i18n::t(
                cx,
                match kind {
                    PaletteKind::Command => "kind_command",
                    PaletteKind::Model => "kind_model",
                    PaletteKind::Thinking => "kind_thinking",
                    PaletteKind::Session => "kind_session",
                    PaletteKind::Action => "kind_action",
                    PaletteKind::File => "kind_file",
                },
            )
        };
        let kind_icon = |kind: PaletteKind| -> IconName {
            match kind {
                PaletteKind::Command => IconName::Terminal,
                PaletteKind::Model => IconName::Cpu,
                PaletteKind::Thinking => IconName::Lightbulb,
                PaletteKind::Session => IconName::MessageSquareDiff,
                PaletteKind::Action => IconName::Zap,
                PaletteKind::File => IconName::File,
            }
        };

        // The highlighted row, clamped to the filtered list.
        let selected = if items.is_empty() {
            0
        } else {
            self.slot().palette_selected.min(items.len() - 1)
        };

        let view = cx.entity();
        let run_item = move |this: &mut PidoApp,
                             item: &PaletteItem,
                             window: &mut Window,
                             cx: &mut Context<PidoApp>| {
            let is_command = matches!(item.action, PaletteAction::RunCommand(_));
            let composer_focus = if is_command {
                this.slot()
                    .draft
                    .as_ref()
                    .map(|draft| draft.read(cx).focus().clone())
            } else {
                None
            };
            this.close_palette(window, cx);
            match &item.action {
                PaletteAction::SwitchSession(path) => {
                    this.open_session(path.to_string_lossy().into_owned(), cx)
                }
                PaletteAction::SetModel { provider, model_id } => this
                    .slot()
                    .state
                    .update(cx, |s, cx| s.set_model(provider, model_id, cx)),
                PaletteAction::SetThinking(level) => this
                    .slot()
                    .state
                    .update(cx, |s, cx| s.set_thinking(*level, cx)),
                PaletteAction::RunCommand(command) => {
                    if let Some(draft) = &this.slot().draft {
                        draft.update(cx, |field, cx| field.set_text(format!("{command} "), cx));
                    }
                }
                PaletteAction::ShellTemplate => {
                    if let Some(draft) = &this.slot().draft {
                        draft.update(cx, |field, cx| field.set_text("!", cx));
                    }
                }
                PaletteAction::CheckUpdate => {
                    this.check_for_updates(true, cx);
                }
                PaletteAction::InsertFile(file) => {
                    if let Some(draft) = &this.slot().draft {
                        let at = format!("@{file} ");
                        draft.update(cx, |field, cx| field.insert(&at, cx));
                    }
                }
                PaletteAction::NewChat => this.new_chat(cx),
                PaletteAction::ExportHtml => {
                    this.slot().state.update(cx, |s, cx| s.export_html(cx));
                }
                PaletteAction::Compact => {
                    this.slot().state.update(cx, |s, cx| s.compact_now(cx));
                }
                PaletteAction::ToggleTheme => toggle_theme(cx),
            }
            // A picked command lands in the composer for editing; pi expands
            // it on send.
            if let (Some(draft), Some(focus)) = (&this.slot().draft, composer_focus) {
                window.focus(&draft.read(cx).focus().clone(), cx);
                let _ = focus;
            }
        };

        // Rows: the picker's own row styling, with a click to run.
        let mut rows: Vec<gpui::AnyElement> = Vec::new();
        if items.is_empty() {
            rows.push(
                div()
                    .px_3()
                    .py_3()
                    .text_color(theme.colors.fg_subtle)
                    .child(i18n::t(cx, "find_none"))
                    .into_any_element(),
            );
        }
        for (ix, item) in items.iter().enumerate() {
            let row_view = view.clone();
            let row_item = item.clone();
            let choice = Choice::new(format!("{gen_id}:{ix}"), item.label.clone())
                .icon(kind_icon(item.kind))
                .note(kind_label(item.kind));
            rows.push(
                marked_row(
                    format!("palette-row-{gen_id}-{ix}"),
                    &choice,
                    None,
                    ix == selected,
                    None,
                    cx,
                )
                .on_click(move |_, window, cx| {
                    row_view.update(cx, |this, cx| {
                        let item = row_item.clone();
                        this.close_palette(window, cx);
                        run_item(this, &item, window, cx);
                    });
                })
                .into_any_element(),
            );
        }

        let items_run = items.clone();
        let items_len = items.len();
        Some(
            div()
                .id("palette-overlay")
                .absolute()
                .inset_0()
                .occlude()
                .bg(gpui::rgba(0x00000059))
                .on_mouse_down(gpui::MouseButton::Left, {
                    let view = view.downgrade();
                    move |_, window, cx| {
                        view.update(cx, |this, cx| this.close_palette(window, cx))
                            .ok();
                    }
                })
                .child(
                    div()
                        .absolute()
                        .top_10()
                        .left_0()
                        .right_0()
                        .flex()
                        .justify_center()
                        // Clicks on the card must not fall through to the
                        // scrim's close.
                        .child(
                            div()
                                .id("palette-card")
                                .w(gpui::px(640.))
                                .flex()
                                .flex_col()
                                .rounded_lg()
                                .border_1()
                                .border_color(theme.colors.border)
                                .bg(theme.colors.surface)
                                .shadow_lg()
                                .overflow_hidden()
                                .on_mouse_down(gpui::MouseButton::Left, |_, _, cx| {
                                    cx.stop_propagation()
                                })
                                // Arrows and Enter steer the palette before
                                // they reach the input's own bindings.
                                .capture_action({
                                    let view = view.clone();
                                    let items = items.clone();
                                    move |_: &Up, _, cx| {
                                        cx.stop_propagation();
                                        view.update(cx, |this, cx| {
                                            if this.slot().palette_selected > 0 {
                                                this.slot_mut().palette_selected -= 1;
                                                cx.notify();
                                            }
                                        });
                                        let _ = (&items, items_len);
                                    }
                                })
                                .capture_action({
                                    let view = view.clone();
                                    move |_: &Down, _, cx| {
                                        cx.stop_propagation();
                                        view.update(cx, |this, cx| {
                                            if this.slot().palette_selected + 1 < items_len {
                                                this.slot_mut().palette_selected += 1;
                                                cx.notify();
                                            }
                                        });
                                    }
                                })
                                .capture_action({
                                    let view = view.clone();
                                    let items = items_run.clone();
                                    move |_: &Enter, window, cx| {
                                        cx.stop_propagation();
                                        view.update(cx, |this, cx| {
                                            let Some(item) =
                                                items.get(this.slot().palette_selected)
                                            else {
                                                return;
                                            };
                                            let item = item.clone();
                                            run_item(this, &item, window, cx);
                                        });
                                    }
                                })
                                .child(
                                    div()
                                        .flex_none()
                                        .px_3()
                                        .py_2()
                                        .border_b_1()
                                        .border_color(theme.colors.border)
                                        .child(field),
                                )
                                .child(
                                    div()
                                        .id("palette-list")
                                        .flex_1()
                                        .min_h_0()
                                        .max_h(gpui::px(400.))
                                        .overflow_y_scroll()
                                        .flex()
                                        .flex_col()
                                        .children(rows),
                                ),
                        ),
                )
                .into_any_element(),
        )
    }

    /// The panel's shell, spawned on first use; a dead one spawns again.
    fn ensure_terminal(&mut self, cx: &mut Context<Self>) {
        let ix = self.active;
        self.ensure_terminal_for(ix, cx);
    }

    fn ensure_terminal_for(&mut self, ix: usize, cx: &mut Context<Self>) {
        use ely_gpui_component::terminal::{Launch, Terminal};
        if self.slots[ix].terminal.is_some() {
            return;
        }
        // PowerShell on Windows; the user's login shell elsewhere.
        let program = if cfg!(windows) {
            Some(("powershell.exe".to_string(), Vec::new()))
        } else {
            Some((
                std::env::var("SHELL").unwrap_or_else(|_| "/bin/sh".to_string()),
                Vec::new(),
            ))
        };
        let cwd = self.slots[ix]
            .state
            .read(cx)
            .cwd
            .clone()
            .or_else(dirs::home_dir);
        let launch = Launch {
            program,
            cwd,
            env: Vec::new(),
        };
        let spawned = cx.new(|cx| {
            match Terminal::spawn(launch, cx) {
                Ok(terminal) => terminal,
                Err(error) => {
                    // A deleted cwd or unavailable ConPTY must not panic the
                    // whole app. Show the failure in a read-only terminal.
                    log::warn!("terminal failed to start: {error}");
                    let message = format!(
                        "{}\r\n",
                        crate::i18n::t_arg(cx, "terminal_failed", "e", &error.to_string())
                    );
                    Terminal::replay(message.as_bytes(), 80, 24, cx)
                }
            }
        });
        let id = self.slots[ix].id;
        cx.subscribe(
            &spawned,
            move |this, _, event: &ely_gpui_component::terminal::TerminalEvent, cx| {
                if let ely_gpui_component::terminal::TerminalEvent::Exited(_) = event {
                    // The shell is gone; the next open spawns a fresh one.
                    if let Some(slot) = this.slots.iter_mut().find(|slot| slot.id == id) {
                        slot.terminal = None;
                    }
                    cx.notify();
                }
            },
        )
        .detach();
        self.slots[ix].terminal = Some(spawned);
    }

    /// Spawn a fresh shell in the session's directory, dropping the old one.
    pub fn restart_terminal(&mut self, cx: &mut Context<Self>) {
        let ix = self.active;
        self.restart_terminal_for(ix, cx);
    }

    fn restart_terminal_for(&mut self, ix: usize, cx: &mut Context<Self>) {
        self.slots[ix].terminal = None;
        self.ensure_terminal_for(ix, cx);
        cx.notify();
    }

    /// Open the rename dialog over a project's sidebar label.
    pub fn open_project_rename(
        &mut self,
        cwd: String,
        label: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.rename_project = Some(cwd);
        self.open_rename_with(None, label, window, cx);
    }

    /// Ask before dropping a project from the sidebar; its sessions stay on
    /// disk either way.
    pub fn confirm_remove_project(&mut self, cwd: String, cx: &mut Context<Self>) {
        self.confirm_remove = Some(cwd);
        cx.notify();
    }

    /// Ask before deleting a session outright. Carries the display title so
    /// the dialog can name it without re-reading the rows.
    pub fn confirm_delete_session(&mut self, path: String, cx: &mut Context<Self>) {
        let title = self
            .slot()
            .state
            .read(cx)
            .sidebar_rows
            .iter()
            .find_map(|row| match row {
                crate::state::SidebarRow::Session {
                    path: row_path,
                    title,
                    ..
                } if *row_path == path => Some(title.clone()),
                _ => None,
            })
            .unwrap_or_else(|| path.clone().into());
        self.confirm_delete = Some((path, title.to_string()));
        cx.notify();
    }

    /// The delete-session confirmation, while one is pending.
    fn confirm_delete_dialog(&mut self, cx: &mut Context<Self>) -> Option<gpui::AnyElement> {
        use ely_gpui_component::overlays::ConfirmDialog;

        let (path, title) = self.confirm_delete.clone()?;
        let me = cx.entity();
        let message = crate::i18n::t_arg(cx, "delete_session_desc", "name", &title).to_string();
        let shut_me = me.clone();
        let confirm_me = me.clone();
        let shut = move |_: &mut Window, cx: &mut App| {
            shut_me.update(cx, |this, cx| {
                this.confirm_delete = None;
                cx.notify();
            });
        };
        // Captured rather than re-read: the dialog closes (clearing the
        // field) before `on_confirm` runs.
        let confirm_path = path;
        Some(
            ConfirmDialog::new(
                "delete-session-dialog",
                crate::i18n::t(cx, "delete_session"),
                message,
                shut,
            )
            .confirm(crate::i18n::t(cx, "remove_confirm"))
            .destructive()
            .on_confirm(move |_, cx| {
                confirm_me.update(cx, |this, cx| this.delete_session(confirm_path.clone(), cx));
            })
            .into_any_element(),
        )
    }

    /// Delete a session for real: close any slot holding it first (a pi that
    /// still appends would keep the file alive on Windows), purge it from
    /// the pin/archive sets, then unlink the file in the background.
    pub fn delete_session(&mut self, path: String, cx: &mut Context<Self>) {
        self.confirm_delete = None;
        let key = crate::state::session_key(&path);
        if let Some(ix) = self.slot_index_of_key(&key, cx) {
            self.close_slot(ix, cx);
        }
        crate::settings::save_setting(|settings| {
            settings
                .pinned_sessions
                .retain(|existing| crate::state::session_key(existing) != key);
            settings
                .archived_sessions
                .retain(|existing| crate::state::session_key(existing) != key);
        });
        let state = self.slot().state.clone();
        state.update(cx, |s, _| {
            s.pinned_sessions.remove(&key);
            s.archived_sessions.remove(&key);
        });
        cx.spawn(async move |this, cx| {
            let removed = cx
                .background_executor()
                .spawn(async move { std::fs::remove_file(&path) })
                .await;
            this.update(cx, |_, cx| {
                state.update(cx, |s, cx| match removed {
                    Ok(()) => s.refresh_sessions(cx),
                    Err(error) => {
                        let text = crate::i18n::t_arg(
                            cx,
                            "session_delete_failed",
                            "e",
                            &error.to_string(),
                        )
                        .to_string();
                        s.push_notice(crate::state::NoticeKind::Error, text);
                    }
                });
            })
            .ok();
        })
        .detach();
        cx.notify();
    }

    /// The remove-project confirmation, while one is pending.
    fn remove_project_dialog(&mut self, cx: &mut Context<Self>) -> Option<gpui::AnyElement> {
        use ely_gpui_component::overlays::ConfirmDialog;

        let cwd = self.confirm_remove.clone()?;
        let me = cx.entity();
        let label = self
            .slot()
            .state
            .read(cx)
            .project_labels
            .get(&crate::state::session_key(&cwd))
            .cloned()
            .unwrap_or_else(|| crate::state::shorten_project_display(&cwd));
        let message = crate::i18n::t_arg(cx, "remove_project_desc", "name", &label).to_string();
        let shut_me = me.clone();
        let confirm_me = me.clone();
        let shut = move |_: &mut Window, cx: &mut App| {
            shut_me.update(cx, |this, cx| {
                this.confirm_remove = None;
                cx.notify();
            });
        };
        // The label is captured rather than re-read from `confirm_remove`:
        // ConfirmDialog closes (and runs `on_close`, which clears the field)
        // before `on_confirm`, so reading it there would yield an empty path.
        let confirm_cwd = cwd.clone();
        Some(
            ConfirmDialog::new(
                "remove-project-dialog",
                crate::i18n::t(cx, "remove_project"),
                message,
                shut,
            )
            .confirm(crate::i18n::t(cx, "remove_confirm"))
            .destructive()
            .on_confirm(move |_, cx| {
                confirm_me.update(cx, |this, cx| {
                    this.slot()
                        .state
                        .update(cx, |s, cx| s.remove_project(&confirm_cwd, cx));
                    cx.notify();
                });
            })
            .into_any_element(),
        )
    }

    /// Open the rename dialog over the current session's name.
    pub fn open_rename(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let (target, current) = {
            let state = self.slot().state.read(cx);
            (
                state.session_file.clone(),
                state.session_name.clone().unwrap_or_default(),
            )
        };
        self.open_rename_with(target, current, window, cx);
    }

    /// Open the rename dialog for a session picked in the sidebar: its own
    /// name prefills the field, and saving renames that session rather than
    /// whichever one is open.
    pub fn open_rename_at(
        &mut self,
        path: String,
        title: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let prefill = {
            let state = self.slot().state.read(cx);
            let open = state.session_file.as_deref().is_some_and(|current| {
                crate::state::session_key(current) == crate::state::session_key(&path)
            });
            if open {
                state.session_name.clone().unwrap_or_default()
            } else {
                title
            }
        };
        self.open_rename_with(Some(path), prefill, window, cx);
    }

    fn open_rename_with(
        &mut self,
        target: Option<String>,
        prefill: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let field = cx.new(|cx| {
            let mut input = TextInput::new(window, cx);
            if !prefill.is_empty() {
                input.set_text(prefill, cx);
            }
            input
        });
        self.slot_mut().rename_field = Some(field.clone());
        self.slot_mut().rename_open = true;
        self.slot_mut().rename_target = target;
        // Enter in the field saves, the same path the Save button takes.
        cx.subscribe(&field, move |this, field, event: &InputEvent, cx| {
            if *event != InputEvent::Submit {
                return;
            }
            this.finish_rename(&field, cx);
        })
        .detach();
        cx.notify();
    }

    /// Commit the rename dialog's field, then close it. A project rename is
    /// a sidebar label; a session rename goes through pi or the file.
    fn finish_rename(&mut self, field: &gpui::Entity<TextInput>, cx: &mut Context<Self>) {
        let name = field.read(cx).text().to_string();
        if let Some(cwd) = self.rename_project.clone() {
            self.slot()
                .state
                .update(cx, |state, cx| state.rename_project(&cwd, &name, cx));
        } else {
            match self.slot().rename_target.clone() {
                Some(path) => self
                    .slot()
                    .state
                    .update(cx, |state, cx| state.rename_session_at(&path, name, cx)),
                None => self
                    .slot()
                    .state
                    .update(cx, |state, cx| state.rename_session(name, cx)),
            }
        }
        self.slot_mut().rename_open = false;
        self.slot_mut().rename_field = None;
        self.slot_mut().rename_target = None;
        self.rename_project = None;
        cx.notify();
    }

    /// The rename dialog, while open.
    /// Close the rename dialog and drop every target it carried. Both of
    /// the dialog's close paths go through this — a cancel button that
    /// misses a field would rename the wrong thing next time.
    fn close_rename(&mut self, cx: &mut Context<Self>) {
        self.slot_mut().rename_open = false;
        self.slot_mut().rename_field = None;
        self.slot_mut().rename_target = None;
        self.rename_project = None;
        cx.notify();
    }

    /// Whether a dialog owns the keyboard right now. Overlays that open from
    /// shortcuts must not stack themselves on top of one.
    fn modal_open(&self, cx: &App) -> bool {
        self.slot().rename_open
            || self.confirm_remove.is_some()
            || self.confirm_delete.is_some()
            || self.confirm_logout.is_some()
            || self.settings_open.is_some()
            || self.slot().state.read(cx).pending_dialog.is_some()
    }

    /// Open the settings window on `page`, snapshotting what it shows.
    /// A no-op while another modal owns the window.
    pub fn open_settings(
        &mut self,
        page: settings::SettingsPage,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.modal_open(cx) {
            return;
        }
        let ready = crate::state::AppState::shared_environment()
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .as_ref()
            .map(|environment| environment.ready_providers.clone())
            .unwrap_or_default();
        let project_cwd = self.slot().state.read(cx).cwd.clone();
        let snapshot = settings::SettingsSnapshot::load(project_cwd.as_deref(), ready);
        // Seed the edit fields from the snapshot; the save actions read them
        // back, so they must exist for as long as the window does.
        let provider = snapshot.setting_str("defaultProvider").unwrap_or_default();
        let model = snapshot.setting_str("defaultModel").unwrap_or_default();
        self.settings_provider_input = Some(cx.new(|cx| {
            let mut input = TextInput::new(window, cx);
            if !provider.is_empty() {
                input.set_text(&provider, cx);
            }
            input
        }));
        self.settings_model_input = Some(cx.new(|cx| {
            let mut input = TextInput::new(window, cx);
            if !model.is_empty() {
                input.set_text(&model, cx);
            }
            input
        }));
        self.settings_ext_input = Some(cx.new(|cx| TextInput::new(window, cx)));
        self.confirm_logout = None;
        self.settings_snapshot = Some(snapshot);
        self.settings_open = Some(page);
        // Take the keyboard: otherwise the composer behind the scrim keeps it
        // and typing (or Enter) lands in the chat draft.
        window.focus(&self.settings_focus, cx);
        cx.notify();
    }

    pub fn close_settings(&mut self, cx: &mut Context<Self>) {
        self.settings_open = None;
        self.settings_snapshot = None;
        self.settings_provider_input = None;
        self.settings_model_input = None;
        self.settings_ext_input = None;
        self.confirm_logout = None;
        // Hand the keyboard back to the composer.
        self.slot_mut().composer_focused = false;
        cx.notify();
    }

    /// Switch pages inside the open settings window.
    pub fn set_settings_page(&mut self, page: settings::SettingsPage, cx: &mut Context<Self>) {
        if self.settings_open != Some(page) {
            self.settings_open = Some(page);
            cx.notify();
        }
    }

    // -- settings window: pi config edits -----------------------------------
    //
    // pi has no RPC for its persisted configuration, so every action here
    // edits one of pi's files directly (atomically, unknown fields intact)
    // and patches the snapshot in place. pi reads these files at startup, so
    // running sessions pick changes up only after a restart — the pages say
    // so.

    /// A failed config write surfaces as a notice on the active slot.
    fn settings_error(&mut self, error: String, cx: &mut Context<Self>) {
        log::warn!("settings: {error}");
        let text = crate::i18n::t_arg(cx, "settings_write_failed", "e", &error).to_string();
        self.slot().state.update(cx, |state, _| {
            state.push_notice(crate::state::NoticeKind::Error, text)
        });
    }

    pub fn open_logout_confirm(&mut self, provider: String, cx: &mut Context<Self>) {
        self.confirm_logout = Some(provider);
        cx.notify();
    }

    /// Persist the provider/model fields next to pi's own defaults.
    pub fn save_pi_defaults(&mut self, cx: &mut Context<Self>) {
        let provider = self
            .settings_provider_input
            .as_ref()
            .map(|field| field.read(cx).text().trim().to_string())
            .unwrap_or_default();
        let model = self
            .settings_model_input
            .as_ref()
            .map(|field| field.read(cx).text().trim().to_string())
            .unwrap_or_default();
        if provider.is_empty() && model.is_empty() {
            return;
        }
        let Some(agent_dir) = self
            .settings_snapshot
            .as_ref()
            .and_then(|snapshot| snapshot.agent_dir.clone())
        else {
            return;
        };
        let path = pi_rpc::config::agent_settings_path(&agent_dir);
        let result = pi_rpc::config::write_json_atomic(&path, |value| {
            if !provider.is_empty() {
                value["defaultProvider"] = serde_json::Value::String(provider);
            }
            if !model.is_empty() {
                value["defaultModel"] = serde_json::Value::String(model);
            }
        });
        self.after_settings_write(result, cx);
    }

    /// Persist pi's startup thinking level.
    pub fn set_default_thinking(&mut self, level: String, cx: &mut Context<Self>) {
        let Some(agent_dir) = self
            .settings_snapshot
            .as_ref()
            .and_then(|snapshot| snapshot.agent_dir.clone())
        else {
            return;
        };
        let path = pi_rpc::config::agent_settings_path(&agent_dir);
        let result = pi_rpc::config::write_json_atomic(&path, |value| {
            value["defaultThinkingLevel"] = serde_json::Value::String(level);
        });
        self.after_settings_write(result, cx);
    }

    /// Toggle a skill on or off in the settings array of its own scope —
    /// a user skill flips in the global file, a project skill in `.pi/`.
    pub fn set_skill_enabled(
        &mut self,
        skill: pi_rpc::resources::SkillSummary,
        enabled: bool,
        cx: &mut Context<Self>,
    ) {
        let Some((path, base_dir)) = self.resource_target(skill.scope) else {
            return;
        };
        let Some(pattern) =
            pi_rpc::config::resource_pattern(&base_dir, &skill.path.join("SKILL.md"))
        else {
            return;
        };
        let result = pi_rpc::config::write_json_atomic(&path, |value| {
            let mut list = pi_rpc::config::string_array(value, "skills");
            pi_rpc::config::toggle_resource(&mut list, &pattern, enabled);
            value["skills"] =
                serde_json::Value::Array(list.into_iter().map(serde_json::Value::String).collect());
        });
        match result {
            Ok(()) => {
                if let Some(snapshot) = self.settings_snapshot.as_mut() {
                    for existing in &mut snapshot.skills {
                        if existing.path == skill.path {
                            existing.enabled = enabled;
                        }
                    }
                }
                cx.notify();
            }
            Err(error) => self.settings_error(error, cx),
        }
    }

    /// Toggle a builtin extension. A project override entry already owns the
    /// decision, so the flip lands there; otherwise in the global file.
    pub fn set_builtin_extension_enabled(
        &mut self,
        id: &str,
        enabled: bool,
        cx: &mut Context<Self>,
    ) {
        let Some((current, project_owns)) = self.settings_snapshot.as_ref().map(|snapshot| {
            pi_rpc::config::builtin_extension_state(
                &snapshot.pi_settings,
                &snapshot.project_settings,
                id,
            )
        }) else {
            return;
        };
        if current == enabled {
            return;
        }
        let scope = if project_owns {
            pi_rpc::resources::Scope::Project
        } else {
            pi_rpc::resources::Scope::User
        };
        let Some(path) = self.resource_settings_path(scope) else {
            return;
        };
        let id = id.to_string();
        let result = pi_rpc::config::write_json_atomic(&path, |value| {
            let mut list = pi_rpc::config::string_array(value, "extensions");
            pi_rpc::config::toggle_resource(&mut list, &id, enabled);
            value["extensions"] =
                serde_json::Value::Array(list.into_iter().map(serde_json::Value::String).collect());
        });
        self.after_settings_write(result, cx);
    }

    /// Toggle a discovered extension file with an explicit override in its
    /// scope's settings array.
    pub fn set_extension_enabled(
        &mut self,
        entry: pi_rpc::resources::ExtensionEntry,
        enabled: bool,
        cx: &mut Context<Self>,
    ) {
        if entry.enabled == enabled {
            return;
        }
        let Some(path) = entry.path.clone() else {
            return;
        };
        let Some((settings_path, base_dir)) = self.resource_target(entry.scope) else {
            return;
        };
        let Some(pattern) = pi_rpc::config::resource_pattern(&base_dir, &path) else {
            return;
        };
        let result = pi_rpc::config::write_json_atomic(&settings_path, |value| {
            let mut list = pi_rpc::config::string_array(value, "extensions");
            pi_rpc::config::toggle_resource(&mut list, &pattern, enabled);
            value["extensions"] =
                serde_json::Value::Array(list.into_iter().map(serde_json::Value::String).collect());
        });
        match result {
            Ok(()) => {
                if let Some(snapshot) = self.settings_snapshot.as_mut() {
                    for existing in &mut snapshot.extensions {
                        if existing.source == entry.source
                            && existing.scope == entry.scope
                            && existing.channel == entry.channel
                        {
                            existing.enabled = enabled;
                        }
                    }
                }
                cx.notify();
            }
            Err(error) => self.settings_error(error, cx),
        }
    }

    /// Drop one configured entry (`extensions` array or `packages`) from its
    /// scope's settings file — pi's "remove" without uninstalling the copy.
    pub fn remove_extension(
        &mut self,
        entry: pi_rpc::resources::ExtensionEntry,
        cx: &mut Context<Self>,
    ) {
        let key = match entry.channel {
            pi_rpc::resources::ExtensionChannel::SettingArray => "extensions",
            pi_rpc::resources::ExtensionChannel::Package => "packages",
            pi_rpc::resources::ExtensionChannel::Discovered => return,
        };
        let Some(path) = self.resource_settings_path(entry.scope) else {
            return;
        };
        let result = pi_rpc::config::write_json_atomic(&path, |value| {
            pi_rpc::config::remove_list_entry(value, key, &entry.source);
        });
        match result {
            Ok(()) => {
                if let Some(snapshot) = self.settings_snapshot.as_mut() {
                    snapshot.extensions.retain(|existing| {
                        !(existing.source == entry.source
                            && existing.scope == entry.scope
                            && existing.channel == entry.channel)
                    });
                }
                cx.notify();
            }
            Err(error) => self.settings_error(error, cx),
        }
    }

    /// Add the source typed into the add-extension row: remote specs to
    /// `packages`, existing local paths to `extensions`, in the chosen scope.
    pub fn add_extension(&mut self, cx: &mut Context<Self>) {
        let source = self
            .settings_ext_input
            .as_ref()
            .map(|field| field.read(cx).text().trim().to_string())
            .unwrap_or_default();
        if source.is_empty() {
            return;
        }
        let remote = ["npm:", "git:", "https://", "http://"]
            .iter()
            .any(|prefix| source.starts_with(prefix));
        if !remote && !std::path::Path::new(&source).exists() {
            let text = crate::i18n::t(cx, "settings_ext_invalid").to_string();
            self.slot().state.update(cx, |state, _| {
                state.push_notice(crate::state::NoticeKind::Warn, text)
            });
            return;
        }
        let key = if remote { "packages" } else { "extensions" };
        let scope = self.ext_add_scope;
        let Some(path) = self.resource_settings_path(scope) else {
            return;
        };
        let result = pi_rpc::config::write_json_atomic(&path, |value| {
            pi_rpc::config::push_list_entry(value, key, &source);
        });
        match result {
            Ok(()) => {
                if let Some(snapshot) = self.settings_snapshot.as_mut() {
                    snapshot.extensions = pi_rpc::resources::pi_extensions(
                        snapshot.agent_dir.as_deref(),
                        snapshot.project_cwd.as_deref(),
                    );
                }
                if let Some(field) = self.settings_ext_input.clone() {
                    field.update(cx, |input, cx| input.set_text("", cx));
                }
                cx.notify();
            }
            Err(error) => self.settings_error(error, cx),
        }
    }

    /// Re-run the provider probes in the background and refresh the status
    /// dots; also updates the process-wide environment cache.
    pub fn refresh_providers(&mut self, cx: &mut Context<Self>) {
        cx.spawn(async move |this, cx| {
            let ready = cx
                .background_executor()
                .spawn(async { crate::state::probe_ready_providers_blocking() })
                .await;
            if let Ok(mut cached) = AppState::shared_environment().lock()
                && let Some(environment) = cached.as_mut()
            {
                environment.ready_providers = ready.clone();
            }
            this.update(cx, |this, cx| {
                if let Some(snapshot) = this.settings_snapshot.as_mut() {
                    snapshot.ready_providers = ready;
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    /// Sign out: delete the provider's entry from pi's auth.json. pi
    /// re-reads that file on every credential use, so this applies at once.
    pub fn logout_provider(&mut self, provider: String, cx: &mut Context<Self>) {
        self.confirm_logout = None;
        let Some(agent_dir) = self
            .settings_snapshot
            .as_ref()
            .and_then(|snapshot| snapshot.agent_dir.clone())
        else {
            return;
        };
        match pi_rpc::config::auth_remove_provider(&agent_dir, &provider) {
            Ok(_) => {
                if let Some(snapshot) = self.settings_snapshot.as_mut() {
                    snapshot.auth_providers.retain(|name| *name != provider);
                }
                self.refresh_providers(cx);
            }
            Err(error) => self.settings_error(error, cx),
        }
        cx.notify();
    }

    /// Sign in: run `pi auth login <provider>` in the built-in terminal and
    /// bring that tab forward. The interactive flow (browser, key paste)
    /// happens there; pido never touches the credentials themselves.
    pub fn login_provider(&mut self, provider: String, cx: &mut Context<Self>) {
        // Resolve pi the way the session spawner does — desktop launches
        // often miss the installer's PATH entries.
        let probe = pi_rpc::client::probe_pi_install(None);
        let program = probe
            .program()
            .map(|program| program.display().to_string())
            .unwrap_or_else(|| "pi".to_string());
        let quoted = if cfg!(windows) {
            format!("& '{program}'")
        } else {
            format!("'{program}'")
        };
        let command = format!("{quoted} auth login {provider}\r\n");
        self.right_open = true;
        self.right_tab = RightTab::Terminal;
        self.ensure_terminal(cx);
        if let Some(terminal) = self.slot().terminal.clone() {
            terminal.update(cx, |terminal, _| terminal.write(command.into_bytes()));
        }
        cx.notify();
    }

    /// The provider sign-out confirmation, while one is pending.
    fn logout_provider_dialog(&mut self, cx: &mut Context<Self>) -> Option<gpui::AnyElement> {
        use ely_gpui_component::overlays::ConfirmDialog;

        let provider = self.confirm_logout.clone()?;
        let me = cx.entity();
        let message =
            crate::i18n::t_arg(cx, "provider_logout_confirm", "name", &provider).to_string();
        let shut_me = me.clone();
        let confirm_me = me.clone();
        let shut = move |_: &mut Window, cx: &mut App| {
            shut_me.update(cx, |this, cx| {
                this.confirm_logout = None;
                cx.notify();
            });
        };
        // Captured for the same reason as the remove-project dialog:
        // ConfirmDialog closes before `on_confirm` runs.
        let confirm_provider = provider;
        Some(
            ConfirmDialog::new(
                "logout-provider-dialog",
                crate::i18n::t(cx, "provider_logout_title"),
                message,
                shut,
            )
            .confirm(crate::i18n::t(cx, "provider_logout_do"))
            .destructive()
            .on_confirm(move |_, cx| {
                confirm_me.update(cx, |this, cx| {
                    this.logout_provider(confirm_provider.clone(), cx)
                });
            })
            .into_any_element(),
        )
    }

    /// Where a scope's resource toggle writes: the settings file plus the
    /// base dir patterns are relative to.
    fn resource_target(
        &self,
        scope: pi_rpc::resources::Scope,
    ) -> Option<(std::path::PathBuf, std::path::PathBuf)> {
        let snapshot = self.settings_snapshot.as_ref()?;
        match scope {
            pi_rpc::resources::Scope::User => snapshot
                .agent_dir
                .as_ref()
                .map(|dir| (pi_rpc::config::agent_settings_path(dir), dir.clone())),
            pi_rpc::resources::Scope::Project => {
                let cwd = snapshot.project_cwd.as_ref()?;
                let base = cwd.join(".pi");
                Some((pi_rpc::config::project_settings_path(cwd), base))
            }
        }
    }

    /// Just the settings file for a scope, for edits that need no base dir.
    fn resource_settings_path(
        &self,
        scope: pi_rpc::resources::Scope,
    ) -> Option<std::path::PathBuf> {
        self.resource_target(scope).map(|(path, _)| path)
    }

    /// Shared tail of a settings write: refresh the snapshot's file views on
    /// success, surface a notice on failure.
    fn after_settings_write(&mut self, result: Result<(), String>, cx: &mut Context<Self>) {
        match result {
            Ok(()) => {
                if let Some(snapshot) = self.settings_snapshot.as_mut() {
                    snapshot.refresh_settings_files();
                }
                cx.notify();
            }
            Err(error) => self.settings_error(error, cx),
        }
    }

    fn rename_dialog(&mut self, cx: &mut Context<Self>) -> Option<gpui::AnyElement> {
        use ely_gpui_component::buttons::{Button, ButtonVariant};
        use ely_gpui_component::overlays::Dialog;

        if !self.slot().rename_open {
            return None;
        }
        let field = self.slot().rename_field.clone()?;
        let close_me = cx.entity();
        let shut = move |_: &mut Window, cx: &mut App| {
            close_me.update(cx, |this, cx| this.close_rename(cx));
        };
        let save_field = field.clone();
        let save_me = cx.entity();
        // The dialog is rebuilt every frame, so focus goes back to the field
        // each time — otherwise the composer would keep the keystrokes.
        let focus = field.read(cx).focus().clone();
        // Labels resolve here: the action closures must be 'static, so they
        // cannot borrow `cx` to look a string up when they run.
        let (title, dialog_id) = if self.rename_project.is_some() {
            (
                crate::i18n::t(cx, "rename_project"),
                "project-rename-dialog",
            )
        } else {
            (crate::i18n::t(cx, "rename"), "rename-dialog")
        };
        let cancel_label = crate::i18n::t(cx, "ext_cancel");
        let save_label = crate::i18n::t(cx, "save");
        Some(
            Dialog::new(dialog_id, title, shut)
                .focus_first(focus)
                .child(field)
                .action({
                    let me = cx.entity();
                    move |close| {
                        Button::new("rename-cancel", cancel_label.clone())
                            .variant(ButtonVariant::Ghost)
                            .on_click(move |_, window, cx| {
                                me.update(cx, |this, cx| this.close_rename(cx));
                                close(window, cx);
                            })
                    }
                })
                .action(move |close| {
                    Button::new("rename-save", save_label.clone())
                        .primary()
                        .on_click(move |_, window, cx| {
                            save_me.update(cx, |this, cx| this.finish_rename(&save_field, cx));
                            close(window, cx);
                        })
                })
                .into_any_element(),
        )
    }

    /// The native dialog for a pending extension request, if any. The text
    /// field for input/editor is created here, where a Window is available,
    /// and rebuilt only when the request changes.
    fn extension_dialog_overlay(
        &mut self,
        state: &gpui::Entity<AppState>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Option<gpui::AnyElement> {
        use ely_gpui_component::buttons::{Button, ButtonVariant};
        use ely_gpui_component::overlays::Dialog;

        let request = state.read(cx).pending_dialog.clone()?;

        // Keep one field alive per request; rebuild when the request changes.
        if self.slot().dialog_field_for.as_deref() != Some(request.id.as_str()) {
            self.slot_mut().dialog_field_for = Some(request.id.clone());
            let field = cx.new(|cx| {
                let mut input = match request.method {
                    pi_rpc::protocol::ExtensionUiMethod::Editor => {
                        TextInput::new(window, cx).multi_line(6, 24)
                    }
                    _ => TextInput::new(window, cx),
                };
                if let Some(prefill) = &request.prefill {
                    let prefill = prefill.clone();
                    input.set_text(prefill, cx);
                }
                input
            });
            self.slot_mut().dialog_field = Some(field.clone());

            // Enter (or the multi-line submit key) answers the dialog; this
            // is the same path the buttons take.
            let submit_state = state.clone();
            let submit_field = field.clone();
            let submit_id = request.id.clone();
            cx.subscribe(&field, move |_, _field, event: &InputEvent, cx| {
                if *event != InputEvent::Submit {
                    return;
                }
                let text = submit_field.read(cx).text().to_string();
                submit_state.update(cx, |s, cx| s.answer_dialog_value(&submit_id, text, cx));
            })
            .detach();
        }

        let title = request.title.clone().unwrap_or_else(|| {
            match request.method {
                pi_rpc::protocol::ExtensionUiMethod::Select => crate::i18n::t(cx, "ext_select"),
                pi_rpc::protocol::ExtensionUiMethod::Confirm => crate::i18n::t(cx, "ext_confirm"),
                pi_rpc::protocol::ExtensionUiMethod::Input => crate::i18n::t(cx, "ext_input"),
                pi_rpc::protocol::ExtensionUiMethod::Editor => crate::i18n::t(cx, "ext_editor"),
                _ => crate::i18n::t(cx, "extension_notify"),
            }
            .to_string()
        });

        let field = self.slot().dialog_field.clone()?;
        let cancel_state = state.clone();
        let cancel_id = request.id.clone();
        let shut = move |_: &mut Window, cx: &mut App| {
            cancel_state.update(cx, |s, cx| s.cancel_dialog(&cancel_id, cx));
        };

        let cancel_label = crate::i18n::t(cx, "ext_cancel");
        let cancel_label_for_closure = cancel_label.clone();
        let submit_label = crate::i18n::t(cx, "ext_submit");
        let confirm_label = crate::i18n::t(cx, "ext_confirm");

        let overlay: gpui::AnyElement = match request.method {
            pi_rpc::protocol::ExtensionUiMethod::Select => {
                let mut list = div().flex().flex_col().gap_2();
                for (ix, option) in request
                    .options
                    .clone()
                    .unwrap_or_default()
                    .into_iter()
                    .enumerate()
                {
                    let answer_state = state.clone();
                    let id = request.id.clone();
                    let value = option.clone();
                    list = list.child(div().w_full().child(
                        Button::new(("ext-option", ix), option).on_click(move |_, _, cx| {
                            answer_state
                                .update(cx, |s, cx| s.answer_dialog_value(&id, value.clone(), cx));
                        }),
                    ))
                }
                let mut select_dialog = Dialog::new(
                    ("ext-dialog", dialog_slot(request.id.as_str())),
                    title,
                    shut,
                );
                if let Some(message) = request.message.clone() {
                    select_dialog = select_dialog.detail(message);
                }
                select_dialog
                    .child(list)
                    .action(move |close| {
                        Button::new("ext-select-cancel", cancel_label.clone())
                            .variant(ButtonVariant::Ghost)
                            .on_click(move |_, window, cx| close(window, cx))
                    })
                    .into_any_element()
            }
            pi_rpc::protocol::ExtensionUiMethod::Confirm => {
                // A hand-built dialog rather than ConfirmDialog: that one
                // closes (which reads as a cancel) *before* handing over the
                // answer, so every confirmation reached pi as `cancelled`.
                let confirm_state = state.clone();
                let confirm_id = request.id.clone();
                let confirm_label = confirm_label.clone();
                let cancel_label = cancel_label_for_closure.clone();
                let mut confirm_dialog = Dialog::new(
                    ("ext-dialog", dialog_slot(request.id.as_str())),
                    title,
                    shut,
                );
                if let Some(message) = request.message.clone() {
                    confirm_dialog = confirm_dialog.detail(message);
                }
                confirm_dialog
                    .action(move |close| {
                        Button::new("ext-confirm-cancel", cancel_label.clone())
                            .variant(ButtonVariant::Ghost)
                            .on_click(move |_, window, cx| close(window, cx))
                    })
                    .action(move |close| {
                        Button::new("ext-confirm-yes", confirm_label.clone())
                            .primary()
                            .on_click(move |_, window, cx| {
                                confirm_state.update(cx, |s, cx| {
                                    s.answer_dialog_confirm(&confirm_id, true, cx)
                                });
                                close(window, cx);
                            })
                    })
                    .into_any_element()
            }
            pi_rpc::protocol::ExtensionUiMethod::Input => {
                // A hand-built dialog rather than PromptDialog: that one
                // closes (which reads as a cancel) *before* handing over the
                // text, so every answer arrived as `cancelled`.
                let submit_state = state.clone();
                let submit_field = field.clone();
                let submit_id = request.id.clone();
                let submit_label = submit_label.clone();
                let cancel_label = cancel_label_for_closure.clone();
                let field_focus = field.read(cx).focus().clone();
                let mut input_dialog = Dialog::new(
                    ("ext-dialog", dialog_slot(request.id.as_str())),
                    title,
                    shut,
                )
                .focus_first(field_focus);
                if let Some(placeholder) = request.placeholder.clone() {
                    input_dialog = input_dialog.detail(placeholder);
                }
                input_dialog
                    .child(field)
                    .action(move |close| {
                        Button::new("ext-input-cancel", cancel_label.clone())
                            .variant(ButtonVariant::Ghost)
                            .on_click(move |_, window, cx| close(window, cx))
                    })
                    .action(move |close| {
                        Button::new("ext-input-submit", submit_label.clone())
                            .primary()
                            .on_click(move |_, window, cx| {
                                let text = submit_field.read(cx).text().to_string();
                                submit_state.update(cx, |s, cx| {
                                    s.answer_dialog_value(&submit_id, text, cx)
                                });
                                close(window, cx);
                            })
                    })
                    .into_any_element()
            }
            pi_rpc::protocol::ExtensionUiMethod::Editor => {
                let submit_state = state.clone();
                let submit_field = field.clone();
                let submit_id = request.id.clone();
                let mut editor_dialog = Dialog::new(
                    ("ext-dialog", dialog_slot(request.id.as_str())),
                    title,
                    shut,
                );
                if let Some(message) = request.message.clone() {
                    editor_dialog = editor_dialog.detail(message);
                }
                editor_dialog
                    .child(div().min_h(gpui::px(160.)).child(field))
                    .action(move |close| {
                        Button::new("ext-editor-cancel", cancel_label)
                            .variant(ButtonVariant::Ghost)
                            .on_click(move |_, window, cx| close(window, cx))
                    })
                    .action(move |close| {
                        Button::new("ext-editor-submit", submit_label)
                            .primary()
                            .on_click(move |_, window, cx| {
                                let text = submit_field.read(cx).text().to_string();
                                submit_state.update(cx, |s, cx| {
                                    s.answer_dialog_value(&submit_id, text, cx)
                                });
                                close(window, cx);
                            })
                    })
                    .into_any_element()
            }
            _ => return None,
        };
        Some(overlay)
    }
}

/// A stable numeric slot for a dialog element id, from its request id.
fn dialog_slot(id: &str) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    id.hash(&mut hasher);
    hasher.finish()
}

/// Toggle light/dark (for the header button) and persist the choice.
pub fn toggle_theme(cx: &mut App) {
    let next = if cx.theme().is_dark() {
        Mode::Light
    } else {
        Mode::Dark
    };
    crate::settings::save_setting(|s| {
        s.theme = Some(if next == Mode::Light { "light" } else { "dark" }.to_string());
    });
    Theme::set_mode(next, cx);
}
