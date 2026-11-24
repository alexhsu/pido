//! The settings window: a modal card over the main window with a nav
//! column (general / models / skills / extensions) and a content pane.
//!
//! The General page edits pido's own preferences and the active session's
//! pi-side toggles; the other three pages are read-only views of pi's
//! configuration, with buttons that reveal the backing files. Everything
//! shown was snapshotted once when the window opened ([`SettingsSnapshot`]).

use std::path::{Path, PathBuf};

use ely_gpui_component::buttons::{Button, ButtonVariant, IconButton};
use ely_gpui_component::forms::Switch;
use ely_gpui_component::primitives::{Icon, IconName};
use ely_gpui_component::theme::{ActiveTheme, IconSize};
use gpui::{
    AnyElement, App, Div, Entity, FontWeight, InteractiveElement, IntoElement, ParentElement,
    SharedString, StatefulInteractiveElement, Styled, Window, div, px, relative, rgba,
    prelude::FluentBuilder,
};
use pi_rpc::resources::{BUILTIN_EXTENSIONS, ExtensionSource, Scope, SkillSummary};

use crate::i18n;
use crate::settings::{Settings, ThemePref, apply_theme_pref, save_setting};
use crate::state::{Conn, level_slug};
use crate::ui::app::PidoApp;

/// The providers pido probes by default; `auth.json` keys join them.
const KNOWN_PROVIDERS: &[&str] = &["deepseek", "anthropic", "openai", "openrouter", "google"];

/// Which settings page the window shows.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SettingsPage {
    General,
    Models,
    Skills,
    Extensions,
}

const ALL_PAGES: [SettingsPage; 4] = [
    SettingsPage::General,
    SettingsPage::Models,
    SettingsPage::Skills,
    SettingsPage::Extensions,
];

impl SettingsPage {
    fn icon(self) -> IconName {
        match self {
            SettingsPage::General => IconName::Settings,
            SettingsPage::Models => IconName::Cpu,
            SettingsPage::Skills => IconName::Sparkles,
            SettingsPage::Extensions => IconName::Puzzle,
        }
    }

    fn title(self, cx: &App) -> SharedString {
        let key = match self {
            SettingsPage::General => "settings_general",
            SettingsPage::Models => "settings_models",
            SettingsPage::Skills => "settings_skills",
            SettingsPage::Extensions => "settings_extensions",
        };
        i18n::t(cx, key)
    }

    fn detail(self, cx: &App) -> SharedString {
        let key = match self {
            SettingsPage::General => "settings_general_detail",
            SettingsPage::Models => "settings_models_detail",
            SettingsPage::Skills => "settings_skills_detail",
            SettingsPage::Extensions => "settings_extensions_detail",
        };
        i18n::t(cx, key)
    }
}

/// Everything the window shows, read once per open. Files are tiny, so a
/// synchronous read on the click path is fine — and keeps the render path
/// free of file IO.
pub struct SettingsSnapshot {
    pub theme_pref: ThemePref,
    pub locale: String,
    pub queue_mode: String,
    pub update_check: bool,
    pub agent_dir: Option<PathBuf>,
    /// pi's global settings.json, raw.
    pub pi_settings: serde_json::Value,
    /// Providers with stored credentials (auth.json keys).
    pub auth_providers: Vec<String>,
    /// Providers whose `pi auth check` passed at probe time.
    pub ready_providers: Vec<String>,
    pub skills: Vec<SkillSummary>,
    pub extensions: Vec<ExtensionSource>,
}

impl SettingsSnapshot {
    pub fn load(project_cwd: Option<&Path>, ready_providers: Vec<String>) -> Self {
        let persisted = Settings::load();
        let agent_dir = pi_rpc::client::pi_agent_dir();
        Self {
            theme_pref: persisted.theme_pref(),
            locale: persisted
                .locale
                .unwrap_or_else(|| i18n::ZH.to_string()),
            queue_mode: persisted
                .queue_mode
                .unwrap_or_else(|| "steer".to_string()),
            update_check: persisted.update_check.unwrap_or(true),
            pi_settings: pi_rpc::client::pi_agent_settings(),
            auth_providers: pi_rpc::client::pi_auth_provider_keys(),
            ready_providers,
            skills: pi_rpc::resources::pi_skills(agent_dir.as_deref(), project_cwd),
            extensions: pi_rpc::resources::pi_extensions(agent_dir.as_deref(), project_cwd),
            agent_dir,
        }
    }

