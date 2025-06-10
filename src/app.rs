use std::{
    borrow::Cow,
    collections::VecDeque,
    i32,
    sync::mpsc::{Receiver, Sender},
    thread::JoinHandle,
    time::{Duration, Instant},
};

use arboard::Clipboard;
use bstr::ByteVec;
use color_eyre::{eyre::Result, owo_colors::OwoColorize};
use compact_str::{CompactString, ToCompactString};
use crokey::{KeyCombination, key};
use enum_rotate::EnumRotate;
use itertools::Itertools;
use ratatui::{
    Frame, Terminal,
    crossterm::event::{KeyCode, KeyEvent, KeyModifiers},
    layout::{Constraint, Layout, Margin, Offset, Rect, Size},
    prelude::Backend,
    style::{Color, Modifier, Style, Stylize},
    text::{Line, Span, Text, ToLine, ToSpan},
    widgets::{
        Block, Borders, Clear, HighlightSpacing, Paragraph, Row, Scrollbar, ScrollbarOrientation,
        ScrollbarState, Table, TableState, Widget, Wrap,
    },
};
use ratatui_macros::{horizontal, line, span, vertical};
use serialport::{SerialPortInfo, SerialPortType};
use struct_table::{ArrowKey, StructTable};
use strum::{VariantArray, VariantNames};
use takeable::Takeable;
use tinyvec::tiny_vec;
use tracing::{debug, error, info, instrument, warn};
use tui_big_text::{BigText, PixelSize};
use tui_input::{Input, StateChanged, backend::crossterm::EventHandler};
use unicode_width::UnicodeWidthStr;

use crate::{
    buffer::Buffer,
    event_carousel::{self, CarouselHandle},
    history::{History, UserInput},
    keybinds::{Action, AppAction, BaseAction, Keybinds, PortAction, ShowPopupAction},
    notifications::{
        EMERGE_TIME, EXPAND_TIME, EXPIRE_TIME, Notification, Notifications, PAUSE_TIME,
    },
    serial::{
        PrintablePortInfo, ReconnectType, Reconnections, SerialDisconnectReason, SerialEvent,
        handle::SerialHandle,
        worker::{InnerPortStatus, MOCK_PORT_NAME},
    },
    settings::{Behavior, PortSettings, Rendering, Settings},
    traits::{FirstChars, LastIndex, LineHelpers, ToggleBool},
    tui::{
        centered_rect_size,
        prompts::{AttemptReconnectPrompt, DisconnectPrompt, PromptTable, centered_rect},
        single_line_selector::{SingleLineSelector, SingleLineSelectorState, StateBottomed},
    },
};

#[cfg(feature = "macros")]
use crate::macros::{MacroNameTag, Macros};

#[cfg(feature = "macros")]
use crate::keybinds::MacroAction;

#[cfg(feature = "logging")]
use crate::settings::Logging;
#[cfg(feature = "logging")]
use crate::tui::logging::toggle_logging_button;
#[cfg(feature = "logging")]
use crate::{buffer::LoggingEvent, keybinds::LoggingAction};

#[cfg(feature = "espflash")]
use crate::keybinds::EspAction;
#[cfg(feature = "espflash")]
use crate::serial::esp::{EspEvent, EspRestartType};
#[cfg(feature = "espflash")]
use crate::tui::esp::espflash_buttons;
#[cfg(feature = "espflash")]
use crate::tui::esp::{self, EspFlashState};

#[derive(Clone, Debug)]
pub enum CrosstermEvent {
    Resize,
    KeyPress(KeyEvent),
    MouseScroll { up: bool },
    RightClick,
}

impl From<CrosstermEvent> for Event {
    fn from(value: CrosstermEvent) -> Self {
        Self::Crossterm(value)
    }
}

#[derive(Debug)]
pub enum Event {
    Crossterm(CrosstermEvent),
    Serial(SerialEvent),
    #[cfg(feature = "logging")]
    Logging(LoggingEvent),
    Tick(Tick),
    Quit,
}

#[derive(Clone, Debug)]
pub enum Tick {
    /// Sent every second
    PerSecond,
    /// When just trying to update the UI a little early
    ///
    /// `&str` has the origin of the update request
    Requested(&'static str),
    /// Used to twiddle repeating_line_flip for each transmission
    Tx,
    /// Used to periodically scroll long text for UIs
    Scroll,
    /// Used to force UI updates when a notification is on screen
    Notification,
    /// Used to trigger consumption of the Action Queue
    Action,
}

impl From<Tick> for Event {
    fn from(value: Tick) -> Self {
        Self::Tick(value)
    }
}

#[derive(Debug, Clone, Copy)]
pub enum Menu {
    PortSelection(PortSelectionElement),
    Terminal(TerminalPrompt),
}

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub enum TerminalPrompt {
    #[default]
    None,
    DisconnectPrompt,
    AttemptReconnectPrompt,
}

impl From<TerminalPrompt> for Menu {
    fn from(value: TerminalPrompt) -> Self {
        Self::Terminal(value)
    }
}

#[derive(Debug, Default, Clone, Copy, EnumRotate)]
pub enum PortSelectionElement {
    #[default]
    Ports,
    BaudSelection,
    CustomBaud,
    MoreOptions,
}

impl From<PortSelectionElement> for Menu {
    fn from(value: PortSelectionElement) -> Self {
        Self::PortSelection(value)
    }
}

#[derive(Debug, Default, PartialEq, Eq)]
pub enum RunningState {
    #[default]
    Running,
    Finished,
}

#[derive(Debug, PartialEq, Eq, EnumRotate, VariantArray, VariantNames)]
#[repr(u8)]
#[strum(serialize_all = "title_case")]
pub enum PopupMenu {
    PortSettings,
    RenderingSettings,
    BehaviorSettings,
    #[cfg(feature = "logging")]
    Logging,
    #[cfg(feature = "espflash")]
    #[strum(serialize = "ESP32 Flashing")]
    EspFlash,
    #[cfg(feature = "macros")]
    Macros,
}

// 0 is for a custom baud rate
pub const COMMON_BAUD: &[u32] = &[
    4800, 9600, 19200, 38400, 57600, 74880, 115200, 230400, 460800, 921600, 0,
];
const COMMON_BAUD_DEFAULT: usize = 6;

pub const DEFAULT_BAUD: u32 = {
    let baud = COMMON_BAUD[COMMON_BAUD_DEFAULT];
    assert!(baud == 115200);
    baud
};

const FAILED_SEND_VISUAL_TIME: Duration = Duration::from_millis(750);

// Maybe have the buffer in a TUI struct?

pub struct App {
    state: RunningState,
    menu: Menu,

    tx: Sender<Event>,
    rx: Receiver<Event>,

    table_state: TableState,
    baud_selection_state: SingleLineSelectorState,
    popup: Option<PopupMenu>,
    popup_menu_item: usize,
    popup_hint_scroll: i32,
    popup_table_state: TableState,
    // popup_scrollbar_state: ScrollbarState,
    // popup_single_line_state: SingleLineSelectorState,
    notifs: Notifications,
    ports: Vec<SerialPortInfo>,
    serial: SerialHandle,
    serial_thread: Takeable<JoinHandle<()>>,

    carousel: CarouselHandle,
    carousel_thread: Takeable<JoinHandle<()>>,

    user_input: UserInput,

    buffer: Buffer,
    // Tempted to move these into Buffer, or a new BufferState
    // buffer_scroll: usize,
    // buffer_scroll_state: ScrollbarState,
    // buffer_stick_to_bottom: bool,
    // buffer_wrapping: bool,
    // Filled in while drawing UI
    // buffer_rendered_lines: usize,
    repeating_line_flip: bool,
    failed_send_at: Option<Instant>,

    #[cfg(feature = "macros")]
    macros: Macros,
    action_queue: VecDeque<(Option<KeyCombination>, Action)>,

    user_broke_connection: bool,
    settings: Settings,
    scratch: Settings,
    keybinds: Keybinds,

