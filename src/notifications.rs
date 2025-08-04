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

/// Very simple helper for showing toast notifications on-screen.
///
/// TODO: Scrollable notification history, with timestamps and level(?).
pub struct Notifications {
    pub inner: Option<ToastContent>,
    replaced_amount: (usize, Option<CompactString>),
    tx: Sender<Event>,
}

#[derive(Debug)]
pub struct ToastContent {
    pub line: Line<'static>,
    pub color: Color,
    pub shown_at: Instant,
    pub replaced: bool,
}

impl ToastContent {
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
        self.inner = Some(ToastContent {
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

/// Max time a toast should be visible (including transitions!).
pub const EXPIRE_TIME: Duration = Duration::from_millis(3150);
/// Time to show toast on screen.
pub const PAUSE_AND_SHOW_TIME: Duration = Duration::from_millis(3000);
/// For when another notification is sent while a toast is
/// already being shown, overriding it, how long should
/// the new toast stay in the "expanded" state.
pub const EXPAND_TIME: Duration = Duration::from_millis(250);
/// How long toast should take to emerge if none was present before.
pub const EMERGE_TIME: Duration = Duration::from_millis(75);

/// If the screen width is greater than this value, toasts will be centered.
///
/// If the screen width is less than or equal to this value,
/// the toast will span the full width of the top few rows.
pub const MIN_NOTIF_WIDTH: u16 = 70;

impl Widget for &Notifications {
    fn render(self, area: Rect, buf: &mut Buffer)
    where
        Self: Sized,
    {
        let Some(toast) = &self.inner else {
            return;
        };

        let center_area = {
            let mut top_lines = area;
            top_lines.height = 2;
            // new_area.width = area.width.saturating_div(2);
            let [_, centered_area, _] = if area.width > MIN_NOTIF_WIDTH {
                horizontal![*=2,*=5,*=2].areas(top_lines)
            } else {
                horizontal![*=1,*=5,*=1].areas(top_lines)
            };
            let mut new_width = centered_area.width.max(toast.line.width() as u16 + 4);
            new_width = new_width.min(area.width);
            let mut new_area = centered_area;
            new_area.width = new_width;
            if centered_area.width != new_width {
                new_area.x = (area.width.saturating_sub(new_width)) / 2;
            }
            new_area
        };
        let expand_area = {
            let mut new_area = center_area;
            if new_area.width != area.width {
                new_area.width = center_area.width + 2;
            }
            new_area.height = 3;
            new_area.x = new_area.x.saturating_sub(1);
            // new_area.x += (area.width.saturating_sub(new_area.width)) / 2;
            new_area
        };

        let (toast_height, expand) = match (toast.shown_for(), toast.replaced) {
            // Any toast: if longer than expire time, hide.
            (d, _) if d >= EXPIRE_TIME => return,
            // Any toast: if longer than time to show, begin "sliding" back into top.
            (d, _) if d >= PAUSE_AND_SHOW_TIME => (1, false),

            // Normal toast: if longer than emerge time, then show full toast.
            (d, false) if d >= EMERGE_TIME => (2, false),
            // Normal toast: if under emerge time, show only bottom line of toast.
            (_, false) => (1, false),

            // Replacement toast: if longer than expand time, then show full toast.
            (d, true) if d >= EXPAND_TIME => (2, false),
            // Replacement toast: if under expand time, use expanded area (full toast + 1 width + 1 height)
            (_, true) => (0, true),
        };

        let block_area = {
            let mut area = if toast.replaced && expand {
                expand_area
            } else {
                center_area
            };
            if !expand {
                area.height = toast_height;
            }
            area
        };

        if block_area.height > 0 {
            Clear::render(Clear, block_area, buf);
            let mut block = Block::new()
                .borders(Borders::BOTTOM)
                .border_style(Style::from(toast.color));

            if area.width.saturating_sub(center_area.width) >= 2 {
                block = block.borders(Borders::BOTTOM | Borders::LEFT | Borders::RIGHT);
            }

            let inner_area = block.inner(center_area);
            let text = &toast.line;
            if toast.replaced
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