    fn setting_str(&self, key: &str) -> Option<String> {
        self.pi_settings
            .get(key)
            .and_then(|value| value.as_str())
            .map(String::from)
    }
}

/// The whole settings window, riding at the root above every pane.
/// `None` while it is closed.
pub fn settings_overlay(
    shell: &PidoApp,
    view: &Entity<PidoApp>,
    _window: &mut Window,
    cx: &App,
) -> Option<AnyElement> {
    let page = shell.settings_open?;
    let snapshot = shell.settings_snapshot.as_ref()?;
    let colors = &cx.theme().colors;

    let scrim = div()
        .id("settings-overlay")
        .absolute()
        .inset_0()
        .occlude()
        .bg(rgba(0x00000059))
        .on_mouse_down(gpui::MouseButton::Left, {
            let view = view.downgrade();
            move |_, _, cx| {
                view.update(cx, |this, cx| this.close_settings(cx)).ok();
            }
        })
        .child(
            // Clicks inside the card must not fall through to the scrim's
            // close handler.
            div()
                .absolute()
                .inset_0()
                .flex()
                .items_center()
                .justify_center()
                .child(
                    div()
                        .id("settings-card")
                        .on_mouse_down(gpui::MouseButton::Left, |_, _, cx| {
                            cx.stop_propagation()
                        })
                        .w(relative(0.82))
                        .max_w(px(1000.))
                        .min_w(px(760.))
                        .h(relative(0.75))
                        .max_h(px(680.))
                        .min_h(px(460.))
                        .flex()
                        .flex_col()
                        .rounded_lg()
                        .border_1()
                        .border_color(colors.border)
                        .bg(colors.surface)
                        .shadow_lg()
                        .overflow_hidden()
                        .child(header(view, snapshot, cx))
                        .child(
                            div()
                                .flex_1()
                                .min_h_0()
                                .flex()
                                .child(nav_column(page, view, cx))
                                .child(content_pane(shell, page, snapshot, view, cx)),
                        ),
                ),
        );
    Some(scrim.into_any_element())
}

/// Title, reveal-config button, close button.
fn header(view: &Entity<PidoApp>, snapshot: &SettingsSnapshot, cx: &App) -> Div {
    let colors = &cx.theme().colors;
    let open_target = config_file_target(snapshot);
    div()
        .flex_none()
        .flex()
        .items_center()
        .gap_2()
        .px_4()
        .h(px(52.))
        .border_b_1()
        .border_color(colors.border)
        .child(
            div()
                .text_lg()
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(colors.fg)
                .child(i18n::t(cx, "settings").to_string()),
        )
        .child(div().flex_1())
        .child(
            Button::new("settings-open-config", i18n::t(cx, "open_config_file"))
                .variant(ButtonVariant::Secondary)
                .on_click(move |_, _, _| crate::state::reveal_in_folder(&open_target)),
        )
        .child(
            IconButton::new("settings-close", IconName::X)
                .variant(ButtonVariant::Ghost)
                .tooltip(i18n::t(cx, "close"))
                .on_click({
                    let view = view.clone();
                    move |_, _, cx| {
                        view.update(cx, |this, cx| this.close_settings(cx));
                    }
                }),
        )
}

/// The file `打开配置文件` should reveal: pi's settings.json when it
/// exists, otherwise the agent directory itself.
fn config_file_target(snapshot: &SettingsSnapshot) -> PathBuf {
    let dir = snapshot
        .agent_dir
        .clone()
        .unwrap_or_else(|| PathBuf::from("."));
    let settings = dir.join("settings.json");
    if settings.is_file() {
        settings
    } else {
        dir
    }
}

