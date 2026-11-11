//! The sidebar: new-chat button plus past sessions grouped by project.
//!
//! Row styling mirrors ely-chat's `ConversationItem` (control-height row,
//! rounded hover/active background, ellipsized title, muted ink) with one
//! addition the component has no slot for: a relative age on the right.
//! Grouping is by project, matching pido's model, rather than the
//! component's built-in day buckets.
//!
//! Rows sit edge to edge: the pane's horizontal padding lives on the header
//! and captions, not on the list, so the hover and active backgrounds span
//! the full sidebar width. A right click opens the session menu.

use ely_gpui_component::buttons::{Button, ButtonVariant, IconButton};
use ely_gpui_component::forms as forms_util;
use ely_gpui_component::lists::VirtualList;
use ely_gpui_component::menus::{ContextMenu, Menu, MenuItem};
use ely_gpui_component::primitives::{Icon, IconName};
use ely_gpui_component::theme::{ActiveTheme, ControlSize, IconSize, Radius, TextSize};
use ely_gpui_component::typography::{Caption, Ellipsis, Label};
use gpui::{
    AnyElement, App, ClipboardItem, Entity, FontWeight, InteractiveElement, IntoElement,
    MouseButton, ParentElement, Pixels, SharedString, StatefulInteractiveElement, Styled, Window,
    div, prelude::FluentBuilder, px,
};

use crate::i18n;
use crate::state::{AppState, Conn, NoticeKind, SidebarRow};
use crate::ui::app::PidoApp;

