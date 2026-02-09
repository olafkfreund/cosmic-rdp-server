use cosmic::iced::Length;
use cosmic::widget::{self, settings};
use cosmic::Element;

use crate::fl;
use crate::message::Message;

/// Encoder backend options for the dropdown.
pub const ENCODER_OPTIONS: &[&str] = &["auto", "vaapi", "nvenc", "software"];

/// Render the Display settings page.
#[allow(clippy::too_many_arguments)]
pub fn view<'a>(
    fps: &'a str,
    buffer_capacity: &'a str,
    multi_monitor: bool,
    encoder_idx: usize,
    preset: &'a str,
    bitrate_mbps: &'a str,
    encoder_labels: &'a [String],
) -> Element<'a, Message> {

    let content = widget::column()
        .spacing(16)
        .width(Length::Fill)
        .push(
            settings::section()
                .title(fl!("display-fps"))
                .add(settings::item(
                    fl!("display-fps"),
                    widget::text_input("30", fps)
                        .on_input(Message::Fps)
                        .width(Length::Fixed(80.0)),
                ))
                .add(settings::item(
                    fl!("display-buffer"),
                    widget::text_input("4", buffer_capacity)
                        .on_input(Message::BufferCapacity)
                        .width(Length::Fixed(80.0)),
                ))
                .add(settings::item(
                    fl!("display-multi-monitor"),
                    widget::toggler(multi_monitor)
                        .on_toggle(Message::MultiMonitor),
                )),
        )
        .push(
            settings::section()
                .title(fl!("display-encoder"))
                .add(settings::item(
                    fl!("display-encoder"),
                    widget::dropdown(
                        encoder_labels,
                        Some(encoder_idx),
                        Message::Encoder,
                    ),
                ))
                .add(settings::item(
                    fl!("display-preset"),
                    widget::text_input("ultrafast", preset)
                        .on_input(Message::Preset)
                        .width(Length::Fixed(150.0)),
                ))
                .add(settings::item(
                    fl!("display-bitrate"),
                    widget::text_input("10", bitrate_mbps)
                        .on_input(Message::Bitrate)
                        .width(Length::Fixed(80.0)),
                )),
        )
        .push(
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
