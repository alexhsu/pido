//! pido: a desktop client for the Pi coding agent, built with GPUI and Ely.

mod i18n;
mod state;
mod ui;

use ely_gpui_component::{Assets, theme::{Mode, Theme}};
use gpui::{App, AppContext, Bounds, TitlebarOptions, WindowBounds, WindowOptions, px, size};

fn main() {
    env_logger::init();
    gpui_platform::application()
        .with_assets(Assets)
        .run(|cx: &mut App| {
            ely_gpui_component::init(cx).expect("Ely failed to start");
            Theme::set_mode_now(Mode::Dark, cx);
            i18n::init(cx);

            let bounds = Bounds::centered(None, size(px(1280.0), px(820.0)), cx);
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
                let app = cx.new(|cx| ui::app::PidoApp::new(cx));
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
