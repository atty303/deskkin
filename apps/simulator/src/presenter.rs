use application_core::StatusView;

use crate::StatusWindow;

pub(crate) fn apply_view(ui: &StatusWindow, view: StatusView) {
    let (text, color) = match view {
        StatusView::Unknown => ("Unknown", slint::Color::from_rgb_u8(243, 179, 61)),
        StatusView::Available => ("Available", slint::Color::from_rgb_u8(61, 214, 140)),
        StatusView::Unavailable => ("Unavailable", slint::Color::from_rgb_u8(242, 95, 92)),
    };
    ui.set_status_text(text.into());
    ui.set_status_color(color);
}