    #[cfg(feature = "espflash")]
    espflash: EspFlashState,
    // TODO
    // error_message: Option<String>,
}

impl App {
    pub fn new(tx: Sender<Event>, rx: Receiver<Event>) -> Self {
        let exe_path = std::env::current_exe().unwrap();
        let config_path = exe_path.with_extension("toml");

        let settings = match Settings::load(&config_path, false) {
            Ok(settings) => settings,
            // Err(RedefaulterError::TomlDe(e)) => {
            //     error!("Settings load failed: {e}");
            //     // TODO move human_span formatting into thiserror fmt attr?
            //     let err_str = e.to_string();
            //     // Only grabbing the top line since it has the human-readable line and column information
            //     // (the error's span method is in *bytes*, not lines and columns)
            //     let human_span = err_str.lines().next().unwrap_or("").to_owned();
            //     let reason = e.message().to_owned();
            //     let new_err = RedefaulterError::SettingsLoad { human_span, reason };

            //     settings_load_failed_popup(new_err, lock_file);
            // }
            Err(e) => {
                error!("Settings load failed: {e}");
                panic!("Settings load failed: {e}");
            }
        };

        let mut user_input = UserInput::default();

        let saved_baud_rate = settings.last_port_settings.baud_rate;
        let selected_baud_index = COMMON_BAUD
            .iter()
            .position(|b| *b == saved_baud_rate)
            .unwrap_or_else(|| {
                user_input.input_box = Input::new(saved_baud_rate.to_string());
                COMMON_BAUD.last_index()
            });

        debug!("{settings:#?}");

        let (event_carousel, carousel_thread) = CarouselHandle::new();
        let (serial_handle, serial_thread) = SerialHandle::new(
            tx.clone(),
            settings.last_port_settings.clone(),
            settings.ignored.clone(),
        );

        let tick_tx = tx.clone();
        event_carousel.add_repeating(
            "PerSecond",
            Box::new(move || {
                tick_tx
                    .send(Tick::PerSecond.into())
                    .map_err(|e| e.to_string())
            }),
            Duration::from_secs(1),
        );

        // let serial_signal_tick_handle = serial_handle.clone();
        // let mut cache = arc_swap::Cache::new(std::sync::Arc::clone(
        //     &serial_signal_tick_handle.port_status,
        // ));
        // event_carousel.add_repeating(
        //     "SerialSignals",
        //     Box::new(move || {
        //         let port_status = cache.load();
        //         if port_status.state.is_healthy() {
        //             serial_signal_tick_handle
        //                 .read_signals()
        //                 .map_err(|e| e.to_string())
        //         } else {
        //             Ok(())
        //         }
        //     }),
        //     Duration::from_millis(100),
        // );

        let line_ending = settings.last_port_settings.rx_line_ending.as_bytes();
        let buffer = Buffer::new(
            line_ending,
            settings.rendering.clone(),
            #[cfg(feature = "logging")]
            settings.logging.clone(),
            tx.clone(),
        );
        // debug!("{buffer:#?}");
        Self {
            state: RunningState::Running,
            menu: Menu::PortSelection(PortSelectionElement::Ports),
            popup: None,
            popup_hint_scroll: -2,
            table_state: TableState::new().with_selected(Some(0)),
            baud_selection_state: SingleLineSelectorState::new().with_selected(selected_baud_index),
            popup_menu_item: 0,
            popup_table_state: TableState::new(),
            // popup_scrollbar_state: ScrollbarState::default(),
            // popup_single_line_state: SingleLineSelectorState::new(),
            ports: Vec::new(),

            carousel: event_carousel,
            carousel_thread: Takeable::new(carousel_thread),

            serial: serial_handle,
            serial_thread: Takeable::new(serial_thread),

            user_input,

            buffer,
            // buffer_scroll: 0,
            // buffer_scroll_state: ScrollbarState::default(),
            // buffer_stick_to_bottom: true,
            // // buffer_rendered_lines: 0,
            // buffer_wrapping: false,
            repeating_line_flip: false,
            failed_send_at: None,
            // failed_send_at: Instant::now(),
            #[cfg(feature = "macros")]
            macros: Macros::new(),
            action_queue: VecDeque::new(),
            scratch: settings.clone(),
            settings,
            keybinds: Keybinds::new(),
            notifs: Notifications::new(tx.clone()),
            tx,
            rx,

            #[cfg(feature = "espflash")]
            espflash: EspFlashState::new(),

            user_broke_connection: false,
        }
    }
    fn is_running(&self) -> bool {
        self.state == RunningState::Running
    }
    pub fn run(&mut self, mut terminal: Terminal<impl Backend>) -> Result<()> {
        // Get initial size of buffer.
        self.buffer.update_terminal_size(&mut terminal)?;
        let mut max_draw = Duration::default();
        let mut max_handle = Duration::default();
        while self.is_running() {
            let start = Instant::now();
            self.draw(&mut terminal)?;
            let end = Instant::now();
            let end1 = end.saturating_duration_since(start);
            max_draw = max_draw.max(end1);
            let msg = self.rx.recv()?;
            // debug!("{msg:?}");
            let start2 = Instant::now();
            self.handle_event(msg, &mut terminal)?;
            let end2 = start2.elapsed();
            max_handle = max_handle.max(end2);
            debug!(
                "Frame took {:?} to draw (max: {max_draw:?}), {:?} to handle (max: {max_handle:?}) ",
                end1, end2
            );
            // debug!("{msg:?}");

            // Don't wait for another loop iteration to start shutting down workers.
            // TODO convert into a loop {}, but see if clippy notices first (when i dont have 170+ warnings)
            if !self.is_running() {
                break;
            }
        }
        // Shutting down worker threads, with timeouts
        debug!("Shutting down Serial worker");
        if self.serial.shutdown().is_ok() {
            let serial_thread = self.serial_thread.take();
            if let Err(_) = serial_thread.join() {
                error!("Serial thread closed with an error!");
            }
        }
        debug!("Shutting down event carousel");
        if self.carousel.shutdown().is_ok() {
            let carousel = self.carousel_thread.take();
            if let Err(_) = carousel.join() {
                error!("Carousel thread closed with an error!");
            }
        }
        Ok(())
    }
    fn handle_event(&mut self, event: Event, terminal: &mut Terminal<impl Backend>) -> Result<()> {
        match event {
            Event::Quit => self.shutdown(),

            Event::Crossterm(CrosstermEvent::Resize) => {
                self.buffer.update_terminal_size(terminal)?;
            }
            Event::Crossterm(CrosstermEvent::KeyPress(key)) => self.handle_key_press(key),
            Event::Crossterm(CrosstermEvent::MouseScroll { up }) => {
                let amount = if up { 1 } else { -1 };
                self.buffer.scroll_by(amount);
            }

            Event::Crossterm(CrosstermEvent::RightClick) => {
                match self.user_input.clipboard.get_text() {
                    Ok(clipboard_text) => {
                        let mut previous_value = self.user_input.input_box.value().to_owned();
                        previous_value.push_str(&clipboard_text);
                        self.user_input.input_box = previous_value.into();
                    }
                    Err(e) => {
                        // error!("Failed to get clipboard text!");
                        error!("{e}");
                    }
                }
            }

            Event::Serial(SerialEvent::Connected(reconnect)) => {
                if let Some(reconnect_type) = &reconnect {
                    info!("Reconnected!");
                    let text = match reconnect_type {
                        ReconnectType::PerfectMatch => "Reconnected to same device!",
                        ReconnectType::UsbStrict => "Reconnected to same device?",
                        ReconnectType::UsbLoose => "Connected to similar USB device.",
                        ReconnectType::LastDitch => "Connected to COM port by name.",
                    };

                    self.notifs.notify_str(text, Color::Green);
                } else {
                    // If starting session with device.
                    info!("Connected!");

                    // self.notifs.notify_str("Connected to port!", Color::Green);
                }

                if let Some(current_port) = &self.serial.port_status.load().current_port {
                    #[cfg(feature = "logging")]
                    self.buffer
                        .log_handle
                        .log_port_connected(current_port.to_owned(), reconnect.clone())
                        .unwrap();
                } else {
                    error!("Was told about a port connection but no current port exists!");
                    panic!("Was told about a port connection but no current port exists!");
                }

                match &self.menu {
                    Menu::Terminal(TerminalPrompt::AttemptReconnectPrompt) => {
                        self.menu = Menu::Terminal(TerminalPrompt::None);
                    }
                    _ => (),
                }

                self.buffer.scroll_by(0);
                self.user_broke_connection = false;
            }
            Event::Serial(SerialEvent::Disconnected(reason)) => {
                #[cfg(feature = "espflash")]
                self.espflash.reset_popup();
                // self.menu = Menu::PortSelection;
                // if let Some(reason) = reason {
                //     self.notify(format!("Disconnected from port! {reason}"), Color::Red);
                // }
                #[cfg(feature = "logging")]
                self.buffer.log_handle.log_port_disconnected(false).unwrap();

                match reason {
                    SerialDisconnectReason::Intentional => (),
                    SerialDisconnectReason::UserBrokeConnection => {
                        self.user_broke_connection = true;
                        let mut text = String::from("Broke serial connection!");
                        if self.serial.port_settings.load().reconnections.allowed() {
                            text.push_str(" Reconnections paused!");
                        }
                        self.notifs.notify_str(text, Color::Red);
                    }
                    SerialDisconnectReason::Error(error) => {
                        let reconnect_text = match &self.settings.last_port_settings.reconnections {
                            Reconnections::Disabled => "Not attempting to reconnect.",
                            Reconnections::LooseChecks => "Attempting to reconnect (loose checks).",
                            Reconnections::StrictChecks => {
                                "Attempting to reconnect (strict checks)."
                            }
                        };
                        self.notifs.notify_str(
                            format!("Disconnected from port! {reconnect_text}"),
                            Color::Red,
                        );
                    }
                }
            }
            Event::Serial(SerialEvent::RxBuffer(mut data)) => {
                self.buffer.append_rx_bytes(&mut data);
                self.buffer.scroll_by(0);

                self.repeating_line_flip.flip();
            }
            Event::Serial(SerialEvent::Ports(ports)) => {
                self.ports = ports;
                if let Menu::PortSelection(PortSelectionElement::Ports) = &self.menu {
                    if self.table_state.selected().is_none() {
                        self.table_state.select(Some(0));
                    }
                }
            }
            #[cfg(feature = "espflash")]
            Event::Serial(SerialEvent::EspFlash(esp_event)) => match esp_event {
                EspEvent::BootloaderSuccess { chip } => self
                    .notifs
                    .notify_str(format!("{chip} rebooted into bootloader!"), Color::Green),
                EspEvent::EraseSuccess { chip } => self
                    .notifs
                    .notify_str(format!("{chip} flash erased!"), Color::Green),
                EspEvent::HardResetAttempt => self
                    .notifs
                    .notify_str(format!("Attempted ESP hard reset!"), Color::LightYellow),
                EspEvent::Error(e) => {
                    self.notifs.notify_str(&e, Color::Red);
                    self.action_queue.clear();
                }
                _ => self.espflash.consume_event(esp_event),
            },
            #[cfg(feature = "logging")]
            Event::Logging(LoggingEvent::Started) => self
                .notifs
                .notify_str("Starting logging incoming data to disk!", Color::Green),
            #[cfg(feature = "logging")]
            Event::Logging(LoggingEvent::Stopped { error: None }) => self
                .notifs
                .notify_str("Logging stopped, files closed!", Color::Yellow),
            #[cfg(feature = "logging")]
            Event::Logging(LoggingEvent::Stopped { error: Some(error) }) => self
                .notifs
                .notify_str("Logging stopped with error!", Color::Red),
            #[cfg(feature = "logging")]
            Event::Logging(LoggingEvent::Error(error)) => {
                self.notifs.notify_str("Logging error!", Color::Red)
            }
            Event::Tick(Tick::PerSecond) => match self.menu {
                Menu::Terminal(TerminalPrompt::None) => {
                    let port_status = &self.serial.port_status.load().inner;

                    let reconnections_allowed =
                        self.serial.port_settings.load().reconnections.allowed();
                    if !port_status.is_healthy()
                        && !port_status.is_lent_out()
                        && reconnections_allowed
                        && !self.user_broke_connection
                    {
                        self.repeating_line_flip.flip();
                        self.serial.request_reconnect(None).unwrap();
                    }
                }
                // If disconnect prompt is open, pause reacting to the ticks
                Menu::Terminal(TerminalPrompt::DisconnectPrompt)
                | Menu::Terminal(TerminalPrompt::AttemptReconnectPrompt) => (),
                Menu::PortSelection(_) => {
                    self.serial.request_port_scan().unwrap();
                }
            },
            Event::Tick(Tick::Scroll) => {
                self.popup_hint_scroll += 1;

                if self.popup.is_some() {
                    let tx = self.tx.clone();
                    self.carousel.add_oneshot(
                        "ScrollText",
                        Box::new(move || tx.send(Tick::Scroll.into()).map_err(|e| e.to_string())),
                        Duration::from_millis(400),
                    );
                }
            }
            Event::Tick(Tick::Action) => {
                self.consume_one_queued_action()?;
            }
            Event::Tick(Tick::Notification) => {
                // debug!("notif!");
                if let Some(notif) = &self.notifs.inner {
                    let tx = self.tx.clone();
                    let emerging = notif.shown_for() <= EMERGE_TIME;
                    let collapsing = notif.shown_for() >= PAUSE_TIME;
                    let sleep_time = if emerging || collapsing {
                        Duration::from_millis(50)
                    } else if notif.replaced && notif.shown_for() <= EXPAND_TIME {
                        EXPAND_TIME.saturating_sub(notif.shown_for())
                    } else {
                        PAUSE_TIME.saturating_sub(notif.shown_for())
                    };
                    self.carousel.add_oneshot(
                        "Notification",
                        Box::new(move || {
                            tx.send(Tick::Notification.into())
                                .map_err(|e| e.to_string())
                        }),
                        sleep_time,
                    );
                }
            }
            Event::Tick(Tick::Requested(origin)) => {
                debug!("Requested tick recieved from: {origin}");
                self.failed_send_at
                    .take_if(|i| i.elapsed() >= FAILED_SEND_VISUAL_TIME);
            }
            Event::Tick(Tick::Tx) => {
                self.repeating_line_flip.flip();
            }
        }
        Ok(())
    }
    fn shutdown(&mut self) {
        self.state = RunningState::Finished;
    }
    #[cfg(feature = "macros")]
    fn send_one_macro(
        &mut self,
        macro_ref: MacroNameTag,
        key_combo_opt: Option<KeyCombination>,
    ) -> Result<(), ()> {
        // let serial_healthy = self.serial.port_status.load().inner.is_healthy();

        // if !serial_healthy {
        //     //     self.macros_tx_queue.clear();
        //     return Ok(());
        // }
        // let Some((key_combo_opt, macro_ref)) = self.macros_tx_queue.pop_front() else {
        //     return;
        // };

        let (macro_tag, macro_content) = self
            .macros
            .all
            .iter()
            .find(|(tag, _string)| &&macro_ref == tag)
            .ok_or(())?;

        let italic = Style::new().italic();

        assert!(!macro_content.is_empty());

        let (notif_line, notif_color) = match (key_combo_opt, macro_content) {
            // (_, _) if macro_content.is_empty() => (
            //     line!["Macro \"", span!(italic; macro_tag), "\" is empty!"],
            //     Color::Yellow,
            // ),
            (Some(key_combo), _) => (
                line![span!(italic; macro_tag), span!(" [{key_combo}]")],
                Color::Green,
            ),

            (None, _) => (line![span!(italic; "{macro_tag}")], Color::Green),
        };

        let default_macro_line_ending =
            self.settings.last_port_settings.macro_line_ending.as_bytes(
                &self.settings.last_port_settings.rx_line_ending,
                &self.settings.last_port_settings.tx_line_ending,
            );

        let macro_line_ending = if let Some(line_ending) = &macro_content.escaped_line_ending {
            Cow::Owned(Vec::unescape_bytes(line_ending))
        } else {
            Cow::Borrowed(default_macro_line_ending)
        };

        // let macro_line_ending = macro_content
        //     .escaped_line_ending
        //     .as_ref()
        //     .map(CompactString::as_bytes)
        //     .unwrap_or(default_macro_line_ending);

        match macro_content {
            _ if macro_content.is_empty() => (),
            _ if macro_content.has_bytes => {
                let content = macro_content.unescape_bytes();
                self.serial
                    .send_bytes(content.clone(), Some(&macro_line_ending))
                    .unwrap();

                debug!("{}", format!("Sending Macro Bytes: {:02X?}", content));
                self.buffer.append_user_bytes(
                    &content,
                    &macro_line_ending,
                    true,
                    macro_content.sensitive,
                );
            }
            _ => {
                self.serial
                    .send_str(&macro_content.content, &macro_line_ending, true)
                    .unwrap();
                self.buffer.append_user_text(
                    &macro_content.content,
                    &macro_line_ending,
                    true,
                    macro_content.sensitive,
                );

                debug!(
                    "{}",
                    format!(
                        "Sending Macro Text: {}",
                        macro_content.as_str().escape_debug()
                    )
                );
            }
        };

        self.notifs.notify(notif_line, notif_color);

        Ok(())
    }
    // TODO fuzz this
    fn handle_key_press(&mut self, key: KeyEvent) {
        let key_combo = KeyCombination::from(key);
        // debug!("{key_combo}");

        // let at_port_selection = matches!(self.menu, Menu::PortSelection);
        // TODO soon, redo this variable's name + use
        let mut at_port_selection = false;
        let mut at_terminal = false;
        // Filter for when we decide to handle user *text input*.
        // TODO move these into per-menu funcs.
        match self.popup {
            #[cfg(feature = "macros")]
            Some(PopupMenu::Macros) => {
                match self
                    .macros
                    .search_input
                    .handle_event(&ratatui::crossterm::event::Event::Key(key))
                {
                    Some(StateChanged { value: true, .. }) => {}

                    Some(StateChanged { cursor: true, .. }) => {}
                    _ => (),
                }
            }
            _ => (),
        }
        match self.menu {
            Menu::Terminal(TerminalPrompt::None) if self.popup.is_none() => {
                at_terminal = true;
                match key_combo {
                    // Consuming Ctrl+A so input_box.handle_event doesn't move my cursor.
                    key!(ctrl - a) => (),
                    key!(del) | key!(backspace) if self.user_input.all_text_selected => (),

                    // TODO move into UserInput impl?
                    _ => match self
                        .user_input
                        .input_box
                        .handle_event(&ratatui::crossterm::event::Event::Key(key))
                    {
                        // If we changed something in the value when handling the key event,
                        // we should clear the user_history selection.
                        Some(StateChanged { value: true, .. }) => {
                            self.user_input.clear_history_selection();
                            self.user_input.search_result = None;
                            self.user_input.all_text_selected = false;
                        }

                        Some(StateChanged { cursor: true, .. }) => {
                            self.user_input.all_text_selected = false;
                        }
                        _ => (),
                    },
                    _ => (),
                }
            }
            Menu::Terminal(TerminalPrompt::None) => (),
            Menu::Terminal(TerminalPrompt::DisconnectPrompt) => (),
            Menu::Terminal(TerminalPrompt::AttemptReconnectPrompt) => (),
            Menu::PortSelection(PortSelectionElement::CustomBaud) => {
                // filtering out just letters from being put into the custom baud entry
                // extra checks will be needed at parse stage to ensure non-digit chars arent present
                if !matches!(key.code, KeyCode::Char(c) if c.is_alphabetic()) {
                    self.user_input
                        .input_box
                        .handle_event(&ratatui::crossterm::event::Event::Key(key));
                }
            }
            // might replace with PartialEq, Eq on Menu later, not sure
            Menu::PortSelection(_) => at_port_selection = true,
        }
        let vim_scrollable_menu: bool = match (self.menu, &self.popup) {
            // (_, Some(PopupMenu::Macros), MacrosPrompt::Keybind) => false,
            #[cfg(feature = "macros")]
            (_, Some(PopupMenu::Macros)) => false,
            (Menu::Terminal(TerminalPrompt::None), None) => false,
            _ => true,
        };
        // TODO split this up into more functions based on menu
        match key_combo {
            // Start of hardcoded keybinds.
            key!(q) if at_port_selection && self.popup.is_none() => self.shutdown(),
            key!(ctrl - shift - c) => self.shutdown(),
            // move into ctrl-c func?
            key!(ctrl - c) => match self.menu {
                Menu::Terminal(TerminalPrompt::DisconnectPrompt) => self.shutdown(),
                Menu::Terminal(TerminalPrompt::None) => {
                    self.dismiss_popup();
                    self.table_state.select(Some(0));
                    self.menu = TerminalPrompt::DisconnectPrompt.into();
                }
                _ => self.shutdown(),
            },
            key!(ctrl - a) if at_terminal && !self.user_input.input_box.value().is_empty() => {
                self.user_input.all_text_selected = true;
            }
            key!(home) if self.popup.is_some() => {
                self.popup_menu_item = 0;
            }
            // TODO ctrl+backspace remove a word
            key!(ctrl - pageup) | key!(shift - pageup) => self.buffer.scroll_by(i32::MAX),
            key!(ctrl - pagedown) | key!(shift - pagedown) => self.buffer.scroll_by(i32::MIN),
            key!(ctrl - shift - delete) | key!(ctrl - shift - backspace) => {
                self.user_input.clear();
            }
            key!(delete) | key!(backspace) if self.user_input.all_text_selected => {
                self.user_input.clear();
            }
            key!(pageup) => self.buffer.scroll_page_up(),
            key!(pagedown) => self.buffer.scroll_page_down(),
            // KeyCode::F(f_key) if ctrl_pressed && shift_pressed => {
            //     let meow = key!(ctrl - c);
            //     self.notify(format!("Pressed Ctrl+Shift+F{f_key}"), Color::Blue)
            // }
            // KeyCode::F(f_key) if ctrl_pressed => {
            //     self.notify(format!("Pressed Ctrl+F{f_key}"), Color::Blue)
            // }
            // KeyCode::F(f_key) if shift_pressed => {
            //     self.notify(format!("Pressed Shift+F{f_key}"), Color::Blue)
            // }
            // KeyCode::F(f_key) => self.notify(format!("Pressed F{f_key}"), Color::Blue),
            // KeyCode::End => self
            //     .event_carousel
            //     .add_event(Event::TickSecond, Duration::from_secs(3)),
            key!(h) if vim_scrollable_menu => self.left_pressed(),
            key!(j) if vim_scrollable_menu => self.down_pressed(),
            key!(k) if vim_scrollable_menu => self.up_pressed(),
            key!(l) if vim_scrollable_menu => self.right_pressed(),
            key!(up) => self.up_pressed(),
            key!(down) => self.down_pressed(),
            key!(left) => self.left_pressed(),
            key!(right) => self.right_pressed(),
            key!(enter) => self.enter_pressed(false, false),
            key!(ctrl - enter) => self.enter_pressed(true, false),
            key!(shift - enter) => self.enter_pressed(false, true),
            key!(ctrl - shift - enter) => self.enter_pressed(true, true),
            key!(tab) if at_terminal && self.popup.is_none() => {
                self.user_input.find_input_in_history();
            }
            // KeyCode::Tab => self.tab_pressed(),
            #[cfg(feature = "macros")]
            key!(ctrl - r) if self.popup == Some(PopupMenu::Macros) => {
                self.run_method_from_action(AppAction::Macros(MacroAction::ReloadMacros))
                    .unwrap();
            }
            #[cfg(feature = "espflash")]
            key!(ctrl - r) if self.popup == Some(PopupMenu::EspFlash) => {
                self.run_method_from_action(AppAction::Esp(EspAction::ReloadProfiles))
                    .unwrap();
            }
            key!(esc) => self.esc_pressed(),
            key_combo => {
                let Some(actions_str) = self.keybinds.action_set_from_key_combo(key_combo)
                // .map(ToOwned::to_owned)
                else {
                    return;
                };

                let mut actions = Vec::new();

                for action in actions_str {
                    if let Some(action) = self.get_action_from_string(action) {
                        actions.push(action);
                    } else {
                        self.notifs.notify_str(
                            format!("Unrecognized keybind action: \"{action}\""),
                            Color::Yellow,
                        );
                        return;
                    }
                }

                debug!("{actions:?}");

                self.queue_action_set(actions, Some(key_combo)).unwrap();
            } //     #[cfg(feature = "espflash")]
              //     if let Some(profile_name) = self
              //         .keybinds
              //         .espflash_profile_from_key_combo(key_combo)
              //         .map(ToOwned::to_owned)
              //     {
              //         if let Some(profile) = self
              //             .espflash
              //             .profiles()
              //             .find(|(name, _, _)| *name == profile_name)
              //         {
              //             let italic = Style::new().italic();
              //             if serial_healthy {
              //                 self.notifs.notify(
              //                     line![
              //                         "Flashing with \"",
              //                         span!(italic;profile_name),
              //                         "\" [",
              //                         key_combo.to_string(),
              //                         "]"
              //                     ],
              //                     Color::LightGreen,
              //                 );
              //                 self.serial
              //                     .esp_flash_profile(
              //                         self.espflash.profile_from_name(&profile_name).unwrap(),
              //                     )
              //                     .unwrap();
              //             } else {
              //                 self.notifs.notify(
              //                     line![
              //                         "Not flashing with \"",
              //                         span!(italic;profile_name),
              //                         "\" [",
              //                         key_combo.to_string(),
              //                         "]"
              //                     ],
              //                     Color::Yellow,
              //                 );
              //             }
              //         } else {
              //             error!("No such espflash profile: \"{profile_name}\"");
              //             self.notifs.notify_str(
              //                 format!("No such espflash profile: \"{profile_name}\""),
              //                 Color::Yellow,
              //             );
              //             // let Some(profile) =
              //             //     self.espflash.elfs.iter().find(|p| p.name == profile_name)
              //             // else {};
              //         }

              //         return;
              //     }
        }
    }
    fn queue_action_set(
        &mut self,
        mut actions: Vec<Action>,
        key_combo_opt: Option<KeyCombination>,
    ) -> Result<()> {
        assert!(
            !actions.is_empty(),
            "should never be asked to queue no actions"
        );

        // Don't bother queuing if it's just one action, send it.
        if actions.len() == 1 {
            self.action_dispatch(actions.pop().unwrap(), key_combo_opt)?;
            return Ok(());
        }

        self.action_queue
            .extend(actions.into_iter().map(|a| (key_combo_opt, a)));

        self.tx.send(Tick::Action.into())?;

        Ok(())
    }
    fn get_action_from_string(&self, action: &str) -> Option<Action> {
        if action.trim().is_empty() {
            return None;
        }

        let action = action.trim();

        // Check for matching app method
        if let Ok(app_action) = action.parse::<AppAction>() {
            return Some(Action::AppAction(app_action));
        }

        #[cfg(feature = "espflash")]
        // Try to find esp profile by exact name match.
        if let Some(_) = self.espflash.profile_from_name(action) {
            return Some(Action::EspFlashProfile(action.to_owned()));
        }

        #[cfg(feature = "macros")]
        // Get Macro by name and category, optionally categorically fuzzy
        if let Some(nametag) = self
            .macros
            .get_by_string(action, self.settings.behavior.fuzzy_macro_match)
        {
            return Some(Action::MacroInvocation(nametag));
        }

        let parse_duration = |s: &str| -> Option<Duration> {
            let pause_prefix = "pause_ms:";
            let s_start = s.first_chars(pause_prefix.len())?;
            if !s_start.eq_ignore_ascii_case(pause_prefix) {
                return None;
            }
            let delay_ms = s[pause_prefix.len()..].parse().ok()?;
            Some(Duration::from_millis(delay_ms))
        };

        // Check if it's a pause request
        if let Some(duration) = parse_duration(action) {
            return Some(Action::Pause(duration));
        }

        // Otherwise, it's nothing we recognize.
        None
    }
    fn consume_one_queued_action(&mut self) -> Result<()> {
        let Some((key_combo_opt, action)) = self.action_queue.front() else {
            return Ok(());
        };

        // if action.requires_port_connection() {
        let port_status_guard = self.serial.port_status.load().inner;
        match port_status_guard {
            InnerPortStatus::Connected => (),
            InnerPortStatus::LentOut => {
                let tx = self.tx.clone();
                self.carousel.add_oneshot(
                    "ActionQueue",
                    Box::new(move || tx.send(Tick::Action.into()).map_err(|e| e.to_string())),
                    Duration::from_millis(500),
                );
                return Ok(());
            }
            InnerPortStatus::Idle | InnerPortStatus::PrematureDisconnect => {
                let text = format!(
                    "Port isn't ready, clearing {} queued actions.",
                    self.action_queue.len()
                );
                self.notifs.notify_str(text, Color::Red);
                self.action_queue.clear();
                return Ok(());
            }
        }
        // }

        let Some((key_combo_opt, action)) = self.action_queue.pop_front() else {
            unreachable!()
        };

        let pause_duration_opt = self.action_dispatch(action, key_combo_opt)?;

        let next_action_delay =
            pause_duration_opt.unwrap_or(self.settings.behavior.action_chain_delay);
        let tx = self.tx.clone();
        self.carousel.add_oneshot(
            "ActionQueue",
            Box::new(move || tx.send(Tick::Action.into()).map_err(|e| e.to_string())),
            next_action_delay,
        );

        Ok(())
    }
    // TODO figure out more error handling
    // TODO and figure out logic with device connection halting certain actions
    fn action_dispatch(
        &mut self,
        action: Action,
        key_combo_opt: Option<KeyCombination>,
    ) -> Result<Option<Duration>> {
        debug!("Consuming action: {action:?}");
        match action {
            Action::AppAction(method) => self.run_method_from_action(method)?,
            Action::Pause(duration) => return Ok(Some(duration)),
            #[cfg(feature = "macros")]
            Action::MacroInvocation(name_tag) => {
                self.send_one_macro(name_tag, key_combo_opt).unwrap()
            }
            #[cfg(feature = "espflash")]
            // TODO show name of flashing profile
            Action::EspFlashProfile(profile) => self
                .serial
                .esp_flash_profile(self.espflash.profile_from_name(&profile).unwrap())?,
        }
        Ok(None)
    }
    fn run_method_from_action(&mut self, action: AppAction) -> Result<()> {
        let pretty_bool = |b: bool| {
            if b { "On" } else { "Off" }
        };
        use AppAction as A;
        match action {
            A::Popup(popup) => self.show_popup(popup),

            A::Port(PortAction::ToggleDtr) => {
                self.serial.toggle_signals(true, false).unwrap();
            }
            A::Port(PortAction::ToggleRts) => {
                self.serial.toggle_signals(false, true).unwrap();
            }
            A::Port(PortAction::AssertDtr) => {
                self.serial.write_signals(Some(true), None).unwrap();
            }
            A::Port(PortAction::DeassertDtr) => {
                self.serial.write_signals(Some(false), None).unwrap();
            }
            A::Port(PortAction::AssertRts) => {
                self.serial.write_signals(None, Some(true)).unwrap();
            }
            A::Port(PortAction::DeassertRts) => {
                self.serial.write_signals(None, Some(false)).unwrap();
            }
            A::Port(PortAction::AttemptReconnectStrict) => {
                self.serial
                    .request_reconnect(Some(Reconnections::StrictChecks))
                    .unwrap();
            }
            A::Port(PortAction::AttemptReconnectLoose) => {
                self.serial
                    .request_reconnect(Some(Reconnections::LooseChecks))
                    .unwrap();
            }
            A::Base(BaseAction::ToggleTextwrap) => {
                let state = pretty_bool(self.settings.rendering.wrap_text.flip());
                self.buffer
                    .update_render_settings(self.settings.rendering.clone());
                self.settings.save().unwrap();
                self.notifs
                    .notify_str(format!("Toggled Text Wrapping {state}"), Color::Gray);
            }
            A::Base(BaseAction::ToggleTimestamps) => {
                let state = pretty_bool(self.settings.rendering.timestamps.flip());
                self.buffer
                    .update_render_settings(self.settings.rendering.clone());
                self.settings.save().unwrap();
                self.notifs
                    .notify_str(format!("Toggled Timestamps {state}"), Color::Gray);
            }

            A::Base(BaseAction::ToggleIndices) => {
                let state = pretty_bool(self.settings.rendering.show_indices.flip());
                self.buffer
                    .update_render_settings(self.settings.rendering.clone());
                self.notifs.notify_str(
                    format!("Toggled Line Indices + Length {state}"),
                    Color::Gray,
                );
            }

            A::Base(BaseAction::ToggleHex) => {
                let state = pretty_bool(self.settings.rendering.hex_view.flip());
                self.buffer
                    .update_render_settings(self.settings.rendering.clone());
                self.buffer.scroll_by(0);
                self.notifs
                    .notify_str(format!("Toggled Hex View {state}"), Color::Gray);
            }

            A::Base(BaseAction::ToggleHexHeader) => {
                let state = pretty_bool(self.settings.rendering.hex_view_header.flip());
                self.buffer
                    .update_render_settings(self.settings.rendering.clone());
                self.notifs
                    .notify_str(format!("Toggled Hex View Header {state}"), Color::Gray);
            }

            #[cfg(feature = "macros")]
            A::Macros(MacroAction::ReloadMacros) => {
                self.macros
                    .load_from_folder("../../example_macros")
                    .unwrap();
                self.notifs
                    .notify_str(format!("Reloaded Macros!"), Color::Green);
            }

            A::Base(BaseAction::ReloadColors) => {
                self.buffer.reload_color_rules().unwrap();
                self.notifs
                    .notify_str(format!("Reloaded Color Rules!"), Color::Green);
            }

            #[cfg(feature = "logging")]
            A::Logging(LoggingAction::Start) => {
                let port_status_guard = self.serial.port_status.load();
                let Some(port_info) = &port_status_guard.current_port else {
                    self.notifs
                        .notify_str("Not connected to port, not starting log.", Color::Red);
                    return Ok(());
                };
                self.buffer
                    .log_handle
                    .request_log_start(port_info.clone())
                    .unwrap();
                self.notifs
                    .notify_str("Requested logging start!", Color::Green);
            }

            #[cfg(feature = "logging")]
            A::Logging(LoggingAction::Stop) => {
                if self.buffer.log_handle.logging_active() {
                    self.buffer.log_handle.request_log_stop().unwrap();
                } else {
                    self.notifs
                        .notify_str("No logging session active to stop!", Color::Yellow);
                }
            }

            #[cfg(feature = "logging")]
            A::Logging(LoggingAction::Toggle) => {
                if self.buffer.log_handle.logging_active() {
                    self.run_method_from_action(LoggingAction::Stop.into())?;
                } else {
                    self.run_method_from_action(LoggingAction::Start.into())?;
                }
            }

            #[cfg(feature = "espflash")]
            A::Esp(EspAction::EspHardReset) => {
                self.serial.esp_restart(EspRestartType::UserCode).unwrap();
            }

            #[cfg(feature = "espflash")]
            A::Esp(EspAction::EspBootloader) => {
                self.serial
                    .esp_restart(EspRestartType::Bootloader { active: true })
                    .unwrap();
            }

            #[cfg(feature = "espflash")]
            A::Esp(EspAction::EspBootloaderUnchecked) => {
                self.serial
                    .esp_restart(EspRestartType::Bootloader { active: false })
                    .unwrap();
            }

            #[cfg(feature = "espflash")]
            A::Esp(EspAction::EspDeviceInfo) => {
                self.serial.esp_device_info().unwrap();
            }

            #[cfg(feature = "espflash")]
            A::Esp(EspAction::EspEraseFlash) => {
                self.serial.esp_erase_flash().unwrap();
            }
            #[cfg(feature = "espflash")]
            A::Esp(EspAction::ReloadProfiles) => {
                self.notifs
                    .notify_str("Reloaded espflash profiles!", Color::Green);
                self.espflash.reload().unwrap();
            } // unknown => {
              //     warn!("Unknown keybind action: {unknown}");
              //     self.notifs.notify_str(
              //         format!("Unknown keybind action: \"{unknown}\""),
              //         Color::Yellow,
              //     );
              // }
        };
        Ok(())
    }
    // fn tab_pressed(&mut self) {}
    fn esc_pressed(&mut self) {
        match self.popup {
            None => (),
            Some(_) => {
                self.dismiss_popup();
                return;
            }
        }
        if self.popup.is_some() {
            return;
        }

        match self.menu {
            Menu::Terminal(TerminalPrompt::None) => {
                self.table_state.select(Some(0));
                let port_status_guard = self.serial.port_status.load();
                let port_settings_guard = self.serial.port_settings.load();

                if self.user_broke_connection
                    || (!port_status_guard.inner.is_healthy()
                        && !port_status_guard.inner.is_lent_out()
                        && !port_settings_guard.reconnections.allowed())
                {
                    self.menu = TerminalPrompt::AttemptReconnectPrompt.into();
                } else {
                    self.menu = TerminalPrompt::DisconnectPrompt.into();
                }
            }
            Menu::Terminal(TerminalPrompt::DisconnectPrompt)
            | Menu::Terminal(TerminalPrompt::AttemptReconnectPrompt) => {
                self.menu = TerminalPrompt::None.into();
            }
            Menu::PortSelection(_) => self.shutdown(),
        }
    }
    fn up_pressed(&mut self) {
        self.user_input.all_text_selected = false;
        if self.popup.is_some() {
            self.popup_hint_scroll = -2;
            match self.popup_menu_item {
                0 => self.popup_menu_item = self.current_popup_item_count(),
                _ => self.popup_menu_item = self.popup_menu_item - 1,
            }

            return;
        }

        use PortSelectionElement as Pse;
        match self.menu {
            Menu::PortSelection(e @ Pse::Ports) => match self.table_state.selected() {
                Some(0) => self.menu = e.prev().into(),
                Some(_) => self.scroll_menu_up(),
                None => (),
            },
            Menu::PortSelection(e) => self.menu = e.prev().into(),
            Menu::Terminal(TerminalPrompt::None) => self.user_input.scroll_history(true),
            Menu::Terminal(TerminalPrompt::DisconnectPrompt) => wrapping_prompt_scroll(
                <DisconnectPrompt as VariantArray>::VARIANTS.len(),
                &mut self.table_state,
                true,
            ),
            Menu::Terminal(TerminalPrompt::AttemptReconnectPrompt) => wrapping_prompt_scroll(
                <AttemptReconnectPrompt as VariantArray>::VARIANTS.len(),
                &mut self.table_state,
                true,
            ),
        }
        self.post_menu_vert_scroll(true);
    }
    fn down_pressed(&mut self) {
        self.user_input.all_text_selected = false;
        if self.popup.is_some() {
            self.popup_hint_scroll = -2;
            match self.popup_menu_item {
                _ if self.popup_menu_item == self.current_popup_item_count() => {
                    self.popup_menu_item = 0
                }
                _ => self.popup_menu_item = self.popup_menu_item + 1,
            }

            return;
        }

        use PortSelectionElement as Pse;
        match self.menu {
            Menu::PortSelection(e @ Pse::Ports) if self.table_state.on_last(&self.ports) => {
                // self.table_state.select(None);
                self.menu = e.next().into();
            }
            Menu::PortSelection(Pse::Ports) => {
                self.scroll_menu_down();
            }
            Menu::PortSelection(e) => {
                self.menu = e.next().into();
            }
            // Menu::PortSelection(_) => {
            //     if self.single_line_state.active {
            //         ()
            //         // move down to More Options/Custom baud entry
            //     } else {
            //         match self.table_state.selected() {
            //             None => (),
            //             Some(index) if self.table_state.on_last(&self.ports) => {
            //                 self.table_state.select(None);
            //                 self.single_line_state.active = true;
            //             }
            //             Some(_) => self.scroll_menu_down(),
            //         }
            //     }
            // },
            Menu::Terminal(TerminalPrompt::None) => self.user_input.scroll_history(false),
            Menu::Terminal(TerminalPrompt::DisconnectPrompt) => wrapping_prompt_scroll(
                <DisconnectPrompt as VariantArray>::VARIANTS.len(),
                &mut self.table_state,
                false,
            ),
            Menu::Terminal(TerminalPrompt::AttemptReconnectPrompt) => wrapping_prompt_scroll(
                <AttemptReconnectPrompt as VariantArray>::VARIANTS.len(),
                &mut self.table_state,
                false,
            ),
        }
        self.post_menu_vert_scroll(false);
    }
    fn left_pressed(&mut self) {
        match &mut self.popup {
            None => (),
            Some(_popup) if self.popup_menu_item == 0 => {
                self.scroll_popup(false);
            }
            Some(PopupMenu::PortSettings) => {
                self.scratch
                    .last_port_settings
                    .handle_input(ArrowKey::Left, self.get_corrected_popup_item().unwrap())
                    .unwrap();
            }
            Some(PopupMenu::BehaviorSettings) => {
                self.scratch
                    .behavior
                    .handle_input(ArrowKey::Left, self.get_corrected_popup_item().unwrap())
                    .unwrap();
            }
            Some(PopupMenu::RenderingSettings) => {
                self.scratch
                    .rendering
                    .handle_input(ArrowKey::Left, self.get_corrected_popup_item().unwrap())
                    .unwrap();
            }
            #[cfg(feature = "macros")]
            Some(PopupMenu::Macros) => {
                if !self.macros.search_input.value().is_empty() {
                    return;
                }
                self.macros.categories_selector.prev();
                if self.popup_menu_item >= 2 {
                    if self.macros.none_visible() {
                        self.popup_menu_item = 1;
                    } else {
                        self.popup_menu_item = 2;
                    }
                }
            }
            #[cfg(feature = "espflash")]
            Some(PopupMenu::EspFlash) => (),
            #[cfg(feature = "logging")]
            Some(PopupMenu::Logging) => {
                if self.popup_menu_item == 1 {
                    return;
                }

                let result = self
                    .scratch
                    .logging
                    .handle_input(ArrowKey::Left, self.get_corrected_popup_item().unwrap())
                    .unwrap();
            }
        }
        if self.popup.is_some() {
            return;
        }
        if matches!(self.menu, Menu::PortSelection(_)) && self.baud_selection_state.active {
            if self.baud_selection_state.current_index == 0 {
                self.baud_selection_state.select(COMMON_BAUD.last_index());
            } else {
                self.baud_selection_state.prev();
            }
        }
    }
    fn right_pressed(&mut self) {
        // KeyCode::Left if at_port_selection && self.single_line_state.active => {
        //     if self.single_line_state.current_index == 0 {
        //         self.single_line_state.select(COMMON_BAUD.last_index());
        //     } else {
        //         self.single_line_state.prev();
        //     }
        // }
        match &mut self.popup {
            None => (),
            Some(popup) if self.popup_menu_item == 0 => {
                self.scroll_popup(true);
            }
            Some(PopupMenu::PortSettings) => {
                self.scratch
                    .last_port_settings
                    .handle_input(ArrowKey::Right, self.get_corrected_popup_item().unwrap())
                    .unwrap();
            }
            Some(PopupMenu::BehaviorSettings) => {
                self.scratch
                    .behavior
                    .handle_input(ArrowKey::Right, self.get_corrected_popup_item().unwrap())
                    .unwrap();
            }
            Some(PopupMenu::RenderingSettings) => {
                self.scratch
                    .rendering
                    .handle_input(ArrowKey::Right, self.get_corrected_popup_item().unwrap())
                    .unwrap();
            }
            #[cfg(feature = "macros")]
            Some(PopupMenu::Macros) => {
                if !self.macros.search_input.value().is_empty() {
                    return;
                }
                self.macros.categories_selector.next();
                if self.popup_menu_item >= 2 {
                    if self.macros.none_visible() {
                        self.popup_menu_item = 1;
                    } else {
                        self.popup_menu_item = 2;
                    }
                }
            }
            #[cfg(feature = "espflash")]
            Some(PopupMenu::EspFlash) => (),
            #[cfg(feature = "logging")]
            Some(PopupMenu::Logging) => {
                if self.popup_menu_item == 1 {
                    return;
                }

                let result = self
                    .scratch
                    .logging
                    .handle_input(ArrowKey::Right, self.get_corrected_popup_item().unwrap())
                    .unwrap();
            }
        }
        if self.popup.is_some() {
            return;
        }
        if matches!(self.menu, Menu::PortSelection(_)) && self.baud_selection_state.active {
            if self.baud_selection_state.next() >= COMMON_BAUD.len() {
                self.baud_selection_state.select(0);
            }
        }
    }
    fn enter_pressed(&mut self, ctrl_pressed: bool, shift_pressed: bool) {
        let serial_healthy = self.serial.port_status.load().inner.is_healthy();
        let popup_was_some = self.popup.is_some();
        // debug!("{:?}", self.menu);
        use PortSelectionElement as Pse;
        match self.popup {
            None => (),
            Some(PopupMenu::PortSettings) => {
                self.settings.last_port_settings = self.scratch.last_port_settings.clone();
                self.buffer
                    .update_line_ending(self.scratch.last_port_settings.rx_line_ending.as_bytes());

                self.serial
                    .update_settings(self.scratch.last_port_settings.clone())
                    .unwrap();

                if matches!(self.menu, Menu::Terminal(_)) {
                    if self.settings.behavior.retain_port_setting_changes {
                        self.settings.save().unwrap();
                        self.notifs.notify_str("Port settings saved!", Color::Green);
                    } else {
                        self.notifs
                            .notify_str("Port settings applied!", Color::Gray);
                    }
                } else {
                    self.settings.save().unwrap();
                    self.notifs.notify_str("Port settings saved!", Color::Green);
                }
                self.dismiss_popup();
            }
            Some(PopupMenu::BehaviorSettings) => {
                self.settings.behavior = self.scratch.behavior.clone();

                self.settings.save().unwrap();
                self.dismiss_popup();
                self.notifs
                    .notify_str("Behavior settings saved!", Color::Green);
            }
            Some(PopupMenu::RenderingSettings) => {
                self.settings.rendering = self.scratch.rendering.clone();
                self.buffer
                    .update_render_settings(self.settings.rendering.clone());

                self.settings.save().unwrap();
                self.dismiss_popup();
                self.notifs
                    .notify_str("Rendering settings saved!", Color::Green);
            }
            #[cfg(feature = "macros")]
            Some(PopupMenu::Macros) => {
                if self.popup_menu_item < 2 {
                    return;
                }
                let index = self.get_corrected_popup_item().unwrap();
                let (tag, string) = self.macros.filtered_macro_iter().nth(index).unwrap();
                let tag = tag.to_owned();
                // let macro_ref: MacroNameTag = macro_binding.into();

                if ctrl_pressed || shift_pressed {
                    if !serial_healthy {
                        self.notifs.notify_str("Port isn't ready!", Color::Red);
                        return;
                    }
                    // Putting macro content into buffer.
                    match string {
                        _ if string.is_empty() => (),
                        _ if string.has_bytes => {
                            todo!()
                        }
                        text => {
                            self.user_input.clear();
                            self.user_input.input_box = text.as_str().into();
                            self.dismiss_popup();
                        }
                    }
                } else {
                    if !serial_healthy {
                        self.notifs.notify_str("Port isn't ready!", Color::Red);
                        return;
                    }
                    match string {
                        _ if string.is_empty() => {
                            self.notifs.notify_str("Macro is empty!", Color::Yellow)
                        }
                        _ => {
                            self.send_one_macro(tag, None).unwrap();
                            // self.action_queue.push_back((None, tag));
                            // self.tx.send(Tick::Action.into()).unwrap();
                        }
                    };
                }
            }
            #[cfg(feature = "espflash")]
            Some(PopupMenu::EspFlash) => {
                if self.popup_menu_item == 0 {
                    return;
                }
                if !serial_healthy {
                    self.notifs.notify_str("Port isn't ready!", Color::Red);
                    return;
                }
                let selected = self.get_corrected_popup_item().unwrap();
                // If a profile is selected
                if self.popup_menu_item > esp::ESPFLASH_BUTTON_COUNT {
                    assert!(
                        !self.espflash.is_empty(),
                        "shouldn't have selected a non-existant flash profile"
                    );

                    self.serial
                        .esp_flash_profile(self.espflash.profile_from_index(selected).unwrap())
                        .unwrap();
                } else {
                    match selected {
                        0 => self
                            .run_method_from_action(EspAction::EspHardReset.into())
                            .unwrap(),
                        1 if ctrl_pressed || shift_pressed => self
                            .run_method_from_action(EspAction::EspBootloaderUnchecked.into())
                            .unwrap(),
                        1 => self
                            .run_method_from_action(EspAction::EspBootloader.into())
                            .unwrap(),
                        2 => self
                            .run_method_from_action(EspAction::EspDeviceInfo.into())
                            .unwrap(),
                        3 => {
                            // TODO config option to allow skipping this?
                            if !shift_pressed && !ctrl_pressed {
                                self.notifs.notify_str(
                                    "Press Shift/Ctrl+Enter to erase flash!",
                                    Color::Yellow,
                                );
                            } else {
                                self.run_method_from_action(EspAction::EspEraseFlash.into())
                                    .unwrap();
                            }
                        }
                        unknown => unreachable!("unknown espflash command index {unknown}"),
                    }
                }
            }
            #[cfg(feature = "logging")]
            Some(PopupMenu::Logging) => {
                if self.popup_menu_item == 0 {
                    return;
                } else if self.popup_menu_item == 1 {
                    let logging_active = self.buffer.log_handle.logging_active();
                    if logging_active {
                        self.buffer.log_handle.request_log_stop().unwrap();
                    } else {
                        let port_status_guard = self.serial.port_status.load();
                        if let Some(port_info) = &port_status_guard.current_port {
                            self.buffer
                                .log_handle
                                .request_log_start(port_info.clone())
                                .unwrap();
                        } else {
                            self.notifs
                                .notify_str("No port active, not starting logging.", Color::Red);
                        }
                    }
                    return;
                }
                // Otherwise, save settings.

                self.settings.logging = self.scratch.logging.clone();
                let current_port = {
                    let port_status_guard = self.serial.port_status.load();
                    port_status_guard.current_port.clone()
                };
                self.buffer
                    .update_logging_settings(self.settings.logging.clone(), current_port);

                self.settings.save().unwrap();
                self.dismiss_popup();
                self.notifs
                    .notify_str("Logging settings saved!", Color::Green);
            }
        }
        if self.popup.is_some() || popup_was_some {
            return;
        }
        match self.menu {
            Menu::PortSelection(Pse::Ports) => {
                let selected = self.ports.get(self.table_state.selected().unwrap());
                if let Some(info) = selected {
                    info!("Port {}", info.port_name);

                    let baud_rate =
                        if COMMON_BAUD.last_index_eq(self.baud_selection_state.current_index) {
                            // TODO This should return an actual user-visible error
                            self.user_input.input_box.value().parse().unwrap()
                        } else {
                            COMMON_BAUD[self.baud_selection_state.current_index]
                        };

                    self.scratch.last_port_settings.baud_rate = baud_rate;

                    self.settings.last_port_settings = self.scratch.last_port_settings.clone();
                    self.settings.save().unwrap();

                    self.serial
                        .connect(&info, self.scratch.last_port_settings.clone())
                        .unwrap();

                    self.menu = Menu::Terminal(TerminalPrompt::None);
                }
            }
            Menu::PortSelection(Pse::MoreOptions) => {
                self.show_popup(ShowPopupAction::ShowPortSettings)
            }
            Menu::PortSelection(_) => (),
            Menu::Terminal(TerminalPrompt::None) => {
                if serial_healthy {
                    let user_input = self.user_input.input_box.value();

                    if self.settings.behavior.fake_shell {
                        let user_le = &self.settings.last_port_settings.tx_line_ending;
                        let user_le_bytes =
                            user_le.as_bytes(&self.settings.last_port_settings.rx_line_ending);
                        self.serial
                            .send_str(
                                user_input,
                                user_le_bytes,
                                self.settings.behavior.fake_shell_unescape,
                            )
                            .unwrap();
                        self.buffer
                            .append_user_text(user_input, user_le_bytes, false, false);
                        self.user_input.history.push(user_input);

                        self.user_input.clear();
                    } else {
                        // self.serial.send_str(user_input, "").unwrap();
                        todo!("not ready yet");
                        // self.buffer.append_user_text(user_input);
                    }

                    self.repeating_line_flip.flip();
                    // Scroll all the way down
                    // TODO: Make this behavior a toggle
                    self.buffer.scroll_by(i32::MIN);
                } else {
                    self.failed_send_at = Some(Instant::now());
                    // Temporarily show text on red background when trying to send while unhealthy
                    let tx = self.tx.clone();
                    self.carousel.add_oneshot(
                        "UnhealthyTxUi",
                        Box::new(move || {
                            tx.send(Tick::Requested("Unhealthy TX Background Removal").into())
                                .map_err(|e| e.to_string())
                        }),
                        FAILED_SEND_VISUAL_TIME,
                    );
                }
            }
            Menu::Terminal(TerminalPrompt::DisconnectPrompt) => {
                if self.table_state.selected().is_none() {
                    return;
                }
                let index = self.table_state.selected().unwrap() as u8;
                match DisconnectPrompt::try_from(index).unwrap() {
                    DisconnectPrompt::ExitApp => self.shutdown(),
                    DisconnectPrompt::Cancel => self.menu = Menu::Terminal(TerminalPrompt::None),
                    DisconnectPrompt::OpenPortSettings => {
                        self.show_popup(ShowPopupAction::ShowPortSettings)
                    }
                    DisconnectPrompt::Disconnect if shift_pressed || ctrl_pressed => {
                        // This is intentionally being set true unconditionally here, and also when the event pops.
                        // This is so that I or a user can forcibly trigger the pausing of reconnections/the appearance of the
                        // manual reconnection Esc popup.
                        self.user_broke_connection = true;
                        self.menu = Menu::Terminal(TerminalPrompt::AttemptReconnectPrompt);
                        self.serial.break_connection().unwrap();

                        // let port_status_guard = self.serial.port_status.load();
                        // if port_status_guard.inner.is_healthy() {
                        // } else {
                        // self.notifs
                        // .notify_str("Can't break connection!", Color::Red);
                        // }
                    }
                    DisconnectPrompt::Disconnect => {
                        self.serial.disconnect().unwrap();
                        // Refresh port listings
                        self.ports.clear();
                        self.serial.request_port_scan().unwrap();

                        self.buffer.intentional_disconnect_clear();
                        // Clear the input box, but keep the user history!
                        self.user_input.clear();

                        self.menu = Menu::PortSelection(Pse::Ports);
                    }
                }
            }
            Menu::Terminal(TerminalPrompt::AttemptReconnectPrompt) => {
                if self.table_state.selected().is_none() {
                    return;
                }
                let index = self.table_state.selected().unwrap() as u8;
                match AttemptReconnectPrompt::try_from(index).unwrap() {
                    AttemptReconnectPrompt::ExitApp => self.shutdown(),
                    AttemptReconnectPrompt::AttemptReconnect if shift_pressed && ctrl_pressed => {
                        self.user_broke_connection = false;
                        self.menu = Menu::Terminal(TerminalPrompt::None);
                        self.notifs
                            .notify_str("Unpausing reconnections!", Color::LightGreen);
                    }
                    AttemptReconnectPrompt::AttemptReconnect if shift_pressed || ctrl_pressed => {
                        self.repeating_line_flip.flip();
                        self.notifs
                            .notify_str("Attempting to reconnect! (Loose Checks)", Color::Yellow);
                        self.serial
                            .request_reconnect(Some(Reconnections::LooseChecks))
                            .unwrap();
                    }
                    AttemptReconnectPrompt::AttemptReconnect => {
                        self.repeating_line_flip.flip();
                        self.notifs
                            .notify_str("Attempting to reconnect! (Strict Checks)", Color::Yellow);
                        self.serial
                            .request_reconnect(Some(Reconnections::StrictChecks))
                            .unwrap();
                    }
                    AttemptReconnectPrompt::Cancel => {
                        self.menu = Menu::Terminal(TerminalPrompt::None)
                    }
                    AttemptReconnectPrompt::OpenPortSettings => {
                        self.show_popup(ShowPopupAction::ShowPortSettings)
                    }

                    AttemptReconnectPrompt::BackToPortSelection => {
                        self.serial.disconnect().unwrap();
                        // Refresh port listings
                        self.ports.clear();
                        self.serial.request_port_scan().unwrap();

                        self.buffer.intentional_disconnect_clear();
                        // Clear the input box, but keep the user history!
                        self.user_input.clear();

                        self.menu = Menu::PortSelection(Pse::Ports);
                    }
                }
            }
        }
    }
    fn post_menu_vert_scroll(&mut self, up: bool) {
        if self.popup.is_some() {
            return;
        }
        // Logic for skipping the Custom Baud Entry field if it's not visible
        let is_custom_visible = self.baud_selection_state.on_last(COMMON_BAUD);
        use PortSelectionElement as Pse;
        match self.menu {
            Menu::PortSelection(e @ Pse::CustomBaud) if !is_custom_visible => {
                if up {
                    self.menu = e.prev().into();
                } else {
                    self.menu = e.next().into();
                }
            }
            _ => (),
        }
        // Logic for selecting the correct index of port when swapping off/to the table
        match self.menu {
            Menu::PortSelection(Pse::Ports) => {
                // Not using a match guard since it would always set it back to None
                if self.table_state.selected().is_none() {
                    if up {
                        self.table_state.select_last();
                    } else {
                        self.table_state.select_first();
                    }
                }
            }
            Menu::PortSelection(_) => self.table_state.select(None),
            _ => (),
        }
    }
    // consider making these some kind of trait method?
    // for the different menus and selections
    // not sure, things are gonna get interesting with the key presses
    fn scroll_menu_up(&mut self) {
        self.table_state.scroll_up_by(1);
    }
    fn scroll_menu_down(&mut self) {
        self.table_state.scroll_down_by(1);
    }
    /// Get max number of selectable elements for current popup.
    ///
    /// Panics if no popup is active.
    fn current_popup_item_count(&self) -> usize {
        let popup = self
            .popup
            .as_ref()
            .expect("popup needed to get max scroll index");
        match popup {
            #[cfg(feature = "macros")]
            PopupMenu::Macros => {
                1 + // Macros' category selector
                self.macros.visible_len()
            }
            PopupMenu::PortSettings => PortSettings::VISIBLE_FIELDS,
            PopupMenu::BehaviorSettings => Behavior::VISIBLE_FIELDS,
            PopupMenu::RenderingSettings => Rendering::VISIBLE_FIELDS,
            #[cfg(feature = "espflash")]
            // TODO proper scrollbar for espflash profiles
            PopupMenu::EspFlash => esp::ESPFLASH_BUTTON_COUNT + self.espflash.len(),
            #[cfg(feature = "logging")]
            PopupMenu::Logging => {
                1 + // Start/Stop Logging button
                Logging::VISIBLE_FIELDS
            }
        }
    }
    /// Gets corrected index of selected element.
    ///
    /// Returns None if either the Menu selector or the Macros category selector is active
    ///
    /// Used to select the current active element in tables.
    ///
    /// Panics if no popup is active.
    fn get_corrected_popup_item(&self) -> Option<usize> {
        let popup = self
            .popup
            .as_ref()
            .expect("popup needed to get corrected scroll index");
        let raw_scroll = self.popup_menu_item;
        // debug_assert!(raw_scroll > 0);
        match (popup, raw_scroll) {
            // Menu selector active
            (_, 0) => None,

            // Normal settings menus
            // Just correct for the category selector
            (PopupMenu::PortSettings, _)
            | (PopupMenu::RenderingSettings, _)
            | (PopupMenu::BehaviorSettings, _) => Some(self.popup_menu_item - 1),

            #[cfg(feature = "macros")]
            // Macro Categories selector active
            (PopupMenu::Macros, 1) => None,
            #[cfg(feature = "macros")]
            // Macros selection
            (PopupMenu::Macros, _) => Some(raw_scroll - 2),

            #[cfg(feature = "logging")]
            // Logging Toggle button active
            (PopupMenu::Logging, 1) => None,
            #[cfg(feature = "logging")]
            // Logging Settings active
            (PopupMenu::Logging, _) => Some(raw_scroll - 2),

            #[cfg(feature = "espflash")]
            // espflash pre-set action buttons
            (PopupMenu::EspFlash, _) if raw_scroll >= esp::ESPFLASH_BUTTON_COUNT + 1 => {
                Some(raw_scroll - (esp::ESPFLASH_BUTTON_COUNT + 1))
            }
            #[cfg(feature = "espflash")]
            // espflash user profiles
            (PopupMenu::EspFlash, _) => Some(raw_scroll - 1),
        }
    }
    pub fn draw(&mut self, terminal: &mut Terminal<impl Backend>) -> Result<()> {
        // let start = Instant::now();
        terminal.draw(|frame| self.render_app(frame))?;
        // debug!("A4: {:?}", start.elapsed());
        Ok(())
    }
    fn render_app(&mut self, frame: &mut Frame) {
        // self.buffer.update_terminal_size(frame.area().as_size());
        // TODO, make more reactive based on frame size :)
        let vertical_slices = Layout::vertical([
            Constraint::Fill(1),
            Constraint::Fill(4),
            Constraint::Fill(1),
        ])
        .split(frame.area());

        // let start = Instant::now();
        match self.menu {
            Menu::PortSelection(_) => {
                let big_text = BigText::builder()
                    .pixel_size(PixelSize::Quadrant)
                    .style(Style::new().blue())
                    .centered()
                    .lines(vec!["yap".blue().into()])
                    .build();
                frame.render_widget(big_text, vertical_slices[0]);

                self.port_selection(frame, vertical_slices[1]);
            }
            Menu::Terminal(prompt) => self.terminal_menu(frame, frame.area(), prompt),
        }
        // debug!("a1: {:?}", start.elapsed());

        // let start = Instant::now();
        self.render_popups(frame, frame.area());
        // debug!("a2: {:?}", start.elapsed());

        // let start = Instant::now();
        self.render_notifs(frame, frame.area());
        // debug!("a3: {:?}", start.elapsed());

        #[cfg(feature = "espflash")]
        self.espflash.render_espflash(frame, frame.area());

        // TODO:
        // self.render_error_messages(frame, frame.area());
    }
    fn render_notifs(&mut self, frame: &mut Frame, area: Rect) {
        if let Some(notif) = &self.notifs.inner {
            frame.render_widget(&self.notifs, area);
            if notif.shown_for() >= EXPIRE_TIME {
                _ = self.notifs.inner.take();
            }
        }
    }
    fn render_popups(&mut self, frame: &mut Frame, area: Rect) {
        let Some(popup) = &self.popup else {
            return;
        };

        let popup_color = match popup {
            #[cfg(feature = "macros")]
            PopupMenu::Macros => Color::Green,
            PopupMenu::RenderingSettings => Color::Red,
            PopupMenu::BehaviorSettings => Color::Blue,
            PopupMenu::PortSettings => Color::Cyan,
            #[cfg(feature = "espflash")]
            PopupMenu::EspFlash => Color::Magenta,
            #[cfg(feature = "logging")]
            PopupMenu::Logging => Color::Yellow,
        };

        let mut menu_selector_state = SingleLineSelectorState::new();

        // Set active states based on item index
        #[cfg(not(feature = "macros"))]
        {
            match self.popup_menu_item {
                0 => menu_selector_state.active = true,

                _ => menu_selector_state.active = false,
            }

            assert!(
                menu_selector_state.active == (self.popup_menu_item == 0),
                "Either a table element needs to be selected, or the menu title widget, but never both or neither."
            );
        }
        #[cfg(feature = "macros")]
        {
            match (self.popup_menu_item, popup) {
                (0, _) => {
                    menu_selector_state.active = true;
                    self.macros.categories_selector.active = false;
                }
                (1, PopupMenu::Macros) => {
                    menu_selector_state.active = false;
                    self.macros.categories_selector.active = true;
                }
                (_, _) => {
                    menu_selector_state.active = false;
                    self.macros.categories_selector.active = false;
                }
            }
            // Above match should ensure these pass without issue.
            let selector_range = match popup {
                PopupMenu::Macros => 0..=1,
                _ => 0..=0,
            };

            assert!(
                (menu_selector_state.active || self.macros.categories_selector.active)
                    == selector_range.contains(&self.popup_menu_item),
                "Either a table element needs to be selected, or the menu title widget, but never both or neither."
            );

            assert_eq!(
                menu_selector_state.active && self.macros.categories_selector.active,
                false,
                "Both selectors can't be active."
            );
        }

        let center_area = centered_rect_size(
            Size {
                width: area.width.min(60),
                height: area.height.min(16),
            },
            area,
        );
        frame.render_widget(Clear, center_area);

        let block = Block::bordered().border_style(Style::from(popup_color));

        frame.render_widget(&block, center_area);

        // let title_lines = ;

        let popup_menu_title_selector =
            SingleLineSelector::new(<PopupMenu as VariantNames>::VARIANTS.iter().map(|s| *s))
                .with_next_symbol(">")
                .with_prev_symbol("<")
                .with_space_padding(true);

        let title = {
            let mut line = center_area.clone();
            line.height = 1;
            line
        };

        // frame.render_widget(Clear, {
        //     let mut line = line;
        //     line.width += 2;
        //     line.x -= 1;
        //     line
        // });
        menu_selector_state.select(
            <PopupMenu as VariantArray>::VARIANTS
                .iter()
                .position(|v| v == popup)
                .unwrap(),
        );
        frame.render_stateful_widget(&popup_menu_title_selector, title, &mut menu_selector_state);

        let center_inner = block.inner(center_area);

        let settings_area = {
            let mut area = center_inner.clone();
            area.height = area.height.saturating_sub(2);
            area
        };

        let hint_text_area = {
            let mut area = center_inner.clone();
            area.y = area.bottom();
            area.height = 1;
            area
        };

        let line_area = {
            let mut area = center_inner.clone();
            area.y = area.bottom().saturating_sub(2);
            area.height = 1;
            area
        };
        let scrolling_text_area = {
            let mut area = center_inner.clone();
            area.y = area.bottom().saturating_sub(1);
            area.height = 1;
            area
        };

        let macros_table_area = {
            let mut area = center_inner.clone();
            area.height = area.height.saturating_sub(4);
            area.y = area.y.saturating_add(2);
            area
        };

        frame.render_widget(
            Block::new()
                .borders(Borders::TOP)
                .border_style(Style::from(popup_color)),
            line_area,
        );

        let scrollbar_style = Style::new().reset();

        let scrollbar = Scrollbar::new(ScrollbarOrientation::VerticalRight)
            .style(scrollbar_style)
            .begin_symbol(Some("↑"))
            .end_symbol(Some("↓"));

        // let content_length = match popup {
        //     PopupMenu::Macros => self.macros.visible_len(),
        //     // TODO find more clear way than checking this length
        //     PopupMenu::PortSettings => PortSettings::VISIBLE_FIELDS,
        //     PopupMenu::BehaviorSettings => Behavior::VISIBLE_FIELDS,
        //     PopupMenu::RenderingSettings => Rendering::VISIBLE_FIELDS,
        //     #[cfg(feature = "espflash")]
        //     // TODO proper scrollbar for espflash profiles
        //     PopupMenu::EspFlash => 4,
        //     #[cfg(feature = "logging")]
        //     PopupMenu::Logging => Logging::VISIBLE_FIELDS,
        // };

        let height = match popup {
            // TODO do for other popups?
            #[cfg(feature = "macros")]
            PopupMenu::Macros => macros_table_area.height,
            _ => settings_area.height,
        };

        // self.popup_scrollbar_state = self
        //     .popup_scrollbar_state
        //     .content_length(content_length.saturating_sub(height as usize));
        // self.popup_scrollbar_state = self
        //     .popup_scrollbar_state
        //     .position(self.popup_table_state.offset());

        // self.popup_scrollbar_state = self.popup_scrollbar_state.position();

        // let shared_table_state: TableState = {
        //     let mut table_state = TableState::new();

        //     table_state
        // };

        match popup {
            PopupMenu::PortSettings => {
                let selected = self.get_corrected_popup_item();
                self.popup_table_state.select(selected);
                self.popup_table_state.select_first_column();
                // let mut table_state = TableState::new()
                //     .with_selected_column(0)
                //     .with_selected(selected);

                frame.render_stateful_widget(
                    self.scratch.last_port_settings.as_table(),
                    settings_area,
                    &mut self.popup_table_state,
                );

                let text: &str = self
                    .popup_table_state
                    .selected()
                    .map(|i| PortSettings::DOCSTRINGS[i])
                    .unwrap_or(&"");
                render_scrolling_line(
                    text,
                    frame,
                    scrolling_text_area,
                    &mut self.popup_hint_scroll,
                );
                frame.render_widget(
                    Line::raw("Esc: Cancel | Enter: Save")
                        .all_spans_styled(Color::DarkGray.into())
                        .centered(),
                    hint_text_area,
                );
            }
            PopupMenu::BehaviorSettings => {
                let selected = self.get_corrected_popup_item();
                self.popup_table_state.select(selected);
                self.popup_table_state.select_first_column();

                frame.render_stateful_widget(
                    self.scratch.behavior.as_table(),
                    settings_area,
                    &mut self.popup_table_state,
                );
                let text: &str = self
                    .popup_table_state
                    .selected()
                    .map(|i| Behavior::DOCSTRINGS[i])
                    .unwrap_or(&"");
                render_scrolling_line(
                    text,
                    frame,
                    scrolling_text_area,
                    &mut self.popup_hint_scroll,
                );
                frame.render_widget(
                    Line::raw("Esc: Cancel | Enter: Save")
                        .all_spans_styled(Color::DarkGray.into())
                        .centered(),
                    hint_text_area,
                );
            }
            PopupMenu::RenderingSettings => {
                let selected = self.get_corrected_popup_item();
                self.popup_table_state.select(selected);
                self.popup_table_state.select_first_column();

                frame.render_stateful_widget(
                    self.scratch.rendering.as_table(),
                    settings_area,
                    &mut self.popup_table_state,
                );
                let text: &str = self
                    .popup_table_state
                    .selected()
                    .map(|i| Rendering::DOCSTRINGS[i])
                    .unwrap_or(&"");
                render_scrolling_line(
                    text,
                    frame,
                    scrolling_text_area,
                    &mut self.popup_hint_scroll,
                );
                frame.render_widget(
                    Line::raw("Esc: Cancel | Enter: Save")
                        .all_spans_styled(Color::DarkGray.into())
                        .centered(),
                    hint_text_area,
                );
            }
            #[cfg(feature = "macros")]
            PopupMenu::Macros => {
                let new_seperator = {
                    let mut area = center_inner.clone();
                    area.y = area.top().saturating_add(1);
                    area.height = 1;
                    area
                };
                let categories_area = {
                    let mut area = center_inner.clone();
                    area.y = area.top();
                    area.height = 1;
                    area
                };
                frame.render_widget(
                    Block::new()
                        .borders(Borders::TOP)
                        .border_style(Style::from(popup_color)),
                    new_seperator,
                );
                // frame.render_widget(
                //     Line::raw(" <     All Macros    > ").centered(),
                //     categories_area,
                // );

                if self.macros.search_input.value().is_empty() {
                    let categories_iter = ["Has Bytes", "Strings Only", "All Macros"]
                        .iter()
                        .map(|s| *s)
                        .map(String::from)
                        .map(Line::raw)
                        .chain(self.macros.categories().map(String::from).map(Line::raw));
                    let categories_selector = SingleLineSelector::new(categories_iter)
                        .with_next_symbol(">")
                        .with_prev_symbol("<")
                        .with_size_hint(popup_menu_title_selector.max_chars());
                    frame.render_stateful_widget(
                        &categories_selector,
                        categories_area,
                        &mut self.macros.categories_selector,
                    );
                } else {
                    // Get the search text
                    let search_text = self.macros.search_input.value();
                    // Center the search line in the area
                    let search_line = Line::raw(search_text).centered();

                    let width = categories_area.width.max(1) - 1; // So the cursor doesn't bleed off the edge

                    // Calculate padding for centered text
                    let text_width = search_line.width() as u16;
                    let pad_left = if width > text_width {
                        (width - text_width) / 2
                    } else {
                        0
                    };

                    // Can't scroll centered lines horizontally??
                    let scroll = self.macros.search_input.visual_scroll(width as usize);
                    let input_text = Paragraph::new(search_line).scroll((0, scroll as u16));

                    frame.render_widget(input_text, categories_area);

                    // Cursor logic: trailing edge after the last char, with center offset
                    let cursor_pos = self.macros.search_input.visual_cursor();
                    let centered_offset = pad_left as i32 + (cursor_pos as i32 - scroll as i32);
                    let cursor_x = categories_area.x + centered_offset.max(0) as u16;

                    frame.set_cursor_position((cursor_x + 1, categories_area.y));
                }

                let mut table = self
                    .macros
                    .as_table(&self.keybinds, self.settings.behavior.fuzzy_macro_match);

                // if !matches!(self.macros.ui_state, MacrosPrompt::None) {
                //     table = table.dark_gray();
                // };

                let selected = self.get_corrected_popup_item();
                self.popup_table_state.select(selected);
                self.popup_table_state.select_first_column();
                frame.render_stateful_widget(table, macros_table_area, &mut self.popup_table_state);

                // frame.render_widget(
                //     Line::raw("Ctrl+N: New")
                //         .all_spans_styled(Color::DarkGray.into())
                //         .centered(),
                //     new_seperator,
                // );

                frame.render_widget(
                    Line::raw("Ctrl+R: Reload")
                        .all_spans_styled(Color::DarkGray.into())
                        .centered(),
                    line_area,
                );

                frame.render_widget(
                    Line::raw("Esc: Close | Enter: Send")
                        .all_spans_styled(Color::DarkGray.into())
                        .centered(), // .dark_gray()
                    hint_text_area,
                );

                if let Some(index) = self.popup_table_state.selected() {
                    // let text: &str = self
                    //     .popup_table_state
                    //     .selected()
                    //     .map(|i| )
                    //     .unwrap_or(&"");

                    let (tag, content) = self.macros.filtered_macro_iter().nth(index).unwrap();
                    // for now i guess
                    // TOOD replace with fancy line preview
                    let macro_preview = content.as_str();
                    let line = if !content.sensitive {
                        macro_preview.to_line().italic()
                    } else {
                        Line::from(span!("[SENSITIVE]")).italic()
                    };
                    // let line = if matches!(macro_binding.content, MacroContent::Bytes { .. }) {
                    //     line.light_blue()
                    // } else {
                    //     line
                    // };
                    render_scrolling_line(
                        line,
                        frame,
                        scrolling_text_area,
                        &mut self.popup_hint_scroll,
                    );
                } else {
                    if self.macros.is_empty() {
                        frame.render_widget(
                            Line::raw("No macros! Try making one!").centered(),
                            scrolling_text_area,
                        );
                    } else {
                        frame.render_widget(
                            Line::raw("Select a macro to preview.").centered(),
                            scrolling_text_area,
                        );
                    }
                }
                // match prompt {
                //     _ => (),
                // };
            }
            #[cfg(feature = "logging")]
            PopupMenu::Logging => {
                let new_seperator = {
                    let mut area = center_inner.clone();
                    area.y = area.top().saturating_add(1);
                    area.height = 1;
                    area
                };
                let bins_area = {
                    let mut area = center_inner.clone();
                    area.y = area.top().saturating_add(2);
                    area.height = area.height.saturating_sub(4);
                    area
                };
                let line_block = Block::new()
                    .borders(Borders::TOP)
                    .border_style(Style::from(popup_color));

                // TODO make dynamic
                let log_path_text = r"Saving to: $app_dir/logs/";
                let log_path_line = Line::raw(log_path_text)
                    .all_spans_styled(Color::DarkGray.into())
                    .centered();

                if log_path_line.width() <= line_area.width as usize {
                    frame.render_widget(log_path_line, line_area);
                } else {
                    frame.render_widget(log_path_line.right_aligned(), line_area);
                }

                frame.render_widget(
                    Line::raw("Esc: Close | Enter: Select/Save")
                        .all_spans_styled(Color::DarkGray.into())
                        .centered(),
                    hint_text_area,
                );

                let logging_active = self.buffer.log_handle.logging_active();
                let toggle_button = toggle_logging_button(logging_active);
                let logging_toggle_selected = self.popup_menu_item == 1;
                if logging_toggle_selected {
                    self.popup_table_state.select(Some(0));
                    self.popup_table_state.select_first_column();

                    frame.render_stateful_widget(
                        toggle_button,
                        settings_area,
                        &mut self.popup_table_state,
                    );
                    frame.render_widget(&line_block, new_seperator);
                    frame.render_widget(self.scratch.logging.as_table(), bins_area);

                    let text = if logging_active {
                        "Stop logging and close the current log files."
                    } else {
                        "Create new log files and begin logging."
                    };
                    render_scrolling_line(
                        text,
                        frame,
                        scrolling_text_area,
                        &mut self.popup_hint_scroll,
                    );
                } else {
                    let selected = self.get_corrected_popup_item();
                    self.popup_table_state.select(selected);
                    self.popup_table_state.select_first_column();

                    frame.render_widget(toggle_button, settings_area);
                    frame.render_widget(&line_block, new_seperator);
                    frame.render_stateful_widget(
                        self.scratch.logging.as_table(),
                        bins_area,
                        &mut self.popup_table_state,
                    );

                    let text: &str = self
                        .popup_table_state
                        .selected()
                        .map(|i| Logging::DOCSTRINGS[i])
                        .unwrap_or(&"");
                    render_scrolling_line(
                        text,
                        frame,
                        scrolling_text_area,
                        &mut self.popup_hint_scroll,
                    );
                }
                frame.render_widget(
                    Line::raw("Settings:")
                        .all_spans_styled(Color::DarkGray.into())
                        .centered(),
                    new_seperator,
                );

                // if let Some(profile) = self
                //     .popup_table_state
                //     .selected()
                //     .and_then(|idx| self.espflash.profile_from_index(idx))
                // {
                //     let hint_text = match profile {
                //         esp::EspProfile::Bins(_) => "Flash profile binaries to ESP Flash.",
                //         esp::EspProfile::Elf(profile) if profile.ram => {
                //             "Load profile ELF into RAM."
                //         }
                //         esp::EspProfile::Elf(_) => "Flash profile ELF to ESP Flash.",
                //     };
                //     render_scrolling_line(
                //         hint_text,
                //         frame,
                //         scrolling_text_area,
                //         &mut self.popup_desc_scroll,
                //     );
                // }

                // let hints = [
                //     "Attempt to remotely reset the chip.",
                //     "Attempt to reboot into bootloader. Shift/Ctrl to skip check.",
                //     "Query ESP for Flash Size, MAC Address, etc.",
                //     "Erase all flash contents.",
                // ];
                // if let Some(idx) = self.popup_table_state.selected() {
                //     if let Some(&hint_text) = hints.get(idx) {
                //         render_scrolling_line(
                //             hint_text,
                //             frame,
                //             scrolling_text_area,
                //             &mut self.popup_desc_scroll,
                //         );
                //     }
                // }
            }
            #[cfg(feature = "espflash")]
            PopupMenu::EspFlash => {
                let new_seperator = {
                    let mut area = center_inner.clone();
                    area.y = area.top().saturating_add(4);
                    area.height = 1;
                    area
                };
                let bins_area = {
                    let mut area = center_inner.clone();
                    area.y = area.top().saturating_add(5);
                    area.height = area.height.saturating_sub(7);
                    area
                };
                let line_block = Block::new()
                    .borders(Borders::TOP)
                    .border_style(Style::from(popup_color));
                frame.render_widget(
                    Line::raw("Powered by esp-rs/espflash!")
                        .all_spans_styled(Color::DarkGray.into())
                        .centered(),
                    line_area,
                );

                frame.render_widget(
                    Line::raw("Esc: Close | Enter: Select")
                        .all_spans_styled(Color::DarkGray.into())
                        .centered(),
                    hint_text_area,
                );

                let profiles_selected = self.popup_menu_item >= esp::ESPFLASH_BUTTON_COUNT + 1;

                // if self.popup_menu_item ==
                if profiles_selected {
                    let corrected_index = self.get_corrected_popup_item().unwrap();

                    frame.render_widget(esp::espflash_buttons(), settings_area);
                    frame.render_widget(&line_block, new_seperator);
                    frame.render_stateful_widget(
                        self.espflash.profiles_table(),
                        bins_area,
                        &mut TableState::new()
                            .with_selected_column(0)
                            .with_selected(Some(corrected_index)),
                    );
                    if let Some(profile) = self.espflash.profile_from_index(corrected_index) {
                        let chip = match &profile {
                            esp::EspProfile::Bins(bins) => {
                                if let Some(chip) = &bins.expected_chip {
                                    Cow::from(chip.to_string().to_ascii_uppercase())
                                } else {
                                    Cow::from("ESP")
                                }
                            }
                            esp::EspProfile::Elf(elf) => {
                                if let Some(chip) = &elf.expected_chip {
                                    Cow::from(chip.to_string().to_ascii_uppercase())
                                } else {
                                    Cow::from("ESP")
                                }
                            }
                        };

                        let hint_text = match &profile {
                            esp::EspProfile::Bins(bins) if bins.bins.len() == 1 => {
                                format!("Flash selected profile binary to {chip} Flash.")
                            }
                            esp::EspProfile::Bins(_) => {
                                format!("Flash selected profile binaries to {chip} Flash.")
                            }
                            esp::EspProfile::Elf(profile) if profile.ram => {
                                format!("Load selected profile ELF into {chip} RAM.")
                            }
                            esp::EspProfile::Elf(_) => {
                                format!("Flash selected profile ELF to {chip} Flash.")
                            }
                        };
                        render_scrolling_line(
                            hint_text,
                            frame,
                            scrolling_text_area,
                            &mut self.popup_hint_scroll,
                        );
                    }
                } else {
                    let corrected_index = self.get_corrected_popup_item();

                    frame.render_stateful_widget(
                        esp::espflash_buttons(),
                        settings_area,
                        &mut TableState::new()
                            .with_selected_column(0)
                            .with_selected(corrected_index),
                    );
                    frame.render_widget(&line_block, new_seperator);
                    frame.render_widget(self.espflash.profiles_table(), bins_area);

                    let hints = [
                        "Attempt to remotely reset the chip.",
                        "Attempt to reboot into bootloader. Shift/Ctrl to skip check.",
                        "Query ESP for Flash Size, MAC Address, etc.",
                        "Erase all flash contents.",
                    ];
                    if let Some(idx) = corrected_index {
                        if let Some(&hint_text) = hints.get(idx) {
                            render_scrolling_line(
                                hint_text,
                                frame,
                                scrolling_text_area,
                                &mut self.popup_hint_scroll,
                            );
                        }
                    }
                }
                frame.render_widget(
                    Line::raw("Flash Profiles | Ctrl+R: Reload")
                        .all_spans_styled(Color::DarkGray.into())
                        .centered(),
                    new_seperator,
                );
            }
        }
        // TODO
        // shrink scrollbar and change content length based on if its for a submenu or not
        let content_length = self.current_popup_item_count();
        let mut scrollbar_state =
            ScrollbarState::new(content_length.saturating_sub(height as usize))
                .position(self.popup_table_state.offset());

        frame.render_stateful_widget(
            scrollbar,
            center_inner.offset(Offset { x: 1, y: 0 }).inner(Margin {
                horizontal: 0,
                vertical: 1,
            }),
            &mut scrollbar_state,
        );
    }

