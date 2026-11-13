//! The root view: Ely's FocusScope wrapping a sidebar and the session pane.

use ely_gpui_component::forms::{InputEvent, TextInput};
use ely_gpui_component::layout::SplitPane;
use ely_gpui_component::primitives::FocusScope;
use ely_gpui_component::theme::{ActiveTheme, ContainerSize, Mode, Theme};
use gpui::{
    App, AppContext, Context, FocusHandle, InteractiveElement, IntoElement, ParentElement, Render,
    StatefulInteractiveElement, Styled, Window, actions, div,
};

use crate::state::{AppState, Conn};
use crate::ui::{chat, composer, sidebar, welcome};

actions!(
    pido,
    [Stop, PinSession, RenameSession, ForkSession, ArchiveSession]
);

pub const HEADER_HEIGHT: f32 = 52.;

/// The sidebar's default width, in logical pixels.
const SIDEBAR_DEFAULT: f32 = 264.0;
/// The sidebar may drag to 0.8x–1.5x of that default.
const SIDEBAR_RANGE: std::ops::RangeInclusive<f32> =
    (SIDEBAR_DEFAULT * 0.8)..=(SIDEBAR_DEFAULT * 1.5);
/// The right panel's width range, in logical pixels.
const RIGHT_RANGE: std::ops::RangeInclusive<f32> = 200.0..=900.0;

/// Keep a pane's width inside its range; dragging past a bound holds the
/// value there, and the next render reseeds the split back into range.
fn clamp_width(width: f32, range: &std::ops::RangeInclusive<f32>) -> f32 {
    width.clamp(*range.start(), *range.end())
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
    pub state: gpui::Entity<AppState>,
    /// The composer's multi-line field, owned here so it survives re-renders.
    pub draft: Option<gpui::Entity<TextInput>>,
    /// The welcome screen's project-directory field, created lazily.
    pub welcome_dir: Option<gpui::Entity<TextInput>>,
    /// Last observed window size, persisted on quit.
    window_size: Option<(f32, f32)>,
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
    /// The project-rename dialog: the project path being named. When set it
    /// takes over the same dialog slot as the session rename.
    rename_project: Option<String>,
    /// The remove-project confirmation: which path it is about.
    confirm_remove: Option<String>,
    /// The right panel: open, which file is picked, and its diff layout.
    pub diff_selected: Option<String>,
    pub diff_layout: ely_gpui_component::git::DiffLayout,
    /// Which right-panel tab shows, and whether the panel is open at all.
    pub right_tab: RightTab,
    right_open: bool,
    /// The panel's terminal, spawned on first use and kept while the app runs.
    pub terminal: Option<gpui::Entity<ely_gpui_component::terminal::Terminal>>,
}

/// The right panel's two tabs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RightTab {
    /// Reviewing the session's file changes.
    Diff,
    /// A live shell next to the chat.
    Terminal,
}

impl PidoApp {
    pub fn new(cx: &mut Context<Self>) -> Self {
        let focus = cx.focus_handle();
        let state = AppState::new(cx);
        // Re-render whenever the state entity changes, and drain any text a
        // cancelled/failed action wants restored into the composer.
        cx.observe(&state, |this, state, cx| {
            let restored = state.update(cx, |state, _| state.take_pending_composer_text());
            if let (Some(text), Some(draft)) = (restored, this.draft.clone()) {
                draft.update(cx, |input, cx| {
                    if !input.text().trim().is_empty() {
                        input.insert("\n", cx);
                    }
                    input.insert(&text, cx);
                });
            }
            cx.notify();
        })
        .detach();
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
        let app = Self {
            left_px,
            right_px,
            snap_since,
            snap_flip,
            focus,
            state,
            draft: None,
            welcome_dir: None,
            window_size: None,
            composer_focused: false,
            dialog_field: None,
            dialog_field_for: None,
            rename_open: false,
            rename_field: None,
            rename_target: None,
            rename_project: None,
            confirm_remove: None,
            diff_selected: None,
            diff_layout: ely_gpui_component::git::DiffLayout::Unified,
            right_tab: RightTab::Diff,
            right_open,
            terminal: None,
        };
        app.observe_window_for_quit(cx);
        app
    }