/// Render the whole sidebar. Session clicks switch the running pi process to
/// that session file; a right click opens its menu.
pub fn render(
    state: &Entity<AppState>,
    view: &Entity<PidoApp>,
    window: &Window,
    cx: &App,
) -> impl IntoElement + use<> {
    let theme = cx.theme();
    let colors = theme.colors.clone();
    let (control_h, radius_md, text_sm, text_xs) = (
        theme.control_height(ControlSize::Md),
        theme.radius(Radius::Md),
        theme.text_size(TextSize::Sm),
        theme.text_size(TextSize::Xs),
    );
    // Sessions indent to where a project's label starts: past the
    // disclosure and the folder mark that sit in front of it.
    // gap_1p5 is 0.375rem at the theme's base size.
    let gap = gpui::Rems(0.375);
    let label_lead = (theme.icon_size(IconSize::Xs) + gap + theme.icon_size(IconSize::Sm) + gap)
        .to_pixels(window.rem_size());
    let app = state.clone();
    let s = state.read(cx);
    let can_start = s.conn == Conn::Live && !s.streaming;
    let empty = s.sidebar_rows.is_empty();

    div()
        // Width comes from the split pane the sidebar sits in.
        .w_full()
        .h_full()
        .min_h_0()
        .flex()
        .flex_col()
        .border_r_1()
        .border_color(colors.border)
        .bg(colors.surface)
        // No horizontal padding: rows carry it, so their backgrounds reach
        // the pane's edges.
        .py_3()
        .gap_3()
        .child(
            div()
                .flex()
                .flex_none()
                .px_3()
                .items_center()
                .justify_between()
                .child(
                    // The wordmark carries the brand; connection and model
                    // details live in the session header instead.
                    ely_gpui_component::primitives::Image::new(
                        "pido-wordmark",
                        crate::brand::wordmark(theme.is_dark(), 1),
                    )
                    .fit(gpui::ObjectFit::Contain)
                    .w(px(72.))
                    .h(px(34.)),
                )
                .child(
                    Button::new("new-chat", i18n::t(cx, "new_chat"))
                        .disabled(!can_start)
                        .on_click({
                            let app = app.clone();
                            move |_, _, cx| app.update(cx, |s, cx| s.new_chat(cx))
                        }),
                ),
        )
        .child(
            // The section header: the static 项目 title, with all the
            // actions on the right. The fold button flips between collapse
            // and expand as the groups go.
            div()
                .flex_none()
                .flex()
                .items_center()
                .gap_1p5()
                .px_2()
                .pb_1()
                .child(
                    div()
                        .text_size(theme.text_size(ely_gpui_component::theme::TextSize::Md))
                        .font_weight(FontWeight::BOLD)
                        .child(Label::new(i18n::t(cx, "projects"))),
                )
                .child(div().flex_1())
                .child({
                    let state = state.clone();
                    let folded = state.read(cx).all_projects_folded();
                    IconButton::new(
                        "projects-fold",
                        if folded {
                            IconName::ChevronsUpDown
                        } else {
                            IconName::List
                        },
                    )
                    .variant(ButtonVariant::Ghost)
                    .size(ely_gpui_component::theme::ControlSize::Sm)
                    .tooltip(i18n::t(
                        cx,
                        if folded { "expand_all" } else { "collapse_all" },
                    ))
                    .on_click(move |_, _, cx| {
                        state.update(cx, |s, cx| {
                            let folded = s.all_projects_folded();
                            s.set_all_projects_folded(!folded, cx);
                        });
                    })
                })
                .child({
                    let state = state.clone();
                    IconButton::new("projects-add", IconName::FolderPlus)
                        .variant(ButtonVariant::Ghost)
                        .size(ely_gpui_component::theme::ControlSize::Sm)
                        .tooltip(i18n::t(cx, "add_project"))
                        .on_click(move |_, window, cx| {
                            let state = state.clone();
                            let what = SharedString::from("add-project");
                            forms_util::choose(
                                what,
                                gpui::PathPromptOptions {
                                    files: false,
                                    directories: true,
                                    multiple: false,
                                    prompt: Some(i18n::t(cx, "add_project")),
                                },
                                window,
                                cx,
                                move |paths, _, cx| {
                                    state.update(cx, |s, cx| {
                                        for path in paths {
                                            s.add_project(path, cx);
                                        }
                                    });
                                },
                            );
                        })
                }),
        )
        .child(
            // The list must be the only growing part and stay inside the
            // window; without a bound its content stretches the whole row and
            // the session pane's composer lands below the window.
            div()
                .flex_1()
                .min_h_0()
                .min_w_0()
                .when(empty, |this| {
                    this.child(div().px_3().child(Caption::new(i18n::t(cx, "no_sessions"))))
                })
                .when(!empty, |this| {
                    // Only the rows in view are built: a project's whole
                    // history can be long, and this list redraws whenever the
                    // transcript does.
                    let rows = s.sidebar_rows.clone();
                    let state = state.clone();
                    let view = view.clone();
                    let (text_sm, text_xs, radius_md) = (text_sm, text_xs, radius_md);
                    this.child(
                        VirtualList::new(
                            ("sessions", rows.len() as u64),
                            rows.len(),
                            move |ix, _, cx| {
                                let row = &rows[ix];
                                match row {
                                    SidebarRow::Group {
                                        label,
                                        cwd,
                                        folded,
                                        sessions,
                                        archived,
                                    } => group_row(
                                        cx,
                                        ix,
                                        &state,
                                        label.clone(),
                                        cwd.clone(),
                                        *folded,
                                        *sessions,
                                        *archived,
                                    ),
                                    SidebarRow::Session {
                                        path,
                                        session_id,
                                        title,
                                        updated,
                                        pinned,
                                        archived,
                                    } => session_row(
                                        cx,
                                        ix,
                                        &state,
                                        &view,
                                        path.clone(),
                                        session_id.clone(),
                                        title.clone(),
                                        *updated,
                                        *pinned,
                                        *archived,
                                        control_h,
                                        radius_md,
                                        text_sm,
                                        text_xs,
                                        label_lead,
                                    ),
                                }
                            },
                        )
                        .size_full(),
                    )
                }),
        )
}