    // #[instrument(skip(self))]

    // fn update_line_count(&mut self) -> usize {
    //     if self.buffer_wrapping {
    //         self.buffer.update_line_count()
    //     } else {
    //         self.buffer.lines.len()
    //     }
    // }

    pub fn terminal_menu<'a>(
        &mut self,
        frame: &mut Frame,
        area: Rect,
        prompt: TerminalPrompt,
        // buffer: impl Iterator<Item = Line<'a>>,
        // state: &mut TableState
    ) {
        let prompt_shown = match prompt {
            TerminalPrompt::DisconnectPrompt => true,
            TerminalPrompt::AttemptReconnectPrompt => true,
            TerminalPrompt::None => false,
        };
        let [terminal_area, line_area, input_area] = vertical![*=1, ==1, ==1].areas(area);
        let [input_symbol_area, input_area] = horizontal![==1, *=1].areas(input_area);

        let start = Instant::now();
        if self.settings.rendering.hex_view {
            self.buffer.render_hex(terminal_area, frame.buffer_mut());
        } else {
            frame.render_widget(&mut self.buffer, terminal_area);
        }
        // debug!("1: {:?}", start.elapsed());
        let start = Instant::now();

        // let total_lines = para.line_count(terminal_area.width.saturating_sub(1));

        // info!(
        //     "total rendered lines: {total_lines}, line vec count: {}",
        //     self.buffer.strings.len()
        // );

        // info!("{}", total_lines);

        // if self.buffer_stick_to_bottom {
        //     let new_pos = total_lines.saturating_sub(terminal.height as usize);
        //     self.buffer_scroll_state = self.buffer_scroll_state.position(new_pos);
        //     vert_scroll = new_pos as u16;
        // }

        // info!(
        //     "scroll: {vert_scroll}, lines: {}, term height: {}",
        //     self.line_count(),
        //     self.last_terminal_size.height
        // );

        // frame.render_widget(Clear, terminal);

        // self.buffer_scroll_state = self.buffer_scroll_state.content_length(total_lines);
        // self.buffer_rendered_lines = total_lines;
        // maybe debug_assert this when we roll our own line-counting?

        let (port_state, serial_signals, port_text) = {
            let port_status_guard = self.serial.port_status.load();
            let port_state = port_status_guard.inner;

            let port_text = match &port_status_guard.current_port {
                Some(port_info) => {
                    if port_state.is_healthy() || port_state.is_lent_out() {
                        let baud_rate = self.serial.port_settings.load().baud_rate;
                        port_info.info_as_string(Some(baud_rate))
                    } else {
                        // Might remove later
                        let info = port_info.info_as_string(None);
                        format!("[!] {info} [!]")
                    }
                }
                None => {
                    error!("Port info missing during render!");
                    "Port info missing!".to_owned()
                }
            };

            (port_state, port_status_guard.signals.clone(), port_text)
        };

        repeating_pattern_widget(frame, line_area, self.repeating_line_flip, port_state);

        let widget_margin: u16 = if area.width >= 100 { 3 } else { 0 };

        #[cfg(debug_assertions)]
        {
            let line = Line::raw(format!(
                "Entries: {} | Lines: {}",
                self.buffer.port_lines_len(),
                self.buffer.combined_height()
            ))
            .right_aligned();
            frame.render_widget(
                line,
                line_area.inner(Margin {
                    horizontal: widget_margin,
                    vertical: 0,
                }),
            );
        }

        #[cfg(not(debug_assertions))]
        {
            let line =
                Line::raw(format!("Lines: {}", self.buffer.port_lines_len())).right_aligned();
            frame.render_widget(
                line,
                line_area.inner(Margin {
                    horizontal: widget_margin,
                    vertical: 0,
                }),
            );
        }

        if self.action_queue.is_empty() {
            let port_name_line = Line::raw(port_text).centered();
            frame.render_widget(port_name_line, line_area);
        } else {
            let port_name_line =
                Line::raw(format!("Queued Actions: {}", self.action_queue.len())).centered();
            frame.render_widget(port_name_line, line_area);
        }

        {
            let reversed_if_true = |signal: bool| -> Modifier {
                if signal {
                    Modifier::REVERSED
                } else {
                    Modifier::empty()
                }
            };

            let (dtr, rts, cts, dsr, ri, cd) = {
                let dtr = reversed_if_true(serial_signals.dtr);
                let rts = reversed_if_true(serial_signals.rts);

                let cts = reversed_if_true(serial_signals.cts);
                let dsr = reversed_if_true(serial_signals.dsr);
                let ri = reversed_if_true(serial_signals.ri);
                let cd = reversed_if_true(serial_signals.cd);

                (dtr, rts, cts, dsr, ri, cd)
            };

            let signals_spans = vec![
                span!["["],
                span![dtr;"DTR"],
                span![" "],
                span![rts;"RTS"],
                span!["|"],
                span![cts;"CTS"],
                span![" "],
                span![dsr;"DSR"],
                span![" "],
                span![ri;"RI"],
                span![" "],
                span![cd;"CD"],
                span!["]"],
            ];
            let signals_line = Line::from(signals_spans);
            frame.render_widget(
                signals_line,
                line_area.offset(Offset {
                    x: widget_margin as i32,
                    y: 0,
                }),
            );
        }

        let input_style = match (&self.failed_send_at, self.user_input.all_text_selected) {
            (Some(instant), _) if instant.elapsed() < FAILED_SEND_VISUAL_TIME => {
                Style::new().on_red()
            }
            (Some(instant), true) if instant.elapsed() < FAILED_SEND_VISUAL_TIME => {
                Style::new().reversed().on_red()
            }
            (_, true) => Style::new().reversed(),
            _ => Style::new(),
        };

        // TODO have this turn into `§` or something when in bytes mode.
        let input_symbol = Span::raw(">").style(if port_state.is_healthy() {
            input_style.not_reversed().green()
        } else {
            input_style.red()
        });

        frame.render_widget(input_symbol, input_symbol_area);

        let should_position_cursor = !prompt_shown && self.popup.is_none();

        if self.user_input.input_box.value().is_empty() {
            // Leading space leaves room for full-width cursors.
            // TODO make binding hint dynamic (should maybe cache?)
            let port_settings_combo = self
                .keybinds
                .port_settings_hint
                .as_ref()
                .map(CompactString::as_str)
                .unwrap_or_else(|| "UNBOUND");
            let input_hint = Line::raw(format!(
                " Input goes here. `{key}` for port settings.",
                key = port_settings_combo
            ))
            .style(input_style)
            .dark_gray()
            .italic();
            frame.render_widget(input_hint, input_area);
            if should_position_cursor {
                frame.set_cursor_position(input_area.as_position());
            }
        } else {
            let width = input_area.width.max(1) - 1; // So the cursor doesn't bleed off the edge
            let scroll = self.user_input.input_box.visual_scroll(width as usize);
            let input_text = Paragraph::new(self.user_input.input_box.value())
                .scroll((0, scroll as u16))
                .style(input_style);
            frame.render_widget(input_text, input_area);
            if should_position_cursor {
                frame.set_cursor_position((
                    // Put cursor past the end of the input text
                    input_area.x
                        + ((self.user_input.input_box.visual_cursor()).max(scroll) - scroll) as u16,
                    input_area.y,
                ));
            }
        }

        match prompt {
            TerminalPrompt::DisconnectPrompt => DisconnectPrompt::render_prompt_block_popup(
                Some("Disconnect from port?"),
                None,
                Style::new().blue(),
                frame,
                area,
                &mut self.table_state,
            ),
            TerminalPrompt::AttemptReconnectPrompt => {
                AttemptReconnectPrompt::render_prompt_block_popup(
                    Some("Reconnect to port?"),
                    None,
                    Style::new().red(),
                    frame,
                    area,
                    &mut self.table_state,
                )
            }
            TerminalPrompt::None => (),
        }

        if prompt_shown {
            // let area = centered_rect(30, 30, area);
            // let save_device_prompt =
            //     DisconnectPrompt::prompt_table_block("Disconnect from port?", Style::new().blue());

            // frame.render_waidget(Clear, area);
            // frame.render_stateful_widget(save_device_prompt, area, &mut self.table_state);
        }
        // debug!("2: {:?}", start.elapsed());
    }