    /// Stash the window size on every render and persist it when the app
    /// quits, so the window reopens where the user left it.
    fn observe_window_for_quit(&self, cx: &mut Context<Self>) {
        cx.on_app_quit(|this, _cx| {
            if let Some((width, height)) = this.window_size {
                crate::settings::save_setting(|s| {
                    s.window = Some(crate::settings::WindowState { width, height });
                });
            }
            async {}
        })
        .detach();
    }

    /// Called once the window is open: fields that need a Window are created
    /// here (see main.rs).
    pub fn setup(&mut self, window: &mut gpui::Window, cx: &mut Context<Self>) {
        self.draft = Some(cx.new(|cx| {
            TextInput::new(window, cx)
                .multi_line(1, 8)
                .placeholder_key("idle_hint")
        }));
        // A session restored with the terminal tab out gets its shell back.
        if self.right_open && self.right_tab == RightTab::Terminal {
            self.ensure_terminal(cx);
        }
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
            self.state
                .update(cx, |s, cx| s.connect(path, resume, extra, cx));
        }
    }
}

impl Render for PidoApp {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme();
        let (bg, fg) = (theme.colors.bg, theme.colors.fg);
        let column = theme.container_width(ContainerSize::Md);
        let bounds = window.bounds();
        self.window_size = Some((f32::from(bounds.size.width), f32::from(bounds.size.height)));
        let state = self.state.clone();
        let live = matches!(state.read(cx).conn, Conn::Live | Conn::Reconnecting { .. });

        if live && !self.composer_focused {
            if let Some(draft) = &self.draft {
                let focus = draft.read(cx).focus().clone();
                window.focus(&focus, cx);
            }
            self.composer_focused = true;
        } else if !live {
            self.composer_focused = false;
        }

        let main: gpui::AnyElement = if live {
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
                        .child(chat::render_header(&state, &cx.entity(), window, cx)),
                )
                .child(
                    div()
                        .flex_1()
                        .min_h_0()
                        .overflow_hidden()
                        .child(chat::render_transcript(&state, cx)),
                )
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
        // the remove-project confirmation and the branch tree join them.
        let overlay = self
            .extension_dialog_overlay(&state, window, cx)
            .or_else(|| self.rename_dialog(cx))
            .or_else(|| self.remove_project_dialog(cx))
            .or_else(|| self.branch_tree_dialog(&state, cx));
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
                        // Dragging past a limit clamps the stored width, but
                        // the split only reseeds from it on the next render —
                        // so the handler nudges one when a drag left the range.
                        let me = cx.entity().downgrade();
                        SplitPane::new("shell", gpui::Axis::Horizontal, gpui::px(200.))
                            .sizes(&sizes)
                            .pane(sidebar::render(&state, &cx.entity(), window, cx))
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
                                crate::settings::save_setting(|s| {
                                    s.sidebar_width = Some(left);
                                    if shares.len() > 2 {
                                        s.right_width = Some(right);
                                    }
                                });
                                if drifted && let Err(error) = me.update(cx, |_, cx| cx.notify()) {
                                    log::debug!("split snap-back lost: {error}");
                                }
                            })
                    })
                    .children(overlay)
                    // Esc stops a running agent (clear queue + abort, the
                    // protocol's recommended Esc semantics). TextInput does not
                    // consume Escape, so this also works while the composer is
                    // focused.
                    .on_action(cx.listener(|this, _: &Stop, _window, cx| {
                        // Esc backs out of the right panel before it stops a
                        // run.
                        if this.right_open {
                            this.close_right(cx);
                            return;
                        }
                        if this.state.read(cx).streaming {
                            this.state.update(cx, |s, cx| s.stop(cx));
                        }
                    }))
                    .on_action(cx.listener(|this, _: &PinSession, _window, cx| {
                        if let Some(file) = this.state.read(cx).session_file.clone() {
                            this.state.update(cx, |s, cx| s.toggle_pin(&file, cx));
                        }
                    }))
                    .on_action(cx.listener(|this, _: &RenameSession, window, cx| {
                        if this.state.read(cx).conn == Conn::Live {
                            this.open_rename(window, cx);
                        }
                    }))
                    .on_action(cx.listener(|this, _: &ForkSession, _window, cx| {
                        if let Some(file) = this.state.read(cx).session_file.clone() {
                            this.state.update(cx, |s, cx| s.fork_session(&file, cx));
                        }
                    }))
                    .on_action(cx.listener(|this, _: &ArchiveSession, _window, cx| {
                        if let Some(file) = this.state.read(cx).session_file.clone() {
                            this.state.update(cx, |s, cx| s.toggle_archive(&file, cx));
                        }
                    })),
            )
    }
}

