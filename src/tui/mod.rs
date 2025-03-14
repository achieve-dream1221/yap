use ratatui::layout::{Rect, Size};

pub mod port_settings;
pub mod prompts;
pub mod single_line_selector;

/// Returns a `Rect` with the provided percentage of the parent `Rect` and centered.
pub fn centered_rect_ratio(percent_x: u16, percent_y: u16, parent: Rect) -> Rect {
    let width = parent.width * percent_x / 100;
    let height = parent.height * percent_y / 100;
    Rect {
        width,
        height,
        x: (parent.width - width) / 2,
        y: (parent.height - height) / 2,
    }
}

/// Returns a `Rect` with the provided percentage of the parent `Rect` and centered.
pub fn centered_rect_size(size: Size, parent: Rect) -> Rect {
    Rect {
        width: size.width,
        height: size.height,
        x: (parent.width.saturating_sub(size.width)) / 2,
        y: (parent.height.saturating_sub(size.height)) / 2,
    }
}
