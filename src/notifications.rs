use std::time::{Duration, Instant};

use ratatui::{
    prelude::*,
    widgets::{Block, Borders, Clear},
};
use tracing::debug;

#[derive(Default)]
pub struct Notifications {
    pub inner: Option<Notification>,
}

#[derive(Debug)]
pub struct Notification {
    text: String,
    color: Color,
    shown_at: Instant,
    // animating: bool,
    replaced: bool,
}

impl Notification {
    pub fn shown_for(&self) -> Duration {
        self.shown_at.elapsed()
    }
}

impl Notifications {
    pub fn notify<S: AsRef<str>>(&mut self, text: S, color: Color) {
        let text: &str = text.as_ref();
        debug!("Notification: {text}, Color: {color}");
        self.inner = Some(Notification {
            text: text.to_owned(),
            color,
            shown_at: Instant::now(),
            // animating: true,
            replaced: self.inner.is_some(),
        })
    }
    pub fn is_some(&self) -> bool {
        self.inner.is_some()
    }

    pub fn is_none(&self) -> bool {
        self.inner.is_none()
    }
}

impl Widget for &Notifications {
    fn render(self, area: Rect, buf: &mut Buffer)
    where
        Self: Sized,
    {
        let Some(notification) = &self.inner else {
            return;
        };

        let center_area = {
            let mut new_area = area.clone();
            new_area.width = area.width.saturating_div(2);
            new_area.height = 2;
            new_area.x += (area.width.saturating_sub(new_area.width)) / 2;
            new_area
        };
        let expand_area = {
            let mut new_area = area.clone();
            new_area.width = area.width.saturating_div(2) + 2;
            new_area.height = 3;
            new_area.x += (area.width.saturating_sub(new_area.width)) / 2;
            new_area
        };

        let (meow_height, expand) = match (notification.shown_for(), notification.replaced) {
            // (d, true) if d >= Duration::from_millis(200) => (0, false),
            // (d, true) if d >= Duration::from_millis(200) => (0, false),
            (d, _) if d >= Duration::from_millis(3150) => (0, false),
            (d, _) if d >= Duration::from_millis(3000) => (1, false),

            (d, true) if d >= Duration::from_millis(250) => (2, false),
            (_, true) => (0, true),

            (d, false) if d >= Duration::from_millis(75) => (2, true),
            (_, false) => (1, false),
            // d if d >= Duration::from_millis(75) => 1,
            // _ => 0,
        };

        let block_area = {
            let mut area = if notification.replaced && expand {
                expand_area.clone()
            } else {
                center_area.clone()
            };
            if !expand {
                area.height = meow_height;
            }
            area
        };

        if block_area.height > 0 {
            Clear::render(Clear, block_area, buf);
            let block = Block::new()
                .borders(Borders::BOTTOM | Borders::LEFT | Borders::RIGHT)
                .border_style(Style::from(notification.color));

            let inner_area = block.inner(center_area);
            let text = Line::raw(&notification.text).centered();
            text.render(inner_area, buf);

            block.render(block_area, buf);
        }
    }
}