    fn port_selection(&mut self, frame: &mut Frame, given_area: Rect) {
        let area = if frame.area().width < 45 {
            given_area
        } else {
            let [_, middle_area, _] = horizontal![==25%, ==50%, ==25%].areas(given_area);
            middle_area
        };
        let block = Block::bordered()
            .title("Port Selection")
            .border_style(Style::new().blue())
            .title_style(Style::reset())
            .title_alignment(ratatui::layout::Alignment::Center);

        let rows: Vec<Row> = self
            .ports
            .iter()
            .map(|p| {
                Row::new(vec![
                    // Column 1: Port name
                    Cow::Borrowed(p.port_name.as_str()),
                    // Column 2: Port info
                    match &p.port_type {
                        SerialPortType::UsbPort(usb) => {
                            let mut text = format!("[USB] {:04X}:{:04X}", usb.vid, usb.pid);
                            if let Some(serial_number) = &usb.serial_number {
                                text.push_str(" S/N: ");
                                text.push_str(serial_number);
                            }

                            Cow::Owned(text)
                        }
                        SerialPortType::BluetoothPort => Cow::Borrowed("[Bluetooth]"),
                        SerialPortType::PciPort => Cow::Borrowed("[PCI]"),
                        SerialPortType::Unknown if p.port_name == MOCK_PORT_NAME => {
                            Cow::Borrowed("[Mock Testing Port]")
                        }
                        // TODO make more reactive for Unix stuff
                        SerialPortType::Unknown => Cow::Borrowed("[Unspecified]"),
                    },
                ])
            })
            .collect();
        let widths = [Constraint::Percentage(25), Constraint::Percentage(75)];

        let table = Table::new(rows, widths)
            .row_highlight_style(Style::new().reversed())
            .highlight_spacing(HighlightSpacing::Always)
            .highlight_symbol(">>");

        let [
            table_area,
            mut filler_or_custom_baud_entry,
            mut baud_text_area,
            mut baud_selector,
            more_options,
        ] = vertical![*=1, ==1, ==1, ==1, ==1].areas(block.inner(area));

        let custom_visible = self.baud_selection_state.on_last(COMMON_BAUD);
        let custom_selected = matches!(
            self.menu,
            Menu::PortSelection(PortSelectionElement::CustomBaud)
        );
        if custom_visible {
            // ------ "Baud Rate:" [115200]
            std::mem::swap(&mut filler_or_custom_baud_entry, &mut baud_text_area);
            //  "Baud Rate:" ------ [115200]
            std::mem::swap(&mut filler_or_custom_baud_entry, &mut baud_selector);
            //  "Baud Rate:" [115200] ------
        }

        // let static_baud = line![format!("← {current_baud} →")];

        let baud_text = line!["Baud Rate:"];

        let more_options_button = span![format!("[More options]")];

        frame.render_widget(block, area);

        if self.popup.is_none() {
            frame.render_stateful_widget(table, table_area, &mut self.table_state);
        } else {
            frame.render_widget(table, table_area);
        }

        frame.render_widget(baud_text.centered(), baud_text_area);

        self.baud_selection_state.active = matches!(
            self.menu,
            Menu::PortSelection(PortSelectionElement::BaudSelection)
        );

        let selector = SingleLineSelector::new(COMMON_BAUD.iter().map(|&b| {
            if b == 0 {
                "Custom:".to_string()
            } else {
                format!("{b:^6}")
            }
        }));

        frame.render_stateful_widget(&selector, baud_selector, &mut self.baud_selection_state);

        if custom_visible {
            let [left, input_area, right] =
                horizontal![*=1, ==10, *=1].areas(filler_or_custom_baud_entry);

            let style = if custom_selected {
                Style::new().reversed()
            } else {
                Style::new()
            };

            frame.render_widget(Line::from(Span::styled("[", style)).right_aligned(), left);

            let user_text: &str = self.user_input.input_box.value();

            let user_input = Line::raw(if user_text.is_empty() { " " } else { user_text });

            let width = input_area.width.max(1) - 1; // So the cursor doesn't bleed off the edge
            let scroll = self.user_input.input_box.visual_scroll(width as usize);
            let input_text = Paragraph::new(user_input)
                .scroll((0, scroll as u16))
                .style(style);
            frame.render_widget(input_text, input_area);

            if custom_selected {
                frame.set_cursor_position((
                    // Put cursor past the end of the input text
                    input_area.x
                        + ((self.user_input.input_box.visual_cursor()).max(scroll) - scroll) as u16,
                    input_area.y,
                ));
            }

            frame.render_widget(Line::from(Span::styled("]", style)).left_aligned(), right);
        }

        // let mut state = SingleLineSelectorState {
        //     current_index: COMMON_BAUD_DEFAULT,
        // };

        // frame.render_widget(static_baud.centered(), baud_selector);

        if matches!(
            self.menu,
            Menu::PortSelection(PortSelectionElement::MoreOptions)
        ) {
            frame.render_widget(
                Line::from(more_options_button.reversed()).centered(),
                more_options,
            );
        } else {
            frame.render_widget(Line::from(more_options_button).centered(), more_options);
        }
    }
    fn refresh_scratch(&mut self) {
        self.scratch = self.settings.clone();
    }
    fn show_popup(&mut self, popup: ShowPopupAction) {
        self.popup = Some(popup.into());
        self.refresh_scratch();
        self.popup_hint_scroll = -2;
        self.popup_menu_item = 0;

        self.tx
            .send(Tick::Scroll.into())
            .map_err(|e| e.to_string())
            .unwrap();
    }
    fn dismiss_popup(&mut self) {
        self.refresh_scratch();
        self.popup.take();
        self.popup_menu_item = 0;
        self.popup_hint_scroll = -2;
    }
    fn scroll_popup(&mut self, next: bool) {
        let Some(popup) = &mut self.popup else {
            return;
        };

        let mut new_popup = if next { popup.next() } else { popup.prev() };

        std::mem::swap(popup, &mut new_popup);

        self.refresh_scratch();
        self.popup_hint_scroll = -2;
        #[cfg(feature = "macros")]
        self.macros.search_input.reset();
    }
}

