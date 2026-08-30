use deskkin_application::{ApplicationView, availability, synthetic_notice};

use crate::StatusWindow;

pub(crate) fn apply_view(ui: &StatusWindow, view: ApplicationView) {
    let (text, color, notice_visible, notice_text) = match view {
        ApplicationView::Empty | ApplicationView::Availability(availability::Surface::Unknown) => (
            "Unknown",
            slint::Color::from_rgb_u8(243, 179, 61),
            false,
            "",
        ),
        ApplicationView::Availability(availability::Surface::Available) => (
            "Available",
            slint::Color::from_rgb_u8(61, 214, 140),
            false,
            "",
        ),
        ApplicationView::Availability(availability::Surface::Unavailable) => (
            "Unavailable",
            slint::Color::from_rgb_u8(242, 95, 92),
            false,
            "",
        ),
        ApplicationView::SyntheticNotice(synthetic_notice::NoticeKind::CompositionCheck) => (
            "Unknown",
            slint::Color::from_rgb_u8(243, 179, 61),
            true,
            "Deskkin notice",
        ),
    };
    ui.set_status_text(text.into());
    ui.set_status_color(color);
    ui.set_notice_visible(notice_visible);
    ui.set_notice_text(notice_text.into());
}
