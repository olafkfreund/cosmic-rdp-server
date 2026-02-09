mod app;
mod config;
mod i18n;
mod message;
mod pages;

use app::App;

fn main() -> cosmic::iced::Result {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        .init();

    i18n::init();

    cosmic::app::run::<App>(cosmic::app::Settings::default(), ())
}
