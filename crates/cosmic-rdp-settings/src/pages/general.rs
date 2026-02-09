use cosmic::iced::Length;
use cosmic::widget::{self, settings};
use cosmic::Element;

use crate::fl;
use crate::message::Message;

/// Render the General settings page.
#[allow(clippy::too_many_arguments)]
pub fn view<'a>(
    bind_address: &'a str,
    port: &'a str,
    static_display: bool,
    server_running: bool,
    active_connections: u32,
    bound_address: &'a str,
) -> Element<'a, Message> {
    let status_label = if server_running {
        fl!("general-status-running")
    } else {
        fl!("general-status-stopped")
    };

    let mut content = widget::column()
        .spacing(16)
        .width(Length::Fill)
        .push(
            settings::section()
                .title(fl!("general-status"))
                .add(settings::item(
                    fl!("general-server-toggle"),
                    widget::toggler(server_running)
                        .on_toggle(Message::ToggleServer),
                ))
                .add(settings::item_row(vec![
                    widget::text::body(format!("{}: {status_label}", fl!("general-status"))).into(),
                ]))
                .add(settings::item_row(vec![
                    widget::text::body(format!(
                        "{}: {active_connections}",
                        fl!("general-connections")
                    ))
                    .into(),
                ])),
        );

    let mut network_section = settings::section()
        .title(fl!("general-bind-address"))
        .add(settings::item(
            fl!("general-bind-address"),
            widget::text_input("0.0.0.0", bind_address)
                .on_input(Message::BindAddress)
                .width(Length::Fixed(200.0)),
        ))
        .add(settings::item(
            fl!("general-port"),
            widget::text_input("3389", port)
                .on_input(Message::Port)
                .width(Length::Fixed(100.0)),
        ))
        .add(settings::item(
            fl!("general-static-display"),
            widget::toggler(static_display)
                .on_toggle(Message::StaticDisplay),
        ));

    if !bound_address.is_empty() {
        network_section = network_section.add(settings::item_row(vec![
            widget::text::body(format!("{}: {bound_address}", fl!("general-listening")))
                .into(),
        ]));
    }

    content = content.push(network_section);

    content = content.push(
        widget::row()
            .spacing(8)
            .push(
                widget::button::standard(fl!("general-apply"))
                    .on_press(Message::Apply),
            )
            .push(
                widget::button::standard(fl!("general-reset"))
                    .on_press(Message::Reset),
            ),
    );

    content.into()
}
