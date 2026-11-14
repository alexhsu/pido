//! pido: a desktop client for the Pi coding agent, built with GPUI and Ely.

mod brand;
mod i18n;
mod settings;
mod state;
mod ui;

use ely_gpui_component::{
    Assets,
    theme::{Mode, Theme},
};
use gpui::{App, AppContext, Bounds, TitlebarOptions, WindowBounds, WindowOptions, px, size};

fn main() {
    env_logger::init();
    let settings = settings::Settings::load();
    gpui_platform::application()
        .with_assets(Assets)
        .run(move |cx: &mut App| {
            ely_gpui_component::init(cx).expect("Ely failed to start");
            cx.bind_keys([
                gpui::KeyBinding::new("escape", ui::app::Stop, None),
                // The session menu's chords act on the open session.
                gpui::KeyBinding::new("ctrl-alt-p", ui::app::PinSession, None),
                gpui::KeyBinding::new("ctrl-alt-g", ui::app::RenameSession, None),
                gpui::KeyBinding::new("ctrl-alt-f", ui::app::ForkSession, None),
                gpui::KeyBinding::new("ctrl-shift-a", ui::app::ArchiveSession, None),
                gpui::KeyBinding::new("ctrl-f", ui::app::Find, None),
                gpui::KeyBinding::new("ctrl-k", ui::app::Palette, None),
            ]);
            // The terminal's own chords (copy/paste/clear/find) are scoped to
            // the terminal's focus, so they sit alongside the app's.
            ely_gpui_component::terminal::bind_keys(cx);
            match settings.theme.as_deref() {
                Some("light") => Theme::set_mode_now(Mode::Light, cx),
                _ => Theme::set_mode_now(Mode::Dark, cx),
            }
            let start_locale = settings
                .locale
                .clone()
                .unwrap_or_else(|| i18n::ZH.to_string());
            i18n::init(cx, &start_locale);

            let (width, height) = settings
                .window
                .map(|w| (w.width, w.height))
                .unwrap_or((1280.0, 820.0));
            let bounds = Bounds::centered(None, size(px(width), px(height)), cx);
            let options = WindowOptions {
                window_bounds: Some(WindowBounds::Windowed(bounds)),
                titlebar: Some(TitlebarOptions {
                    title: Some("pido".into()),
                    ..Default::default()
                }),
                window_min_size: Some(size(px(880.0), px(560.0))),
                ..Default::default()
            };
            cx.open_window(options, |window, cx| {
                let app = cx.new(ui::app::PidoApp::new);
                app.update(cx, |app, cx| app.setup(window, cx));
                app.update(cx, |app, cx| app.autoconnect(cx));
                let focus = app.read(cx).focus.clone();
                window.focus(&focus, cx);
                app
            })
            .expect("window failed to open");
            cx.activate(true);
        });
}