/// The nav column: one row per page, the active one highlighted.
fn nav_column(active: SettingsPage, view: &Entity<PidoApp>, cx: &App) -> Div {
    let colors = &cx.theme().colors;
    div()
        .flex_none()
        .w(px(190.))
        .flex()
        .flex_col()
        .gap_1()
        .p_2()
        .border_r_1()
        .border_color(colors.border)
        .children(ALL_PAGES.map(|page| {
            let selected = page == active;
            div()
                .id(gpui::ElementId::Name(match page {
                    SettingsPage::General => "nav-general".into(),
                    SettingsPage::Models => "nav-models".into(),
                    SettingsPage::Skills => "nav-skills".into(),
                    SettingsPage::Extensions => "nav-extensions".into(),
                }))
                .flex()
                .items_center()
                .gap_2()
                .px_3()
                .py_2()
                .rounded_md()
                .cursor_pointer()
                .when(selected, |row| {
                    row.bg(colors.selection).text_color(colors.fg)
                })
                .when(!selected, |row| {
                    row.text_color(colors.fg_muted).hover(|row| row.bg(colors.hover))
                })
                .child(
                    Icon::new(page.icon())
                        .size(IconSize::Sm)
                        .color(if selected { colors.info } else { colors.fg_subtle }),
                )
                .child(div().text_sm().child(page.title(cx).to_string()))
                .on_click({
                    let view = view.clone();
                    move |_, _, cx| {
                        view.update(cx, |this, cx| this.set_settings_page(page, cx));
                    }
                })
        }))
}

/// The right pane: page title and description over the page's sections.
fn content_pane(
    shell: &PidoApp,
    page: SettingsPage,
    snapshot: &SettingsSnapshot,
    view: &Entity<PidoApp>,
    cx: &App,
) -> gpui::Stateful<Div> {
    let colors = &cx.theme().colors;
    let body = match page {
        SettingsPage::General => general_page(shell, snapshot, view, cx).into_any_element(),
        SettingsPage::Models => models_page(shell, snapshot, cx).into_any_element(),
        SettingsPage::Skills => skills_page(snapshot, cx).into_any_element(),
        SettingsPage::Extensions => extensions_page(snapshot, cx).into_any_element(),
    };
    div()
        .id("settings-content")
        .flex_1()
        .min_w_0()
        .overflow_y_scroll()
        .px_6()
        .py_5()
        .flex()
        .flex_col()
        .gap_4()
        .child(
            div()
                .flex()
                .flex_col()
                .gap_1()
                .child(
                    div()
                        .text_lg()
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(colors.fg)
                        .child(page.title(cx).to_string()),
                )
                .child(
                    div()
                        .text_sm()
                        .text_color(colors.fg_muted)
                        .child(page.detail(cx).to_string()),
                ),
        )
        .child(body)
}

/// A titled block of rows.
fn section(title: impl Into<SharedString>, cx: &App) -> Div {
    let colors = &cx.theme().colors;
    div()
        .flex()
        .flex_col()
        .gap_2()
        .child(
            div()
                .text_sm()
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(colors.fg)
                .child(title.into().to_string()),
        )
}

/// One settings row: a title, optional description, and a control at the
/// trailing edge.
fn row(title: impl Into<SharedString>, detail: Option<SharedString>, cx: &App) -> Div {
    let colors = &cx.theme().colors;
    div()
        .flex()
        .items_center()
        .gap_3()
        .px_3()
        .py_2()
        .rounded_md()
        .border_1()
        .border_color(colors.border)
        .child(
            div()
                .flex()
                .flex_col()
                .min_w_0()
                .flex_1()
                .child(
                    div()
                        .text_sm()
                        .text_color(colors.fg)
                        .child(title.into().to_string()),
                )
                .children(detail.map(|detail| {
                    div()
                        .text_xs()
                        .text_color(colors.fg_subtle)
                        .child(detail.to_string())
                })),
        )
}

/// A small hand-rolled segment button, the low-risk stand-in for a
/// segmented control: selected rows get the info accent.
fn seg(
    id: impl Into<gpui::ElementId>,
    label: impl Into<SharedString>,
    selected: bool,
    view: &Entity<PidoApp>,
    cx: &App,
    on_click: impl Fn(&mut PidoApp, &mut gpui::Context<PidoApp>) + 'static,
) -> gpui::Stateful<Div> {
    let colors = &cx.theme().colors;
    div()
        .id(id.into())
        .px_3()
        .py_1()
        .rounded_md()
        .text_sm()
        .cursor_pointer()
        .when(selected, |seg| {
            seg.bg(colors.info)
                .text_color(colors.surface)
                .font_weight(FontWeight::MEDIUM)
        })
        .when(!selected, |seg| {
            seg.text_color(colors.fg_muted)
                .hover(|seg| seg.bg(colors.hover))
        })
        .child(label.into().to_string())
        .on_click({
            let view = view.clone();
            move |_, _, cx| {
                view.update(cx, |shell, cx| on_click(shell, cx));
            }
        })
}

