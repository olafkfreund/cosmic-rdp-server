use cosmic::iced::Length;
use cosmic::widget::{self, settings};
use cosmic::Element;

use crate::fl;
use crate::message::Message;

/// Render the Security settings page.
#[allow(clippy::too_many_arguments)]
pub fn view<'a>(
    cert_path: &'a str,
    key_path: &'a str,
    nla_enable: bool,
    nla_username: &'a str,
    nla_password: &'a str,
    nla_domain: &'a str,
) -> Element<'a, Message> {
    let mut content = widget::column()
        .spacing(16)
        .width(Length::Fill)
        .push(
            settings::section()
                .title(fl!("security-tls"))
                .add(settings::item(
                    fl!("security-cert-path"),
                    widget::text_input("", cert_path)
                        .on_input(Message::CertPath)
                        .width(Length::Fixed(350.0)),
                ))
                .add(settings::item(
                    fl!("security-key-path"),
                    widget::text_input("", key_path)
                        .on_input(Message::KeyPath)
                        .width(Length::Fixed(350.0)),
                ))
                .add(settings::item_row(vec![
                    widget::text::caption(fl!("security-self-signed")).into(),
                ])),
        )
        .push(
            settings::section()
                .title(fl!("security-nla"))
                .add(settings::item(
                    fl!("security-nla-enable"),
                    widget::toggler(nla_enable)
                        .on_toggle(Message::NlaEnable),
                )),
        );

    if nla_enable {
        content = content.push(
            settings::section()
                .add(settings::item(
                    fl!("security-username"),
                    widget::text_input("", nla_username)
                        .on_input(Message::NlaUsername)
                        .width(Length::Fixed(250.0)),
                ))
                .add(settings::item(
                    fl!("security-password"),
                    widget::secure_input("", nla_password, None, true)
                        .on_input(Message::NlaPassword)
                        .width(Length::Fixed(250.0)),
                ))
                .add(settings::item(
                    fl!("security-domain"),
                    widget::text_input("", nla_domain)
                        .on_input(Message::NlaDomain)
                        .width(Length::Fixed(250.0)),
                )),
        );
    }

    content = content.push(super::action_buttons());

    content.into()
}