impl PidoApp {
    /// The split's sizes, in shares of the window, from the clamped widths.
    fn pane_sizes(&self, window_width: f32) -> Vec<f32> {
        let left = self.left_px.get() / window_width;
        if self.right_open {
            let right = self.right_px.get() / window_width;
            vec![left, 1.0 - left - right, right]
        } else {
            vec![left, 1.0 - left]
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
        if tab == RightTab::Diff {
            // Reopen on the newest file rather than a stale pick.
            self.diff_selected = None;
        } else {
            self.ensure_terminal(cx);
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

    /// The panel's shell, spawned on first use; a dead one spawns again.
    fn ensure_terminal(&mut self, cx: &mut Context<Self>) {
        use ely_gpui_component::terminal::{Launch, Terminal};
        if self.terminal.is_some() {
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
        let cwd = self.state.read(cx).cwd.clone().or_else(dirs::home_dir);
        let launch = Launch {
            program,
            cwd,
            env: Vec::new(),
        };
        let spawned = cx.new(|cx| Terminal::spawn(launch, cx).expect("terminal starts"));
        cx.subscribe(
            &spawned,
            |this, _, event: &ely_gpui_component::terminal::TerminalEvent, cx| {
                if let ely_gpui_component::terminal::TerminalEvent::Exited(_) = event {
                    // The shell is gone; the next open spawns a fresh one.
                    this.terminal = None;
                    cx.notify();
                }
            },
        )
        .detach();
        self.terminal = Some(spawned);
    }

    /// Spawn a fresh shell in the session's directory, dropping the old one.
    pub fn restart_terminal(&mut self, cx: &mut Context<Self>) {
        self.terminal = None;
        self.ensure_terminal(cx);
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

    /// The remove-project confirmation, while one is pending.
    fn remove_project_dialog(&mut self, cx: &mut Context<Self>) -> Option<gpui::AnyElement> {
        use ely_gpui_component::overlays::ConfirmDialog;

        let cwd = self.confirm_remove.clone()?;
        let me = cx.entity();
        let label = self
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
                    let cwd = this.confirm_remove.take().unwrap_or_default();
                    this.state.update(cx, |s, cx| s.remove_project(&cwd, cx));
                    cx.notify();
                });
            })
            .into_any_element(),
        )
    }

    /// Open the rename dialog over the current session's name.
    pub fn open_rename(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let (target, current) = {
            let state = self.state.read(cx);
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
            let state = self.state.read(cx);
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
        self.rename_field = Some(field.clone());
        self.rename_open = true;
        self.rename_target = target;
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
            self.state
                .update(cx, |state, cx| state.rename_project(&cwd, &name, cx));
        } else {
            match self.rename_target.clone() {
                Some(path) => self
                    .state
                    .update(cx, |state, cx| state.rename_session_at(&path, name, cx)),
                None => self
                    .state
                    .update(cx, |state, cx| state.rename_session(name, cx)),
            }
        }
        self.rename_open = false;
        self.rename_field = None;
        self.rename_target = None;
        self.rename_project = None;
        cx.notify();
    }