pub fn repeating_pattern_widget(
    frame: &mut Frame,
    area: Rect,
    swap: bool,
    port_state: InnerPortStatus,
) {
    let repeat_count = area.width as usize / 2;
    let remainder = area.width as usize % 2;
    let base_pattern = if swap { "-~" } else { "~-" };

    let pattern = if remainder == 0 {
        base_pattern.repeat(repeat_count)
    } else {
        base_pattern.repeat(repeat_count) + &base_pattern[..1]
    };

    let pattern_widget = ratatui::widgets::Paragraph::new(pattern);
    let pattern_widget = match port_state {
        InnerPortStatus::Connected => pattern_widget.green(),
        InnerPortStatus::LentOut => pattern_widget.yellow(),
        InnerPortStatus::PrematureDisconnect => pattern_widget.red(),
        InnerPortStatus::Idle => pattern_widget.red(),
    };
    frame.render_widget(pattern_widget, area);
}

fn wrapping_prompt_scroll(len: usize, table_state: &mut TableState, up: bool) {
    match (table_state.selected(), up) {
        (Some(index), true) => {
            // if would overflow scrolling up
            if index.overflowing_sub(1).1 {
                table_state.select_last();
            } else {
                table_state.scroll_up_by(1);
            }
        }
        (Some(index), false) => {
            // if would overflow scrolling down
            if (index + 1) >= len {
                table_state.select_first();
            } else {
                table_state.scroll_down_by(1);
            }
        }
        (None, true) => table_state.select_first(),
        (None, false) => table_state.select_last(),
    }
}

