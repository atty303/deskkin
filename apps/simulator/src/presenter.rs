use deskkin_application::{ApplicationViews, availability, synthetic_notice};

use crate::StatusWindow;

pub(crate) fn apply_view(ui: &StatusWindow, views: ApplicationViews) {
    let (text, color) = match views.availability {
        None | Some(availability::Surface::Unknown) => {
            ("Unknown", slint::Color::from_rgb_u8(243, 179, 61))
        }
        Some(availability::Surface::Available) => {
            ("Available", slint::Color::from_rgb_u8(61, 214, 140))
        }
        Some(availability::Surface::Unavailable) => {
            ("Unavailable", slint::Color::from_rgb_u8(242, 95, 92))
        }
    };
    let (notice_visible, notice_text) = match views.synthetic_notice {
        None => (false, ""),
        Some(synthetic_notice::NoticeKind::CompositionCheck) => (true, "Deskkin notice"),
    };
    ui.set_status_text(text.into());
    ui.set_status_color(color);
    ui.set_notice_visible(notice_visible);
    ui.set_notice_text(notice_text.into());
}