/// A project header, or the archive fold: chevron, mark, label, and (while
/// hovered) a new-session button for projects.
#[allow(clippy::too_many_arguments)]
fn group_row(
    cx: &App,
    ix: usize,
    state: &Entity<AppState>,
    label: SharedString,
    cwd: SharedString,
    folded: bool,
    sessions: usize,
    archived: bool,
) -> AnyElement {
    let colors = cx.theme().colors.clone();
    let group = SharedString::from(format!("project-{ix}"));
    let fold_state = state.clone();
    let fold_cwd = cwd.to_string();
    let new_state = state.clone();
    let new_cwd = cwd.to_string();
    let shown = if archived {
        i18n::t(cx, "archived")
    } else {
        label
    };
    let mark = if archived {
        IconName::Archive
    } else {
        IconName::Folder
    };
    div()
        .id(("project", ix as u64))
        .group(group.clone())
        .w_full()
        .flex()
        .items_center()
        .gap_1p5()
        .px_2()
        .pt_2()
        .pb_1()
        .cursor_pointer()
        .hover(|style| style.bg(colors.hover))
        .on_click(move |_, _, cx| {
            fold_state.update(cx, |s, cx| s.toggle_project(&fold_cwd, cx));
        })
        .child(Icon::new(mark).size(IconSize::Sm).color(colors.fg_subtle))
        .child(div().flex_1().min_w_0().child(Label::new(shown)))
        .when(folded, |this| {
            this.child(Caption::new(sessions.to_string()))
        })
        // The row's own action, shown while hovered: a new session in this
        // project. The archive fold starts no sessions.
        .when(!archived, |this| {
            this.child(
                div()
                    .id(("project-new", ix as u64))
                    .flex_none()
                    .invisible()
                    .group_hover(group, |style| style.visible())
                    .on_mouse_down(MouseButton::Left, |_, window, _| window.prevent_default())
                    .on_click(move |_, _, cx| {
                        cx.stop_propagation();
                        let cwd = std::path::PathBuf::from(&new_cwd);
                        new_state.update(cx, |s, cx| s.new_session_in(cwd, cx));
                    })
                    .child(
                        Icon::new(IconName::SquarePen)
                            .size(IconSize::Xs)
                            .color(colors.fg_subtle),
                    ),
            )
        })
        .into_any_element()
}

/// One session row, wrapped in its right-click menu.
#[allow(clippy::too_many_arguments)]
fn session_row(
    cx: &App,
    ix: usize,
    state: &Entity<AppState>,
    view: &Entity<PidoApp>,
    path: SharedString,
    session_id: SharedString,
    title: SharedString,
    updated: Option<i64>,
    pinned: bool,
    archived: bool,
    control_h: gpui::Rems,
    radius_md: gpui::Rems,
    text_sm: gpui::Rems,
    text_xs: gpui::Rems,
    label_lead: Pixels,
) -> AnyElement {
    let colors = cx.theme().colors.clone();
    let active = state
        .read(cx)
        .session_file
        .as_deref()
        .is_some_and(|open| same_file(open, &path));
    let age = i18n::relative_time(cx, updated);
    let app = state.clone();
    let click_path = path.clone();
    let row = div()
        .id(("session", ix as u64))
        .role(gpui::Role::ListItem)
        .aria_label(title.clone())
        .aria_selected(active)
        .w_full()
        .flex()
        .items_center()
        .gap_1()
        .h(control_h)
        .pl(px(8.) + label_lead)
        .pr_0p5()
        .rounded(radius_md)
        .text_size(text_sm)
        .text_color(if active { colors.fg } else { colors.fg_muted })
        .when(active, |row| row.bg(colors.active))
        .when(!active, |row| {
            row.cursor_pointer().hover(|style| style.bg(colors.hover))
        })
        .on_click(move |_, _, cx| {
            app.update(cx, |s, cx| {
                s.switch_session(std::path::PathBuf::from(click_path.to_string()), cx)
            });
        })
        .child(div().flex_1().min_w_0().child(Ellipsis::new(title.clone())))
        .when(pinned, |row| {
            row.child(
                Icon::new(IconName::Pin)
                    .size(IconSize::Xs)
                    .color(colors.fg_subtle),
            )
        })
        .child(
            div()
                .flex_none()
                .text_size(text_xs)
                .text_color(colors.fg_subtle)
                .child(age),
        );

    let menu = session_menu(
        cx,
        state,
        view,
        &path,
        &title,
        &session_id,
        pinned,
        archived,
        active,
    );
    ContextMenu::new(("session-menu", ix as u64), menu)
        .w_full()
        .child(row)
        .into_any_element()
}

