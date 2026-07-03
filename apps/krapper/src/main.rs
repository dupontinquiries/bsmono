mod app;
mod audio;
mod dict;
mod scale;
mod session;
mod vibe;
mod vocab;

use app::App;

fn main() -> iced::Result {
    iced::application("kRapper — hum it, rap it", App::update, App::view)
        .subscription(App::subscription)
        .theme(App::theme)
        .window_size(iced::Size::new(860.0, 640.0))
        .run_with(App::new)
}