// ---------------------------------------------------------------------------
// 通用设置
// ---------------------------------------------------------------------------

fn general_page(
    shell: &PidoApp,
    snapshot: &SettingsSnapshot,
    view: &Entity<PidoApp>,
    cx: &App,
) -> Div {
    let connected = live_connection(shell, cx);
    div().flex().flex_col().gap_4()
        // 外观
        .child({
            let mut block = section(i18n::t(cx, "settings_appearance"), cx);
            block = block.child(
                row(i18n::t(cx, "settings_appearance_detail"), None, cx).child(
                    seg_group_children(snapshot, view, cx),
                ),
            );
            block
        })
        // 语言
        .child(
            section(i18n::t(cx, "language"), cx).child(
                row(i18n::t(cx, "settings_language_detail"), None, cx).child(
                    div()
                        .flex()
                        .flex_none()
                        .gap_1()
                        .rounded_md()
                        .border_1()
                        .border_color(cx.theme().colors.border)
                        .p_1()
                        .child(
                            seg(
                                "lang-zh",
                                "中文",
                                snapshot.locale == i18n::ZH,
                                view,
                                cx,
                                locale_click(i18n::ZH.to_string()),
                            ),
                        )
                        .child(
                            seg(
                                "lang-en",
                                "English",
                                snapshot.locale == i18n::EN,
                                view,
                                cx,
                                locale_click(i18n::EN.to_string()),
                            ),
                        ),
                ),
            ),
        )
        // pi 行为
        .child({
            let mut block = section(i18n::t(cx, "settings_pi_behavior"), cx);
            if !connected {
                block = block.child(hint(i18n::t(cx, "settings_need_session"), cx));
            }
            let s = shell.slot().state.read(cx);
            let auto_compact = s.auto_compaction.unwrap_or(true);
            let auto_retry = s.auto_retry.unwrap_or(true);
            block.child(
                row(
                    i18n::t(cx, "auto_compact"),
                    Some(i18n::t(cx, "auto_compact_detail")),
                    cx,
                )
                .child(switch_row("switch-auto-compact", auto_compact, connected, view)),
            )
            .child(
                row(
                    i18n::t(cx, "auto_retry"),
                    Some(i18n::t(cx, "auto_retry_detail")),
                    cx,
                )
                .child(switch_row("switch-auto-retry", auto_retry, connected, view)),
            )
        })
        // 队列与更新
        .child(
            section(i18n::t(cx, "settings_queue_update"), cx).child(
                row(
                    i18n::t(cx, "settings_queue_default"),
                    Some(i18n::t(cx, "settings_queue_default_detail")),
                    cx,
                )
                .child(
                    div()
                        .flex()
                        .flex_none()
                        .gap_1()
                        .rounded_md()
                        .border_1()
                        .border_color(cx.theme().colors.border)
                        .p_1()
                        .child(
                            seg(
                                "queue-steer",
                                i18n::t(cx, "queued_steer"),
                                snapshot.queue_mode == "steer",
                                view,
                                cx,
                                queue_mode_click("steer".to_string()),
                            ),
                        )
                        .child(
                            seg(
                                "queue-followup",
                                i18n::t(cx, "queued_follow_up"),
                                snapshot.queue_mode == "follow_up",
                                view,
                                cx,
                                queue_mode_click("follow_up".to_string()),
                            ),
                        ),
                ),
            )
            .child(
                row(i18n::t(cx, "settings_update_check"), None, cx).child(
                    switch_row(
                        "switch-update-check",
                        snapshot.update_check,
                        true,
                        view,
                    ),
                ),
            ),
        )
        // 思考等级
        .child(thinking_section(shell, view, cx))
}