/// The session's right-click menu. Chords are shown only on the open session,
/// since they act on it.
#[allow(clippy::too_many_arguments)]
fn session_menu(
    cx: &App,
    state: &Entity<AppState>,
    view: &Entity<PidoApp>,
    path: &SharedString,
    title: &SharedString,
    session_id: &SharedString,
    pinned: bool,
    archived: bool,
    active: bool,
) -> Menu {
    let mut pin = MenuItem::new(i18n::t(
        cx,
        if pinned {
            "unpin_session"
        } else {
            "pin_session"
        },
    ))
    .icon(if pinned {
        IconName::PinOff
    } else {
        IconName::Pin
    })
    .on_click({
        let state = state.clone();
        let path = path.to_string();
        move |_, cx| state.update(cx, |s, cx| s.toggle_pin(&path, cx))
    });

    let mut rename = MenuItem::new(i18n::t(cx, "rename_action"))
        .icon(IconName::Pencil)
        .on_click({
            let view = view.clone();
            let path = path.to_string();
            let title = title.to_string();
            move |window, cx| {
                view.update(cx, |this, cx| {
                    this.open_rename_at(path.clone(), title.clone(), window, cx)
                });
            }
        });

    let mut fork = MenuItem::new(i18n::t(cx, "fork_session"))
        .icon(IconName::GitBranch)
        .on_click({
            let state = state.clone();
            let path = path.to_string();
            move |_, cx| state.update(cx, |s, cx| s.fork_session(&path, cx))
        });

    let copy = MenuItem::new(i18n::t(cx, "copy_session_id"))
        .icon(IconName::Copy)
        .on_click({
            let state = state.clone();
            let id = session_id.to_string();
            move |_, cx| {
                cx.write_to_clipboard(ClipboardItem::new_string(id.clone()));
                state.update(cx, |s, cx| {
                    s.push_notice(
                        NoticeKind::Info,
                        i18n::t(cx, "session_id_copied").to_string(),
                    );
                    cx.notify();
                });
            }
        });

    let reveal = MenuItem::new(i18n::t(cx, "reveal_in_folder"))
        .icon(IconName::FolderOpen)
        .on_click({
            let path = path.to_string();
            move |_, _| crate::state::reveal_in_folder(std::path::Path::new(&path))
        });

    let mut archive = MenuItem::new(i18n::t(
        cx,
        if archived {
            "unarchive_session"
        } else {
            "archive_session"
        },
    ))
    .icon(IconName::Archive)
    .on_click({
        let state = state.clone();
        let path = path.to_string();
        move |_, cx| state.update(cx, |s, cx| s.toggle_archive(&path, cx))
    });

    if active {
        // These chords act on the open session; showing them on other rows
        // would promise something the shortcut does not do.
        pin = pin.keys("ctrl-alt-p");
        rename = rename.keys("ctrl-alt-g");
        fork = fork.keys("ctrl-alt-f");
        archive = archive.keys("ctrl-shift-a");
    }

    Menu::new()
        .item(pin)
        .item(rename)
        .item(fork)
        .separator()
        .item(copy)
        .item(reveal)
        .separator()
        .item(archive)
}

/// Compare two session paths without caring about separator style; Windows
/// paths are also case-insensitive.
fn same_file(a: &str, b: &str) -> bool {
    let normalize = |path: &str| {
        let path = path.replace('\\', "/");
        if cfg!(windows) {
            path.to_lowercase()
        } else {
            path
        }
    };
    std::path::Path::new(a) == std::path::Path::new(b) || normalize(a) == normalize(b)
}
