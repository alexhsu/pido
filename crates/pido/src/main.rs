use ely_gpui_component::{
    Assets,
    primitives::FocusScope,
    theme::{ActiveTheme, Mode, Theme},
    typography::{Heading, Paragraph},
};
use gpui::{
    App, AppContext, Bounds, Context, FocusHandle, IntoElement, ParentElement, Render, Styled,
    TitlebarOptions, Window, WindowBounds, WindowOptions, px, size,
};

struct Pido {
    focus: FocusHandle,
}

impl Render for Pido {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme();
        FocusScope::new(&self.focus)
            .root()
            .size_full()
            .flex()
            .flex_col()
            .gap_4()
            .p_8()
            .bg(theme.colors.bg)
            .text_color(theme.colors.fg)
            .child(Heading::h1("pido"))
            .child(Paragraph::new(
                "Desktop client for the Pi coding agent. Skeleton is up; the chat UI lands next.",
            ))
    }
}

fn main() {
    env_logger::init();
    gpui_platform::application()
        .with_assets(Assets)
        .run(|cx: &mut App| {
            ely_gpui_component::init(cx).expect("Ely failed to start");
            Theme::set_mode_now(Mode::Dark, cx);
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
                cx.new(|cx| {
                    let focus = cx.focus_handle();
                    window.focus(&focus, cx);
                    Pido { focus }
                })
            })
            .expect("window failed to open");
            cx.activate(true);
        });
}