/// The three appearance segments. Kept as a helper because the surrounding
/// row builder already holds a `cx` borrow.
fn seg_group_children(
    snapshot: &SettingsSnapshot,
    view: &Entity<PidoApp>,
    cx: &App,
) -> Div {
    div()
        .flex()
        .flex_none()
        .gap_1()
        .rounded_md()
        .border_1()
        .border_color(cx.theme().colors.border)
        .p_1()
        .child(
            seg(
                "theme-dark",
                i18n::t(cx, "appearance_dark"),
                snapshot.theme_pref == ThemePref::Dark,
                view,
                cx,
                theme_click(ThemePref::Dark),
            ),
        )
        .child(
            seg(
                "theme-light",
                i18n::t(cx, "appearance_light"),
                snapshot.theme_pref == ThemePref::Light,
                view,
                cx,
                theme_click(ThemePref::Light),
            ),
        )
        .child(
            seg(
                "theme-system",
                i18n::t(cx, "appearance_system"),
                snapshot.theme_pref == ThemePref::System,
                view,
                cx,
                theme_click(ThemePref::System),
            ),
        )
}

/// Whether the active slot has a pi that can take runtime toggles.
fn live_connection(shell: &PidoApp, cx: &App) -> bool {
    matches!(
        shell.slot().state.read(cx).conn,
        Conn::Live | Conn::Reconnecting { .. }
    )
}

fn theme_click(
    pref: ThemePref,
) -> impl Fn(&mut PidoApp, &mut gpui::Context<PidoApp>) + 'static {
    move |shell, cx| {
        save_setting(|settings| settings.set_theme_pref(pref));
        apply_theme_pref(pref, cx);
        if let Some(snapshot) = shell.settings_snapshot.as_mut() {
            snapshot.theme_pref = pref;
        }
        cx.notify();
    }
}

fn locale_click(
    locale: String,
) -> impl Fn(&mut PidoApp, &mut gpui::Context<PidoApp>) + 'static {
    move |shell, cx| {
        save_setting(|settings| settings.locale = Some(locale.clone()));
        ely_gpui_component::i18n::I18n::set_locale(&locale, cx);
        if let Some(snapshot) = shell.settings_snapshot.as_mut() {
            snapshot.locale = locale.clone();
        }
        cx.notify();
    }
}

fn queue_mode_click(
    mode: String,
) -> impl Fn(&mut PidoApp, &mut gpui::Context<PidoApp>) + 'static {
    move |shell, cx| {
        save_setting(|settings| settings.queue_mode = Some(mode.clone()));
        if let Some(snapshot) = shell.settings_snapshot.as_mut() {
            snapshot.queue_mode = mode.clone();
        }
        cx.notify();
    }
}

fn update_check_click(on: bool) -> impl Fn(&mut PidoApp, &mut gpui::Context<PidoApp>) + 'static {
    move |shell, cx| {
        save_setting(|settings| settings.update_check = Some(on));
        if let Some(snapshot) = shell.settings_snapshot.as_mut() {
            snapshot.update_check = on;
        }
        cx.notify();
    }
}

/// A Switch wired to either a persisted pido preference or the live
/// session's pi-side toggle. `enabled=false` greys it out.
fn switch_row(id: &'static str, on: bool, enabled: bool, view: &Entity<PidoApp>) -> Switch {
    let mut switch = Switch::new(id, on).disabled(!enabled);
    match id {
        "switch-auto-compact" => {
            let view = view.clone();
            switch = switch.on_change(move |on, _, cx| {
                view.update(cx, |shell, cx| {
                    shell.slot().state.update(cx, |s, cx| s.set_auto_compaction(on, cx));
                });
            });
        }
        "switch-auto-retry" => {
            let view = view.clone();
            switch = switch.on_change(move |on, _, cx| {
                view.update(cx, |shell, cx| {
                    shell.slot().state.update(cx, |s, cx| s.set_auto_retry(on, cx));
                });
            });
        }
        "switch-update-check" => {
            let view = view.clone();
            switch = switch.on_change(move |on, _, cx| {
                view.update(cx, |shell, cx| update_check_click(on)(shell, cx));
            });
        }
        _ => {}
    }
    switch
}

