use cosmic::iced::Length;
use cosmic::widget::{self, settings};
use cosmic::Element;

use crate::fl;
use crate::message::Message;

/// Sample rate options for the dropdown.
pub const SAMPLE_RATE_OPTIONS: &[u32] = &[44100, 48000];

/// Channel options for the dropdown.
pub const CHANNEL_OPTIONS: &[u16] = &[1, 2];

/// Render the Features settings page.
#[allow(clippy::too_many_arguments)]
pub fn view<'a>(
    clipboard_enable: bool,
    audio_enable: bool,
    sample_rate_idx: usize,
    channels_idx: usize,
    sample_rate_labels: &'a [String],
    channel_labels: &'a [String],
) -> Element<'a, Message> {

    let mut content = widget::column()
        .spacing(16)
        .width(Length::Fill)
        .push(
            settings::section()
                .title(fl!("features-clipboard"))
                .add(settings::item(
                    fl!("features-clipboard-enable"),
                    widget::toggler(clipboard_enable)
                        .on_toggle(Message::ClipboardEnable),
                )),
        )
        .push(
            settings::section()
                .title(fl!("features-audio"))
                .add(settings::item(
                    fl!("features-audio-enable"),
                    widget::toggler(audio_enable)
                        .on_toggle(Message::AudioEnable),
                )),
        );

    if audio_enable {
        content = content.push(
            settings::section()
                .add(settings::item(
                    fl!("features-sample-rate"),
                    widget::dropdown(
                        sample_rate_labels,
                        Some(sample_rate_idx),
                        Message::SampleRate,
                    ),
                ))
                .add(settings::item(
                    fl!("features-channels"),
                    widget::dropdown(
                        channel_labels,
                        Some(channels_idx),
                        Message::Channels,
                    ),
                )),
        );
    }

    content = content.push(super::action_buttons());

    content.into()
}