    /// The rename dialog, while open.
    fn rename_dialog(&mut self, cx: &mut Context<Self>) -> Option<gpui::AnyElement> {
        use ely_gpui_component::buttons::{Button, ButtonVariant};
        use ely_gpui_component::overlays::Dialog;

        if !self.rename_open {
            return None;
        }
        let field = self.rename_field.clone()?;
        let close_me = cx.entity();
        let shut = move |_: &mut Window, cx: &mut App| {
            close_me.update(cx, |this, cx| {
                this.rename_open = false;
                this.rename_field = None;
                this.rename_target = None;
                this.rename_project = None;
                cx.notify();
            });
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
                                me.update(cx, |this, cx| {
                                    this.rename_open = false;
                                    this.rename_field = None;
                                    this.rename_target = None;
                                    cx.notify();
                                });
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

    /// The branch dialog for the open session: every user prompt in the
    /// session's tree, indented by depth, the active path marked. Clicking
    /// a row forks a new session at that prompt — the only way RPC offers
    /// to move onto a branch.
    fn branch_tree_dialog(
        &mut self,
        state: &gpui::Entity<AppState>,
        cx: &mut Context<Self>,
    ) -> Option<gpui::AnyElement> {
        use gpui::prelude::FluentBuilder;
        use {
            ely_gpui_component::buttons::{Button, ButtonVariant},
            ely_gpui_component::overlays::Dialog,
        };

        let s = state.read(cx);
        if !s.branch_tree_open {
            return None;
        }
        let rows = s.branch_rows.clone().unwrap_or_default();
        let close_state = state.clone();
        let shut = move |_: &mut Window, cx: &mut App| {
            close_state.update(cx, |s, cx| {
                s.branch_tree_open = false;
                s.branch_rows = None;
                cx.notify();
            });
        };
        // Labels resolve here: the closures must be 'static, so they cannot
        // borrow `cx` to look a string up when they run.
        let title = crate::i18n::t(cx, "branch_tree");
        let detail = crate::i18n::t(cx, "branch_tree_hint");
        let close_label = crate::i18n::t(cx, "ext_cancel");
        let empty_label = crate::i18n::t(cx, "tree_empty");
        let active_label = crate::i18n::t(cx, "tree_active");
        let colors = cx.theme().colors.clone();
        let pick_state = state.clone();
        let mut list = div()
            .id("branch-list")
            .w(gpui::px(560.0))
            .max_h(gpui::px(420.0))
            .overflow_y_scroll()
            .flex()
            .flex_col()
            .gap_0p5();
        if rows.is_empty() {
            list = list.child(
                div()
                    .py_3()
                    .text_color(colors.fg_subtle)
                    .child(empty_label),
            );
        }
        for (ix, row) in rows.iter().enumerate() {
            let row_state = pick_state.clone();
            let entry_id = row.entry_id.clone();
            let row_active = row.active;
            let text: gpui::SharedString = row.text.clone().into();
            let active_chip: gpui::SharedString = active_label.clone();
            list = list.child(
                div()
                    .id(("branch-row", ix))
                    .w_full()
                    .flex()
                    .items_center()
                    .gap_2()
                    .px_2()
                    .py_1()
                    .rounded_sm()
                    .cursor_pointer()
                    .hover(|style| style.bg(colors.hover))
                    .on_click(move |_, _, cx| {
                        row_state.update(cx, |s, cx| {
                            s.branch_tree_open = false;
                            s.branch_rows = None;
                            s.fork_from_entry(&entry_id, cx);
                        });
                    })
                    // Each level of the tree indents one step; abandoned
                    // branches sit deeper, under their fork point.
                    .child(div().flex_none().w(gpui::px(row.depth as f32 * 18.0)))
                    .when(row_active, |el| el.text_color(colors.info))
                    .child(div().flex_1().min_w_0().truncate().child(text))
                    .when(row_active, |el| {
                        el.child(
                            div()
                                .flex_none()
                                .px_1p5()
                                .rounded_full()
                                .bg(colors.info_subtle)
                                .text_color(colors.info)
                                .text_size(gpui::px(11.0))
                                .child(active_chip),
                        )
                    }),
            );
        }
        Some(
            Dialog::new("branch-dialog", title, shut)
                .detail(detail)
                .child(list)
                .action(move |close| {
                    Button::new("branch-close", close_label.clone())
                        .variant(ButtonVariant::Ghost)
                        .on_click(move |_, window, cx| close(window, cx))
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
        use ely_gpui_component::overlays::{ConfirmDialog, Dialog};

        let request = state.read(cx).pending_dialog.clone()?;

        // Keep one field alive per request; rebuild when the request changes.
        if self.dialog_field_for.as_deref() != Some(request.id.as_str()) {
            self.dialog_field_for = Some(request.id.clone());
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
            self.dialog_field = Some(field.clone());

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

        let field = self.dialog_field.clone()?;
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
                let confirm_state = state.clone();
                let confirm_id = request.id.clone();
                ConfirmDialog::new(
                    ("ext-dialog", dialog_slot(request.id.as_str())),
                    title,
                    request.message.clone().unwrap_or_default(),
                    shut,
                )
                .confirm(confirm_label)
                .on_confirm(move |_, cx| {
                    confirm_state
                        .update(cx, |s, cx| s.answer_dialog_confirm(&confirm_id, true, cx));
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