/// The thinking-level segments, mirroring the composer's level menu: the
/// levels come from the live session, the click goes through
/// `AppState::set_thinking`.
fn thinking_section(shell: &PidoApp, view: &Entity<PidoApp>, cx: &App) -> Div {
    let colors = &cx.theme().colors;
    let s = shell.slot().state.read(cx);
    let levels = s.thinking_levels.clone();
    let current = s.thinking_level;
    let connected = matches!(s.conn, Conn::Live | Conn::Reconnecting { .. });
    let mut block = section(i18n::t(cx, "thinking"), cx);
    if !connected {
        block = block.child(hint(i18n::t(cx, "settings_need_session"), cx));
    }
    block.child(
        row(i18n::t(cx, "settings_thinking_detail"), None, cx).child(
            div()
                .flex()
                .flex_none()
                .flex_wrap()
                .gap_1()
                .rounded_md()
                .border_1()
                .border_color(colors.border)
                .p_1()
                .children(levels.into_iter().map(|level| {
                    let slug = level_slug(level);
                    seg(
                        gpui::ElementId::Name(SharedString::from(format!("think-{slug}"))),
                        SharedString::from(slug),
                        current == Some(level),
                        view,
                        cx,
                        move |shell, cx| {
                            shell.slot().state.update(cx, |s, cx| s.set_thinking(level, cx));
                        },
                    )
                })),
        ),
    )
}

fn hint(text: impl Into<SharedString>, cx: &App) -> Div {
    let colors = &cx.theme().colors;
    div()
        .text_xs()
        .text_color(colors.fg_subtle)
        .child(text.into().to_string())
}

// ---------------------------------------------------------------------------
// 模型设置（只读）
// ---------------------------------------------------------------------------

fn models_page(shell: &PidoApp, snapshot: &SettingsSnapshot, cx: &App) -> Div {
    let colors = &cx.theme().colors;
    let current = shell
        .slot()
        .state
        .read(cx)
        .current_model
        .as_ref()
        .map(|model| model.name.clone());
    let mut page = div().flex().flex_col().gap_4();

    page = page.child(
        section(i18n::t(cx, "settings_models_current"), cx).child(row(
            current.unwrap_or_else(|| i18n::t(cx, "settings_not_connected").to_string()),
            None,
            cx,
        )),
    );

    // pi's startup defaults, as configured in its settings.json.
    let defaults = [
        (
            i18n::t(cx, "settings_default_provider"),
            snapshot.setting_str("defaultProvider"),
        ),
        (
            i18n::t(cx, "settings_default_model"),
            snapshot.setting_str("defaultModel"),
        ),
        (
            i18n::t(cx, "settings_default_thinking"),
            snapshot.setting_str("defaultThinkingLevel"),
        ),
    ];
    let mut defaults_block = section(i18n::t(cx, "settings_pi_defaults"), cx);
    for (label, value) in defaults {
        defaults_block = defaults_block.child(row(
            label,
            Some(SharedString::from(value.unwrap_or_else(|| "-".to_string()))),
            cx,
        ));
    }
    page = page.child(defaults_block);

    // Provider login status: the union of pido's known providers and every
    // key stored in pi's auth.json.
    let mut providers: Vec<String> = KNOWN_PROVIDERS
        .iter()
        .map(|provider| provider.to_string())
        .chain(snapshot.auth_providers.iter().cloned())
        .collect();
    providers.sort();
    providers.dedup();
    let mut providers_block = section(i18n::t(cx, "settings_providers"), cx);
    for provider in providers {
        let ready = snapshot.ready_providers.iter().any(|ready| ready == &provider);
        let (dot, label) = if ready {
            (colors.success, i18n::t(cx, "provider_ready"))
        } else {
            (colors.fg_subtle, i18n::t(cx, "provider_not_ready"))
        };
        providers_block = providers_block.child(
            row(SharedString::from(provider.clone()), None, cx)
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap_2()
                        .child(
                            div()
                                .size(px(8.))
                                .rounded_full()
                                .bg(dot),
                        )
                        .child(
                            div()
                                .text_xs()
                                .text_color(colors.fg_muted)
                                .child(label.to_string()),
                        ),
                ),
        );
    }
    page = page.child(providers_block);

    // Reveal buttons for the backing files.
    let agent_dir = snapshot
        .agent_dir
        .clone()
        .unwrap_or_else(|| PathBuf::from("."));
    let auth_path = agent_dir.join("auth.json");
    let auth_reveal = if auth_path.is_file() {
        auth_path
    } else {
        agent_dir.clone()
    };
    let models_path = agent_dir.join("models.json");
    let models_reveal = if models_path.is_file() {
        models_path
    } else {
        agent_dir.clone()
    };
    page = page.child(
        div()
            .flex()
            .gap_2()
            .child(
                Button::new("open-auth", i18n::t(cx, "open_auth_config"))
                    .variant(ButtonVariant::Secondary)
                    .on_click(move |_, _, _| crate::state::reveal_in_folder(&auth_reveal)),
            )
            .child(
                Button::new("open-models", i18n::t(cx, "open_models_config"))
                    .variant(ButtonVariant::Secondary)
                    .on_click(move |_, _, _| crate::state::reveal_in_folder(&models_reveal)),
            ),
    );
    page
}