/// If the given text is longer than the supplied area, it will be scrolled out of and then back in to the area.
pub fn render_scrolling_line<'a, T: Into<Line<'a>>>(
    text: T,
    frame: &mut Frame,
    mut area: Rect,
    scroll: &mut i32,
) {
    let orig_area = area.clone();
    assert_eq!(area.height, 1, "Scrolling line expects a height of 1 only.");

    let line: Line = text.into();
    let total_width: usize = line.width();

    let enough_room = total_width as u16 <= area.width;
    let overflow_amount = (total_width as u16).saturating_sub(area.width);

    let (scroll_x, offset_x): (u16, u16) = {
        if total_width as u16 <= area.width {
            (0, 0)
        } else if overflow_amount < 10 {
            match scroll {
                _pause if *scroll <= 0 => (0, 0),
                to_left if *scroll <= overflow_amount as i32 => (*to_left as u16, 0),
                _left_pause if *scroll <= (overflow_amount as i32) + 3 => {
                    (overflow_amount as u16, 0)
                }
                to_right
                    if *scroll
                        <= (overflow_amount as i32) + 3 + (overflow_amount as i32) as i32 =>
                {
                    (
                        (overflow_amount as u16)
                            - ((*to_right as u16) - ((overflow_amount as u16) + 3)),
                        0,
                    )
                }
                scroll_reset
                    if *scroll > (overflow_amount as i32) + 3 + (overflow_amount as i32) =>
                {
                    *scroll_reset = -2;
                    (0, 0)
                }
                _ => (0, 0),
            }
        } else {
            match scroll {
                _pause if *scroll <= 0 => (0, 0),
                to_left if *scroll <= total_width as i32 => (*to_left as u16, 0),
                from_right if *scroll < (total_width as u16 + area.width) as i32 => {
                    (0, (*from_right as u16 - total_width as u16))
                }
                scroll_reset if *scroll >= (total_width as u16 + area.width) as i32 => {
                    *scroll_reset = -2;
                    (0, 0)
                }
                _ => (0, 0),
            }
        }
    };
    // debug!("scroll_x: {scroll_x}, offset_x: {offset_x}");
    let para = Paragraph::new(line).scroll((0, if offset_x > 0 { 0 } else { scroll_x as u16 }));
    if offset_x > 0 {
        area.width = u16::min(offset_x, area.width);
    }
    frame.render_widget(
        para,
        area.offset(Offset {
            x: if offset_x > 0 {
                orig_area.width.saturating_sub(offset_x).into()
            } else {
                0
            },
            y: 0,
        }),
    );
}
