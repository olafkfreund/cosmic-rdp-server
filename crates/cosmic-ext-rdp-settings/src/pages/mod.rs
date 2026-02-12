pub mod display;
pub mod features;
pub mod general;
pub mod security;

use cosmic::widget;
use cosmic::Element;

use crate::fl;
use crate::message::Message;

/// Render the shared Apply / Reset action buttons row.
pub fn action_buttons<'a>() -> Element<'a, Message> {
    widget::row()
        .spacing(8)
        .push(
            widget::button::standard(fl!("general-apply"))
                .on_press(Message::Apply),
        )
        .push(
            widget::button::standard(fl!("general-reset"))
                .on_press(Message::Reset),
        )
        .into()
}