// ---------------------------------------------------------------------------
// Skill 设置（只读）
// ---------------------------------------------------------------------------

fn skills_page(snapshot: &SettingsSnapshot, cx: &App) -> Div {
    let mut page = div().flex().flex_col().gap_4();
    let mut block = section(i18n::t(cx, "settings_installed_skills"), cx);
    if snapshot.skills.is_empty() {
        block = block.child(hint(i18n::t(cx, "settings_no_skills"), cx));
    }
    for skill in &snapshot.skills {
        block = block.child(
            row(
                SharedString::from(skill.name.clone()),
                Some(SharedString::from(skill.description.clone())),
                cx,
            )
            .child(scope_tag(skill.scope, cx)),
        );
    }
    page = page.child(block);

    if let Some(dir) = snapshot
        .agent_dir
        .clone()
        .map(|dir| dir.join("skills"))
        .filter(|dir| dir.is_dir())
    {
        page = page.child(
            div().flex().child(
                Button::new("open-skills", i18n::t(cx, "open_skills_dir"))
                    .variant(ButtonVariant::Secondary)
                    .on_click(move |_, _, _| crate::state::reveal_in_folder(&dir)),
            ),
        );
    }
    page
}

/// 用户/项目 tag for a resource row.
fn scope_tag(scope: Scope, cx: &App) -> Div {
    let colors = &cx.theme().colors;
    div()
        .flex_none()
        .px_2()
        .py_0p5()
        .rounded_md()
        .bg(colors.info_subtle)
        .text_color(colors.info)
        .text_xs()
        .child(
            i18n::t(
                cx,
                match scope {
                    Scope::User => "scope_user",
                    Scope::Project => "scope_project",
                },
            )
            .to_string(),
        )
}

// ---------------------------------------------------------------------------
// 扩展设置（只读）
// ---------------------------------------------------------------------------

fn extensions_page(snapshot: &SettingsSnapshot, cx: &App) -> Div {
    let mut page = div().flex().flex_col().gap_4();

    let mut builtin_block = section(i18n::t(cx, "settings_builtin_extensions"), cx);
    for name in BUILTIN_EXTENSIONS {
        builtin_block = builtin_block.child(
            row(SharedString::from(*name), None, cx).child(builtin_tag(cx)),
        );
    }
    page = page.child(builtin_block);

    let mut configured_block =
        section(i18n::t(cx, "settings_configured_extensions"), cx);
    if snapshot.extensions.is_empty() {
        configured_block = configured_block.child(hint(i18n::t(cx, "settings_no_extensions"), cx));
    }
    for source in &snapshot.extensions {
        configured_block = configured_block.child(
            row(
                SharedString::from(source.source.clone()),
                None,
                cx,
            )
            .child(scope_tag(source.scope, cx)),
        );
    }
    page = page.child(configured_block);

    if let Some(agent_dir) = snapshot.agent_dir.clone() {
        let settings_path = agent_dir.join("settings.json");
        let extensions_dir = agent_dir.join("extensions");
        let mut buttons = div().flex().gap_2().child(
            Button::new("open-ext-config", i18n::t(cx, "open_config_file"))
                .variant(ButtonVariant::Secondary)
                .on_click(move |_, _, _| crate::state::reveal_in_folder(&settings_path)),
        );
        if extensions_dir.is_dir() {
            buttons = buttons.child(
                Button::new("open-ext-dir", i18n::t(cx, "open_extensions_dir"))
                    .variant(ButtonVariant::Secondary)
                    .on_click(move |_, _, _| crate::state::reveal_in_folder(&extensions_dir)),
            );
        }
        page = page.child(buttons);
    }
    page
}

fn builtin_tag(cx: &App) -> Div {
    let colors = &cx.theme().colors;
    div()
        .flex_none()
        .px_2()
        .py_0p5()
        .rounded_md()
        .bg(colors.success_subtle)
        .text_color(colors.success)
        .text_xs()
        .child(i18n::t(cx, "settings_builtin_tag").to_string())
}
