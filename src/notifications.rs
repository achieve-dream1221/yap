use std::{
    sync::mpsc::Sender,
    time::{Duration, Instant},
};

use ratatui::{
    prelude::*,
    widgets::{Block, Borders, Clear},
};
use ratatui_macros::horizontal;
use tracing::debug;

use crate::app::{Event, Tick};

pub struct Notifications {
    pub inner: Option<Notification>,
    replaced: (usize, Option<String>),
    tx: Sender<Event>,
}

#[derive(Debug)]
pub struct Notification {
    pub text: String,
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
            replaced: (0, None),
            tx,
        }
    }
    pub fn notify<S: AsRef<str>>(&mut self, text: S, color: Color) {
        let text: &str = text.as_ref();
        debug!("Notification: \"{text}\", Color: {color}");
        if text.is_empty() {
            return;
        }
        self.replaced = if self.inner.is_none() {
            (0, None)
        } else {
            let amount = self.replaced.0 + 1;
            let text = format!("[+{amount}]");
            (amount, Some(text))
        };
        self.inner = Some(Notification {
            text: text.to_owned(),
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

// TODO make reactive to text width + popup area

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
            let mut new_area = area.clone();
            new_area.height = 2;
            // new_area.width = area.width.saturating_div(2);
            let [_, new_area, _] = if area.width > MIN_NOTIF_WIDTH {
                horizontal![*=2,*=5,*=2].areas(new_area)
            } else {
                horizontal![*=1,*=5,*=1].areas(new_area)
            };
            // new_area.x += (area.width.saturating_sub(new_area.width)) / 2;
            new_area
        };
        let expand_area = {
            let mut new_area = center_area.clone();
            new_area.width = center_area.width + 2;
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
            let block = Block::new()
                .borders(Borders::BOTTOM | Borders::LEFT | Borders::RIGHT)
                .border_style(Style::from(notification.color));

            let inner_area = block.inner(center_area);
            let text = Line::raw(&notification.text).centered();
            if notification.replaced
                && text.width() < inner_area.width as usize
                && self.replaced.0 > 0
            {
                let replaced_amount_text =
                    Line::raw(self.replaced.1.as_ref().unwrap()).right_aligned();
                replaced_amount_text.render(inner_area, buf);
            }
            text.render(inner_area, buf);

            block.render(block_area, buf);
        }
    }
}
