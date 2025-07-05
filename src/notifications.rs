use std::{
    borrow::Cow,
    time::{Duration, Instant},
};

use compact_str::{CompactString, format_compact};
use crossbeam::channel::Sender;
use ratatui::{
    prelude::*,
    widgets::{Block, Borders, Clear},
};
use ratatui_macros::horizontal;
use tracing::debug;

use crate::app::{Event, Tick};

pub struct Notifications {
    pub inner: Option<Notification>,
    replaced_amount: (usize, Option<CompactString>),
    tx: Sender<Event>,
}

#[derive(Debug)]
pub struct Notification {
    pub line: Line<'static>,
    pub color: Color,
    pub shown_at: Instant,
    pub replaced: bool,
}

impl Notification {
    pub fn shown_for(&self) -> Duration {
        self.shown_at.elapsed()
    }
}

impl Notifications {
    pub fn new(tx: Sender<Event>) -> Self {
        Self {
            inner: None,
            replaced_amount: (0, None),
            tx,
        }
    }
    pub fn notify(&mut self, line: Line<'static>, color: Color) {
        self.notify_inner(line.centered(), color);
    }
    pub fn notify_str<S: AsRef<str>>(&mut self, text: S, color: Color) {
        let text: &str = text.as_ref();
        let line: Line = Span::raw(Cow::Owned(text.to_string())).into_centered_line();
        self.notify_inner(line, color);
    }
    fn notify_inner(&mut self, line: Line<'static>, color: Color) {
        debug!("Notification: \"{line}\", Color: {color}");
        self.replaced_amount = if self.inner.is_none() {
            (0, None)
        } else {
            let amount = self.replaced_amount.0 + 1;
            let text = format_compact!("[+{amount}]");
            (amount, Some(text))
        };
        self.inner = Some(Notification {
            line,
            color,
            shown_at: Instant::now(),
            replaced: self.inner.is_some(),
        });
        self.tx.send(Tick::Notification.into()).unwrap();
    }
    pub fn is_some(&self) -> bool {
        self.inner.is_some()
    }

    pub fn is_none(&self) -> bool {
        self.inner.is_none()
    }
}

pub const EXPIRE_TIME: Duration = Duration::from_millis(3150);
pub const PAUSE_TIME: Duration = Duration::from_millis(3000);
pub const EXPAND_TIME: Duration = Duration::from_millis(250);
pub const EMERGE_TIME: Duration = Duration::from_millis(75);

pub const MIN_NOTIF_WIDTH: u16 = 70;

impl Widget for &Notifications {
    fn render(self, area: Rect, buf: &mut Buffer)
    where
        Self: Sized,
    {
        let Some(notification) = &self.inner else {
            return;
        };

        let center_area = {
            let mut top_lines = area.clone();
            top_lines.height = 2;
            // new_area.width = area.width.saturating_div(2);
            let [_, centered_area, _] = if area.width > MIN_NOTIF_WIDTH {
                horizontal![*=2,*=5,*=2].areas(top_lines)
            } else {
                horizontal![*=1,*=5,*=1].areas(top_lines)
            };
            let mut new_width = centered_area
                .width
                .max(notification.line.width() as u16 + 4);
            new_width = new_width.min(area.width);
            let mut new_area = centered_area;
            new_area.width = new_width;
            if centered_area.width != new_width {
                new_area.x = (area.width.saturating_sub(new_width)) / 2;
            }
            new_area
        };
        let expand_area = {
            let mut new_area = center_area.clone();
            if new_area.width != area.width {
                new_area.width = center_area.width + 2;
            }
            new_area.height = 3;
            new_area.x = new_area.x.saturating_sub(1);
            // new_area.x += (area.width.saturating_sub(new_area.width)) / 2;
            new_area
        };

        let (meow_height, expand) = match (notification.shown_for(), notification.replaced) {
            (d, _) if d >= EXPIRE_TIME => (0, false),
            (d, _) if d >= PAUSE_TIME => (1, false),

            (d, true) if d >= EXPAND_TIME => (2, false),
            (_, true) => (0, true),

            (d, false) if d >= EMERGE_TIME => (2, true),
            (_, false) => (1, false),
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
            let mut block = Block::new()
                .borders(Borders::BOTTOM)
                .border_style(Style::from(notification.color));

            if area.width.saturating_sub(center_area.width) >= 2 {
                block = block.borders(Borders::BOTTOM | Borders::LEFT | Borders::RIGHT);
            }

            let inner_area = block.inner(center_area);
            let text = &notification.line;
            if notification.replaced
                && text.width() < inner_area.width as usize
                && self.replaced_amount.0 > 0
            {
                let replaced_amount_text =
                    Line::raw(self.replaced_amount.1.as_ref().unwrap()).right_aligned();
                replaced_amount_text.render(inner_area, buf);
            }
            text.render(inner_area, buf);

            block.render(block_area, buf);
        }
    }
}
