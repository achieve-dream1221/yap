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
    keybinds::{Keybinds, methods::*},
    macros::{MacroNameTag, Macros},
    notifications::{
        EMERGE_TIME, EXPAND_TIME, EXPIRE_TIME, Notification, Notifications, PAUSE_TIME,
    },
    serial::{
        PrintablePortInfo, ReconnectType, Reconnections, SerialEvent,
        handle::SerialHandle,
        worker::{InnerPortStatus, MOCK_PORT_NAME},
    },
    settings::{Behavior, Logging, PortSettings, Rendering, Settings},
    traits::{LastIndex, LineHelpers, ToggleBool},
    tui::{
        centered_rect_size,
        logging::toggle_logging_button,
        prompts::{DisconnectPrompt, PromptTable, centered_rect},
        single_line_selector::{SingleLineSelector, SingleLineSelectorState, StateBottomed},
    },
};

#[cfg(feature = "logging")]
use crate::keybinds::logging_methods::*;

#[cfg(feature = "espflash")]
use crate::keybinds::esp_methods::*;
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
    /// Used to trigger consumption of the Macro TX Queue
    MacroTx,
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

// Maybe have the buffer in the TUI struct?

/// Struct for working copies of settings that the user is editing
pub struct ScratchSpace {
    port: PortSettings,
    behavior: Behavior,
}

pub struct App {
    state: RunningState,
    menu: Menu,

    tx: Sender<Event>,
    rx: Receiver<Event>,

    table_state: TableState,
    baud_selection_state: SingleLineSelectorState,
    popup: Option<PopupMenu>,
    popup_desc_scroll: i32,
    popup_table_state: TableState,
    popup_single_line_state: SingleLineSelectorState,
    popup_scrollbar_state: ScrollbarState,

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

    macros: Macros,
    macros_tx_queue: VecDeque<(Option<KeyCombination>, MacroNameTag)>,

    settings: Settings,
    scratch: Settings,
    keybinds: Keybinds,

    #[cfg(feature = "espflash")]
    espflash: EspFlashState,

    #[cfg(feature = "logging")]
    logging_toggle_selected: bool,
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
            settings.logging.clone(),
        );
        // debug!("{buffer:#?}");
        Self {
            state: RunningState::Running,
            menu: Menu::PortSelection(PortSelectionElement::Ports),
            popup: None,
            popup_desc_scroll: -2,
            popup_scrollbar_state: ScrollbarState::default(),
            table_state: TableState::new().with_selected(Some(0)),
            baud_selection_state: SingleLineSelectorState::new().with_selected(selected_baud_index),
            popup_table_state: TableState::new(),
            popup_single_line_state: SingleLineSelectorState::new(),
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
            macros: Macros::new(),
            macros_tx_queue: VecDeque::new(),
            scratch: settings.clone(),
            settings,
            keybinds: Keybinds::new(),
            notifs: Notifications::new(tx.clone()),
            tx,
            rx,

            #[cfg(feature = "espflash")]
            espflash: EspFlashState::new(),
            #[cfg(feature = "logging")]
            logging_toggle_selected: false,
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
                    self.buffer
                        .log_handle
                        .log_port_connected(current_port.to_owned(), reconnect.clone())
                        .unwrap();
                } else {
                    error!("Was told about a port connection but no current port exists!");
                    panic!("Was told about a port connection but no current port exists!");
                }

                self.buffer.scroll_by(0);
            }
            Event::Serial(SerialEvent::Disconnected(reason)) => {
                #[cfg(feature = "espflash")]
                self.espflash.reset();
                // self.menu = Menu::PortSelection;
                // if let Some(reason) = reason {
                //     self.notify(format!("Disconnected from port! {reason}"), Color::Red);
                // }
                self.buffer.log_handle.log_port_disconnected(false).unwrap();
                if reason.is_some() {
                    let reconnect_text = match &self.settings.last_port_settings.reconnections {
                        Reconnections::Disabled => "Not attempting to reconnect.",
                        Reconnections::LooseChecks => "Attempting to reconnect (loose checks).",
                        Reconnections::StrictChecks => "Attempting to reconnect (strict checks).",
                    };
                    self.notifs.notify_str(
                        format!("Disconnected from port! {reconnect_text}"),
                        Color::Red,
                    );
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
                EspEvent::Error(e) => self.notifs.notify_str(&e, Color::Red),
                _ => self.espflash.consume_event(esp_event),
            },
            Event::Tick(Tick::PerSecond) => match self.menu {
                Menu::Terminal(TerminalPrompt::None) => {
                    let port_status = &self.serial.port_status.load().inner;

                    let reconnections_allowed =
                        self.serial.port_settings.load().reconnections != Reconnections::Disabled;
                    if !port_status.is_healthy()
                        && !port_status.is_lent_out()
                        && reconnections_allowed
                    {
                        self.repeating_line_flip.flip();
                        self.serial.request_reconnect().unwrap();
                    }
                }
                // If disconnect prompt is open, pause reacting to the ticks
                Menu::Terminal(TerminalPrompt::DisconnectPrompt) => (),
                Menu::PortSelection(_) => {
                    self.serial.request_port_scan().unwrap();
                }
            },
            Event::Tick(Tick::Scroll) => {
                self.popup_desc_scroll += 1;

                if self.popup.is_some() {
                    let tx = self.tx.clone();
                    self.carousel.add_oneshot(
                        "ScrollText",
                        Box::new(move || tx.send(Tick::Scroll.into()).map_err(|e| e.to_string())),
                        Duration::from_millis(400),
                    );
                }
            }
            Event::Tick(Tick::MacroTx) => {
                self.send_one_macro();
                if !self.macros_tx_queue.is_empty() {
                    let tx = self.tx.clone();
                    self.carousel.add_oneshot(
                        "MacroTX",
                        Box::new(move || tx.send(Tick::MacroTx.into()).map_err(|e| e.to_string())),
                        self.settings.behavior.macro_chain_delay,
                    );
                }
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
    fn send_one_macro(&mut self) {
        let serial_healthy = self.serial.port_status.load().inner.is_healthy();

        if !serial_healthy {
            self.macros_tx_queue.clear();
            return;
        }
        let Some((key_combo_opt, macro_ref)) = self.macros_tx_queue.pop_front() else {
            return;
        };

        let (macro_tag, macro_content) = self
            .macros
            .all
            .iter()
            .find(|(tag, _string)| &&macro_ref == tag)
            .expect("Failed to find referenced Macro");

        let italic = Style::new().italic();

        let (notif_line, notif_color) = match (key_combo_opt, macro_content) {
            (_, _) if macro_content.is_empty() => (
                line!["Macro \"", span!(italic; macro_tag), "\" is empty!"],
                Color::Yellow,
            ),
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
                self.buffer
                    .append_user_bytes(&content, &macro_line_ending, true);
            }
            _ => {
                self.serial
                    .send_str(&macro_content.content, &macro_line_ending, true)
                    .unwrap();
                self.buffer
                    .append_user_text(&macro_content.content, &macro_line_ending, true);

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
                self.popup_table_state.select(None);
                self.popup_single_line_state.active = true;
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
            key!(ctrl - r) if self.popup == Some(PopupMenu::Macros) => {
                self.run_method_from_string(RELOAD_MACROS).unwrap();
            }
            #[cfg(feature = "espflash")]
            key!(ctrl - r) if self.popup == Some(PopupMenu::EspFlash) => {
                self.run_method_from_string(RELOAD_ESPFLASH).unwrap();
            }
            key!(esc) => self.esc_pressed(),
            key_combo => {
                // TODO move all these into diff func
                if let Some(method) = self
                    .keybinds
                    .method_from_key_combo(key_combo)
                    .map(ToOwned::to_owned)
                {
                    if let Err(e) = self.run_method_from_string(&method) {
                        error!("Error running method `{method}`: {e}");
                    }
                    return;
                }

                let serial_healthy = self.serial.port_status.load().inner.is_healthy();

                #[cfg(feature = "espflash")]
                if let Some(profile_name) = self
                    .keybinds
                    .espflash_profile_from_key_combo(key_combo)
                    .map(ToOwned::to_owned)
                {
                    if let Some(profile) = self
                        .espflash
                        .profiles()
                        .find(|(name, _, _)| *name == profile_name)
                    {
                        let italic = Style::new().italic();
                        if serial_healthy {
                            self.notifs.notify(
                                line![
                                    "Flashing with \"",
                                    span!(italic;profile_name),
                                    "\" [",
                                    key_combo.to_string(),
                                    "]"
                                ],
                                Color::LightGreen,
                            );
                            self.serial
                                .esp_flash_profile(
                                    self.espflash.profile_from_name(&profile_name).unwrap(),
                                )
                                .unwrap();
                        } else {
                            self.notifs.notify(
                                line![
                                    "Not flashing with \"",
                                    span!(italic;profile_name),
                                    "\" [",
                                    key_combo.to_string(),
                                    "]"
                                ],
                                Color::Yellow,
                            );
                        }
                    } else {
                        error!("No such espflash profile: \"{profile_name}\"");
                        self.notifs.notify_str(
                            format!("No such espflash profile: \"{profile_name}\""),
                            Color::Yellow,
                        );
                        // let Some(profile) =
                        //     self.espflash.elfs.iter().find(|p| p.name == profile_name)
                        // else {};
                    }

                    return;
                }

                match self.macros.macro_from_key_combo(
                    key_combo,
                    &self.keybinds.macros,
                    self.settings.behavior.fuzzy_macro_match,
                ) {
                    Ok(somes) if serial_healthy => {
                        if !somes.is_empty() {
                            self.macros_tx_queue.extend(
                                somes.into_iter().map(|tag| (Some(key_combo), tag.clone())),
                            );
                            self.tx.send(Tick::MacroTx.into()).unwrap();
                        }
                    }
                    Ok(somes) => {
                        let unsent = somes
                            .into_iter()
                            .map(|tag| tag.name.clone())
                            .map(|s| Span::raw(s).italic())
                            .map(|s| tiny_vec!([Span;3] => span!("\""), s, span!("\"")))
                            // TODO fully qualified syntax
                            .intersperse(tiny_vec!([Span;3] => span!(", ")))
                            .flatten()
                            .chain(std::iter::once(span!(format!(" [{key_combo}] (Not Sent)"))));
                        self.notifs.notify(Line::from_iter(unsent), Color::Yellow);
                    }
                    Err(Some(nones)) => {
                        // let missed = nones.into_iter().map(|km| km.to_string()).join(", ");
                        // self.notifs.notify_str(format!(" {missed}"), Color::Yellow);
                        let missed_iter = nones
                            .into_iter()
                            .map(|km| km.name.clone())
                            .map(|s| Span::raw(s).italic())
                            .map(|s| tiny_vec!([Span;3] => span!("\""), s, span!("\"")))
                            // TODO fully qualified syntax
                            .intersperse(tiny_vec!([Span;3] => span!(", ")))
                            .flatten();
                        let missed = std::iter::once(span!(format!("Macro search failed for: ")))
                            .chain(missed_iter);

                        self.notifs.notify(Line::from_iter(missed), Color::Yellow);
                    }
                    // No macros found.
                    Err(None) => (),
                }
            }
        }
    }
    fn run_method_from_string(&mut self, method: &str) -> Result<()> {
        let m = method;
        let pretty_bool = |b: bool| {
            if b { "On" } else { "Off" }
        };
        match m {
            _ if m == TOGGLE_TEXTWRAP => {
                let state = pretty_bool(self.settings.rendering.wrap_text.flip());
                self.buffer
                    .update_render_settings(self.settings.rendering.clone());
                self.settings.save().unwrap();
                self.notifs
                    .notify_str(format!("Toggled Text Wrapping {state}"), Color::Gray);
            }
            _ if m == TOGGLE_DTR => {
                self.serial.toggle_signals(true, false).unwrap();
            }
            _ if m == TOGGLE_RTS => {
                self.serial.toggle_signals(false, true).unwrap();
            }
            // key!(ctrl - e) => {
            // "esp-bootloader" => {
            // self.serial.write_signals(Some(false), Some(false));
            // self.serial.write_signals(Some(true), Some(true));
            // self.serial.write_signals(Some(false), Some(true));
            // std::thread::sleep(Duration::from_millis(100));
            // self.serial.write_signals(Some(true), Some(false));
            // std::thread::sleep(Duration::from_millis(100));
            // self.serial.write_signals(Some(false), Some(false));

            // self.buffer
            //     .append_user_text("Attempting to put Espressif device into bootloader...");
            // self.serial.esp_restart(None);
            // }
            _ if m == TOGGLE_TIMESTAMPS => {
                let state = pretty_bool(self.settings.rendering.timestamps.flip());
                self.buffer
                    .update_render_settings(self.settings.rendering.clone());
                self.settings.save().unwrap();
                self.notifs
                    .notify_str(format!("Toggled Timestamps {state}"), Color::Gray);
            }

            _ if m == TOGGLE_INDICES => {
                let state = pretty_bool(self.settings.rendering.show_indices.flip());
                self.buffer
                    .update_render_settings(self.settings.rendering.clone());
                self.notifs.notify_str(
                    format!("Toggled Line Indices + Length {state}"),
                    Color::Gray,
                );
            }

            _ if m == TOGGLE_HEX => {
                let state = pretty_bool(self.settings.rendering.hex_view.flip());
                self.buffer
                    .update_render_settings(self.settings.rendering.clone());
                self.buffer.scroll_by(0);
                self.notifs
                    .notify_str(format!("Toggled Hex View {state}"), Color::Gray);
            }

            _ if m == TOGGLE_HEX_HEADER => {
                let state = pretty_bool(self.settings.rendering.hex_view_header.flip());
                self.buffer
                    .update_render_settings(self.settings.rendering.clone());
                self.notifs
                    .notify_str(format!("Toggled Hex View Header {state}"), Color::Gray);
            }

            _ if m == SHOW_MACROS => {
                self.popup = Some(PopupMenu::Macros);
                if self.macros.is_empty() {
                    self.popup_table_state.select(None);
                    self.popup_single_line_state.active = true;
                } else {
                    self.popup_table_state.select(Some(0));
                    self.popup_single_line_state.active = false;
                }
                self.tx
                    .send(Tick::Scroll.into())
                    .map_err(|e| e.to_string())
                    .unwrap();
            }

            _ if m == SHOW_BEHAVIOR => {
                self.popup = Some(PopupMenu::BehaviorSettings);
                self.popup_single_line_state.active = false;
                self.popup_table_state.select(Some(0));

                self.tx
                    .send(Tick::Scroll.into())
                    .map_err(|e| e.to_string())
                    .unwrap();
            }

            _ if m == SHOW_RENDERING => {
                self.popup = Some(PopupMenu::RenderingSettings);
                self.popup_single_line_state.active = false;
                self.popup_table_state.select(Some(0));

                self.tx
                    .send(Tick::Scroll.into())
                    .map_err(|e| e.to_string())
                    .unwrap();
            }

            _ if m == SHOW_PORTSETTINGS => {
                self.popup = Some(PopupMenu::PortSettings);
                self.refresh_scratch();
                self.popup_desc_scroll = -2;
                self.popup_table_state.select(Some(0));
                self.popup_single_line_state.active = false;

                self.tx
                    .send(Tick::Scroll.into())
                    .map_err(|e| e.to_string())
                    .unwrap();
            }

            _ if m == RELOAD_MACROS => {
                self.macros
                    .load_from_folder("../../example_macros")
                    .unwrap();
                self.notifs
                    .notify_str(format!("Reloaded Macros!"), Color::Green);
            }

            _ if m == RELOAD_COLORS => {
                self.buffer.reload_color_rules().unwrap();
                self.notifs
                    .notify_str(format!("Reloaded Color Rules!"), Color::Green);
            }

            #[cfg(feature = "logging")]
            _ if m == SHOW_LOGGING => {
                self.popup = Some(PopupMenu::Logging);
                self.refresh_scratch();
                self.popup_desc_scroll = -2;
                self.popup_table_state.select(Some(0));
                self.popup_single_line_state.active = false;

                self.tx
                    .send(Tick::Scroll.into())
                    .map_err(|e| e.to_string())
                    .unwrap();
            }

            #[cfg(feature = "logging")]
            _ if m == LOGGING_START => {
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
            _ if m == LOGGING_STOP => {
                if self.buffer.log_handle.logging_active() {
                    self.buffer.log_handle.request_log_stop().unwrap();
                } else {
                    self.notifs
                        .notify_str("No logging session active to stop!", Color::Yellow);
                }
            }

            #[cfg(feature = "logging")]
            _ if m == LOGGING_TOGGLE => {
                if self.buffer.log_handle.logging_active() {
                    self.run_method_from_string(LOGGING_STOP)?;
                } else {
                    self.run_method_from_string(LOGGING_START)?;
                }
            }

            #[cfg(feature = "espflash")]
            _ if m == SHOW_ESPFLASH => {
                self.popup = Some(PopupMenu::EspFlash);
                self.refresh_scratch();
                self.popup_desc_scroll = -2;
                self.popup_table_state.select(Some(0));
                self.popup_single_line_state.active = false;

                self.tx
                    .send(Tick::Scroll.into())
                    .map_err(|e| e.to_string())
                    .unwrap();
            }

            #[cfg(feature = "espflash")]
            _ if m == ESP_HARD_RESET => {
                self.serial.esp_restart(EspRestartType::UserCode).unwrap();
            }

            #[cfg(feature = "espflash")]
            _ if m == ESP_BOOTLOADER => {
                self.serial
                    .esp_restart(EspRestartType::Bootloader { active: true })
                    .unwrap();
            }

            #[cfg(feature = "espflash")]
            _ if m == ESP_BOOTLOADER_UNCHECKED => {
                self.serial
                    .esp_restart(EspRestartType::Bootloader { active: false })
                    .unwrap();
            }

            #[cfg(feature = "espflash")]
            _ if m == ESP_DEVICE_INFO => {
                self.serial.esp_device_info().unwrap();
            }

            #[cfg(feature = "espflash")]
            _ if m == ESP_ERASE_FLASH => {
                self.serial.esp_erase_flash().unwrap();
            }
            #[cfg(feature = "espflash")]
            _ if m == RELOAD_ESPFLASH => {
                self.notifs
                    .notify_str("Reloaded espflash profiles!", Color::Green);
                self.espflash.reload().unwrap();
            }

            unknown => {
                warn!("Unknown keybind action: {unknown}");
                self.notifs.notify_str(
                    format!("Unknown keybind action: \"{unknown}\""),
                    Color::Yellow,
                );
            }
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
                self.menu = TerminalPrompt::DisconnectPrompt.into();
            }
            Menu::Terminal(TerminalPrompt::DisconnectPrompt) => {
                self.menu = TerminalPrompt::None.into();
            }
            Menu::PortSelection(_) => self.shutdown(),
        }
    }
    fn up_pressed(&mut self) {
        self.user_input.all_text_selected = false;
        self.popup_desc_scroll = -2;
        match &self.popup {
            None => (),
            Some(popup) if self.popup_single_line_state.active => {
                match popup {
                    PopupMenu::Macros if self.macros.none_visible() => return,
                    #[cfg(feature = "espflash")]
                    PopupMenu::EspFlash if self.espflash.is_empty() => {
                        self.popup_table_state.select_last();
                    }
                    #[cfg(feature = "espflash")]
                    PopupMenu::EspFlash => {
                        self.popup_table_state.select_last();
                        self.espflash.profiles_selected = true;
                    }
                    _ => (),
                }
                self.popup_single_line_state.active = false;
                self.popup_table_state.select_last();
            }
            Some(PopupMenu::PortSettings) => {
                match self
                    .scratch
                    .last_port_settings
                    .handle_input(ArrowKey::Up, &mut self.popup_table_state)
                    .unwrap()
                {
                    (_, true, _) | (_, _, true) => {
                        self.popup_table_state.select(None);
                        self.popup_single_line_state.active = true;
                    }
                    (_, _, _) => (),
                }
            }
            Some(PopupMenu::RenderingSettings) => {
                match self
                    .scratch
                    .rendering
                    .handle_input(ArrowKey::Up, &mut self.popup_table_state)
                    .unwrap()
                {
                    (_, true, _) | (_, _, true) => {
                        self.popup_table_state.select(None);
                        self.popup_single_line_state.active = true;
                    }
                    (_, _, _) => (),
                }
            }
            Some(PopupMenu::BehaviorSettings) => {
                match self
                    .scratch
                    .behavior
                    .handle_input(ArrowKey::Up, &mut self.popup_table_state)
                    .unwrap()
                {
                    (_, true, _) | (_, _, true) => {
                        self.popup_table_state.select(None);
                        self.popup_single_line_state.active = true;
                    }
                    (_, _, _) => (),
                }
            }
            Some(PopupMenu::Macros) if self.macros.categories_selector.active => {
                self.popup_single_line_state.active = true;
                self.macros.categories_selector.active = false;
            }
            Some(PopupMenu::Macros) => {
                if self.popup_table_state.selected() == Some(0) {
                    self.popup_table_state.select(None);
                    if self.macros.search_input.value().is_empty() {
                        self.macros.categories_selector.active = true;
                    } else {
                        self.popup_single_line_state.active = true;
                    }
                } else {
                    self.scroll_menu_up();
                }
            }
            #[cfg(feature = "espflash")]
            Some(PopupMenu::EspFlash) if self.espflash.profiles_selected => {
                if self.popup_table_state.selected() == Some(0) {
                    self.popup_table_state.select_last();
                    self.espflash.profiles_selected = false;
                } else {
                    self.scroll_menu_up();
                }
            }
            #[cfg(feature = "espflash")]
            Some(PopupMenu::EspFlash) => {
                if self.popup_table_state.selected() == Some(0) {
                    self.popup_table_state.select(None);
                    self.popup_single_line_state.active = true;
                } else {
                    self.scroll_menu_up();
                }
            }
            #[cfg(feature = "logging")]
            Some(PopupMenu::Logging) => {
                ();
            }
        }
        if self.popup.is_some() {
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
        }
        self.post_menu_vert_scroll(true);
    }
    fn down_pressed(&mut self) {
        self.user_input.all_text_selected = false;
        self.popup_desc_scroll = -2;
        match &self.popup {
            None => (),
            // If categories selected, try to select first visible macro.
            Some(popup) if self.macros.categories_selector.active => {
                match popup {
                    PopupMenu::Macros if self.macros.none_visible() => return,
                    _ => (),
                }
                self.macros.categories_selector.active = false;
                self.popup_table_state.select_first();
            }
            // If popup selector chosen on Macro screen, select the Macro Categories (if there's no search active)
            // Has to be above the catch-all below
            Some(PopupMenu::Macros) if self.popup_single_line_state.active => {
                self.popup_single_line_state.active = false;

                if self.macros.search_input.value().is_empty() {
                    self.macros.categories_selector.active = true;
                } else {
                    self.popup_table_state.select_first();
                }
            }
            // If on any other screen, just select the first element.
            Some(popup) if self.popup_single_line_state.active => {
                self.popup_single_line_state.active = false;
                self.popup_table_state.select_first();
            }
            Some(PopupMenu::PortSettings) => {
                match self
                    .scratch
                    .last_port_settings
                    .handle_input(ArrowKey::Down, &mut self.popup_table_state)
                    .unwrap()
                {
                    (_, true, _) | (_, _, true) => {
                        self.popup_table_state.select(None);
                        self.popup_single_line_state.active = true;
                    }
                    (_, _, _) => (),
                }
            }
            Some(PopupMenu::BehaviorSettings) => {
                match self
                    .scratch
                    .behavior
                    .handle_input(ArrowKey::Down, &mut self.popup_table_state)
                    .unwrap()
                {
                    (_, true, _) | (_, _, true) => {
                        self.popup_table_state.select(None);
                        self.popup_single_line_state.active = true;
                    }
                    (_, _, _) => (),
                }
            }
            Some(PopupMenu::RenderingSettings) => {
                match self
                    .scratch
                    .rendering
                    .handle_input(ArrowKey::Down, &mut self.popup_table_state)
                    .unwrap()
                {
                    (_, true, _) | (_, _, true) => {
                        self.popup_table_state.select(None);
                        self.popup_single_line_state.active = true;
                    }
                    (_, _, _) => (),
                }
            }

            Some(PopupMenu::Macros) => {
                if self.popup_table_state.selected() >= self.macros.last_index_checked() {
                    self.popup_table_state.select(None);
                    self.popup_single_line_state.active = true;
                } else {
                    self.scroll_menu_down();
                }
            }
            #[cfg(feature = "espflash")]
            Some(PopupMenu::EspFlash) if self.espflash.profiles_selected => {
                if self.popup_table_state.selected() >= Some(self.espflash.last_index()) {
                    self.popup_table_state.select(None);
                    self.popup_single_line_state.active = true;
                    self.espflash.profiles_selected = false;
                } else {
                    self.scroll_menu_down();
                }
            }
            #[cfg(feature = "espflash")]
            Some(PopupMenu::EspFlash) => {
                if self.popup_table_state.selected() >= Some(esp::ESPFLASH_BUTTON_COUNT - 1) {
                    if self.espflash.is_empty() {
                        self.popup_table_state.select(None);
                        self.popup_single_line_state.active = true;
                    } else {
                        self.popup_table_state.select_first();
                        self.espflash.profiles_selected = true;
                    }
                } else {
                    self.scroll_menu_down();
                }
            }
            #[cfg(feature = "logging")]
            Some(PopupMenu::Logging) => {
                ();
            }
        }
        if self.popup.is_some() {
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
        }
        self.post_menu_vert_scroll(false);
    }
    fn left_pressed(&mut self) {
        match &mut self.popup {
            None => (),
            Some(_popup) if self.popup_single_line_state.active => {
                self.scroll_popup(false);
            }
            Some(PopupMenu::PortSettings) => {
                self.scratch
                    .last_port_settings
                    .handle_input(ArrowKey::Left, &mut self.popup_table_state)
                    .unwrap();
            }
            Some(PopupMenu::BehaviorSettings) => {
                self.scratch
                    .behavior
                    .handle_input(ArrowKey::Left, &mut self.popup_table_state)
                    .unwrap();
            }
            Some(PopupMenu::RenderingSettings) => {
                self.scratch
                    .rendering
                    .handle_input(ArrowKey::Left, &mut self.popup_table_state)
                    .unwrap();
            }
            Some(PopupMenu::Macros) => {
                if !self.macros.search_input.value().is_empty() {
                    return;
                }
                self.macros.categories_selector.prev();
                if self.popup_table_state.selected().is_some() && !self.macros.none_visible() {
                    self.popup_table_state.select_first();
                }
            }
            #[cfg(feature = "espflash")]
            Some(PopupMenu::EspFlash) => (),
            #[cfg(feature = "logging")]
            Some(PopupMenu::Logging) => (),
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
            Some(popup) if self.popup_single_line_state.active => {
                self.scroll_popup(true);
            }
            Some(PopupMenu::PortSettings) => {
                self.scratch
                    .last_port_settings
                    .handle_input(ArrowKey::Right, &mut self.popup_table_state)
                    .unwrap();
            }
            Some(PopupMenu::BehaviorSettings) => {
                self.scratch
                    .behavior
                    .handle_input(ArrowKey::Right, &mut self.popup_table_state)
                    .unwrap();
            }
            Some(PopupMenu::RenderingSettings) => {
                self.scratch
                    .rendering
                    .handle_input(ArrowKey::Right, &mut self.popup_table_state)
                    .unwrap();
            }
            Some(PopupMenu::Macros) => {
                if !self.macros.search_input.value().is_empty() {
                    return;
                }
                self.macros.categories_selector.next();
                if self.popup_table_state.selected().is_some() && !self.macros.none_visible() {
                    self.popup_table_state.select_first();
                }
            }
            #[cfg(feature = "espflash")]
            Some(PopupMenu::EspFlash) => (),
            #[cfg(feature = "logging")]
            Some(PopupMenu::Logging) => (),
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
                return;
            }
            Some(PopupMenu::BehaviorSettings) => {
                self.settings.behavior = self.scratch.behavior.clone();

                self.settings.save().unwrap();
                self.dismiss_popup();
                self.notifs
                    .notify_str("Behavior settings saved!", Color::Green);
                return;
            }
            Some(PopupMenu::RenderingSettings) => {
                self.settings.rendering = self.scratch.rendering.clone();
                self.buffer
                    .update_render_settings(self.settings.rendering.clone());

                self.settings.save().unwrap();
                self.dismiss_popup();
                self.notifs
                    .notify_str("Rendering settings saved!", Color::Green);
                return;
            }
            Some(PopupMenu::Macros) => {
                if self.popup_single_line_state.active || self.macros.categories_selector.active {
                    return;
                }
                let Some(index) = self.popup_table_state.selected() else {
                    unreachable!();
                };
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
                            return;
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
                            self.macros_tx_queue.push_back((None, tag));
                            self.tx.send(Tick::MacroTx.into()).unwrap();
                        }
                    };
                }
            }
            #[cfg(feature = "espflash")]
            Some(PopupMenu::EspFlash) => {
                let Some(selected) = self.popup_table_state.selected() else {
                    return;
                };
                if !serial_healthy {
                    self.notifs.notify_str("Port isn't ready!", Color::Red);
                    return;
                }
                if self.espflash.profiles_selected {
                    assert!(
                        !self.espflash.is_empty(),
                        "shouldn't have selected a non-existant flash profile"
                    );

                    self.serial
                        .esp_flash_profile(self.espflash.profile_from_index(selected).unwrap())
                        .unwrap();
                } else {
                    match selected {
                        0 => self.run_method_from_string(ESP_HARD_RESET).unwrap(),
                        1 if ctrl_pressed || shift_pressed => self
                            .run_method_from_string(ESP_BOOTLOADER_UNCHECKED)
                            .unwrap(),
                        1 => self.run_method_from_string(ESP_BOOTLOADER).unwrap(),
                        2 => self.run_method_from_string(ESP_DEVICE_INFO).unwrap(),
                        3 => {
                            if !shift_pressed && !ctrl_pressed {
                                self.notifs.notify_str(
                                    "Press Shift/Ctrl+Enter to erase flash!",
                                    Color::Yellow,
                                );
                            } else {
                                self.run_method_from_string(ESP_ERASE_FLASH).unwrap();
                            }
                        }
                        unknown => unreachable!("unknown espflash command index {unknown}"),
                    }
                }
            }
            #[cfg(feature = "logging")]
            Some(PopupMenu::Logging) => {
                ();
            }
        }
        if self.popup.is_some() {
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
                self.refresh_scratch();
                self.popup = Some(PopupMenu::PortSettings);
                self.table_state.select(None);
                self.popup_single_line_state.active = true;

                self.tx
                    .send(Tick::Scroll.into())
                    .map_err(|e| e.to_string())
                    .unwrap();
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
                            .append_user_text(user_input, user_le_bytes, false);
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
                    DisconnectPrompt::Cancel => self.menu = Menu::Terminal(TerminalPrompt::None),
                    DisconnectPrompt::Exit => self.shutdown(),
                    DisconnectPrompt::PortSettings => {
                        // TODO i hate this, consolidate this.
                        self.menu = Menu::Terminal(TerminalPrompt::None);
                        self.dismiss_popup();
                        self.popup = Some(PopupMenu::PortSettings);
                        self.popup_table_state.select_first();
                        self.tx.send(Tick::Scroll.into()).unwrap();
                    }
                    DisconnectPrompt::Disconnect => {
                        self.serial.disconnect().unwrap();
                        // Refresh port listings
                        self.ports.clear();
                        self.serial.request_port_scan().unwrap();

                        self.buffer.intentional_disconnect();
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
        if self.popup.is_some() {
            self.popup_table_state.scroll_up_by(1);
            return;
        }
        self.table_state.scroll_up_by(1);
    }
    fn scroll_menu_down(&mut self) {
        if self.popup.is_some() {
            self.popup_table_state.scroll_down_by(1);
            return;
        }
        self.table_state.scroll_down_by(1);
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
            PopupMenu::Macros => Color::Green,
            PopupMenu::RenderingSettings => Color::Red,
            PopupMenu::BehaviorSettings => Color::Blue,
            PopupMenu::PortSettings => Color::Cyan,
            #[cfg(feature = "espflash")]
            PopupMenu::EspFlash => Color::Magenta,
            #[cfg(feature = "logging")]
            PopupMenu::Logging => Color::Yellow,
        };

        let macros_visible_amt = self.macros.visible_len();

        match (
            popup,
            self.macros.categories_selector.active,
            self.popup_single_line_state.active,
            self.popup_table_state.selected().is_some(),
        ) {
            // Macros-specific logic
            (PopupMenu::Macros, _, true, true) | (PopupMenu::Macros, true, _, true) => {
                if macros_visible_amt == 0 {
                    self.popup_single_line_state.active = false;
                    self.macros.categories_selector.active = true;

                    self.popup_table_state.select(None);
                } else {
                    self.popup_single_line_state.active = false;
                    self.macros.categories_selector.active = false;
                }
            }
            (PopupMenu::Macros, false, false, false) if macros_visible_amt == 0 => {
                self.popup_single_line_state.active = false;
                self.macros.categories_selector.active = true;

                self.popup_table_state.select(None);
            }

            // Generic logic
            (_, _, true, true) => {
                self.popup_single_line_state.active = true;
                self.macros.categories_selector.active = false;

                self.popup_table_state.select(None);
            }
            _ => (),
        }
        // Above match should ensure these pass without issue.
        assert!(
            (self.popup_single_line_state.active || self.macros.categories_selector.active)
                != self.popup_table_state.selected().is_some(),
            "Either a table element needs to be selected, or the menu title widget, but never both or neither."
        );
        assert_eq!(
            self.popup_single_line_state.active && self.macros.categories_selector.active,
            false,
            "Both selectors can't be active."
        );
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
        self.popup_single_line_state.select(
            <PopupMenu as VariantArray>::VARIANTS
                .iter()
                .position(|v| v == popup)
                .unwrap(),
        );
        frame.render_stateful_widget(
            &popup_menu_title_selector,
            title,
            &mut self.popup_single_line_state,
        );

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

        let content_length = match popup {
            PopupMenu::Macros => self.macros.visible_len(),
            // TODO find more clear way than checking this length
            PopupMenu::PortSettings => PortSettings::VISIBLE_FIELDS,
            PopupMenu::BehaviorSettings => Behavior::VISIBLE_FIELDS,
            PopupMenu::RenderingSettings => Rendering::VISIBLE_FIELDS,
            #[cfg(feature = "espflash")]
            // TODO proper scrollbar for espflash profiles
            PopupMenu::EspFlash => 4,
            #[cfg(feature = "logging")]
            PopupMenu::Logging => Logging::VISIBLE_FIELDS,
        };

        let height = match popup {
            PopupMenu::Macros => macros_table_area.height,
            _ => settings_area.height,
        };

        self.popup_scrollbar_state = self
            .popup_scrollbar_state
            .content_length(content_length.saturating_sub(height as usize));

        self.popup_scrollbar_state = self
            .popup_scrollbar_state
            .position(self.popup_table_state.offset());
        frame.render_stateful_widget(
            scrollbar,
            center_inner.offset(Offset { x: 1, y: 0 }).inner(Margin {
                horizontal: 0,
                vertical: 1,
            }),
            &mut self.popup_scrollbar_state,
        );

        match popup {
            PopupMenu::PortSettings => {
                frame.render_stateful_widget(
                    self.scratch
                        .last_port_settings
                        .as_table(&mut self.popup_table_state),
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
                    &mut self.popup_desc_scroll,
                );
                frame.render_widget(
                    Line::raw("Esc: Cancel | Enter: Save")
                        .all_spans_styled(Color::DarkGray.into())
                        .centered(),
                    hint_text_area,
                );
            }
            PopupMenu::BehaviorSettings => {
                frame.render_stateful_widget(
                    self.scratch.behavior.as_table(&mut self.popup_table_state),
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
                    &mut self.popup_desc_scroll,
                );
                frame.render_widget(
                    Line::raw("Esc: Cancel | Enter: Save")
                        .all_spans_styled(Color::DarkGray.into())
                        .centered(),
                    hint_text_area,
                );
            }
            PopupMenu::RenderingSettings => {
                frame.render_stateful_widget(
                    self.scratch.rendering.as_table(&mut self.popup_table_state),
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
                    &mut self.popup_desc_scroll,
                );
                frame.render_widget(
                    Line::raw("Esc: Cancel | Enter: Save")
                        .all_spans_styled(Color::DarkGray.into())
                        .centered(),
                    hint_text_area,
                );
            }
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

                    let (tag, string) = self.macros.filtered_macro_iter().nth(index).unwrap();
                    // for now i guess
                    // TOOD replace with fancy line preview
                    let macro_preview = string.as_str();
                    let line = macro_preview.to_line().italic();
                    // let line = if matches!(macro_binding.content, MacroContent::Bytes { .. }) {
                    //     line.light_blue()
                    // } else {
                    //     line
                    // };
                    render_scrolling_line(
                        line,
                        frame,
                        scrolling_text_area,
                        &mut self.popup_desc_scroll,
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
                    area.height = area.height.saturating_sub(7);
                    area
                };
                let line_block = Block::new()
                    .borders(Borders::TOP)
                    .border_style(Style::from(popup_color));
                frame.render_widget(
                    Line::raw(r"G:\git\yap\target\debug\logs")
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

                let toggle_button = toggle_logging_button(
                    &mut self.popup_table_state,
                    self.buffer.log_handle.logging_active(),
                );

                if self.logging_toggle_selected {
                    frame.render_stateful_widget(
                        toggle_button,
                        settings_area,
                        &mut self.popup_table_state,
                    );
                    frame.render_widget(&line_block, new_seperator);
                    frame.render_widget(
                        self.settings.logging.as_table(&mut self.popup_table_state),
                        bins_area,
                    );
                } else {
                    frame.render_widget(toggle_button, settings_area);
                    frame.render_widget(&line_block, new_seperator);
                    frame.render_stateful_widget(
                        self.settings.logging.as_table(&mut self.popup_table_state),
                        bins_area,
                        &mut self.popup_table_state,
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

                if self.espflash.profiles_selected {
                    frame.render_widget(
                        esp::espflash_buttons(&mut self.popup_table_state),
                        settings_area,
                    );
                    frame.render_widget(&line_block, new_seperator);
                    frame.render_stateful_widget(
                        self.espflash.profiles_table(&mut self.popup_table_state),
                        bins_area,
                        &mut self.popup_table_state,
                    );

                    if let Some(profile) = self
                        .popup_table_state
                        .selected()
                        .and_then(|idx| self.espflash.profile_from_index(idx))
                    {
                        let hint_text = match profile {
                            esp::EspProfile::Bins(_) => "Flash profile binaries to ESP Flash.",
                            esp::EspProfile::Elf(profile) if profile.ram => {
                                "Load profile ELF into RAM."
                            }
                            esp::EspProfile::Elf(_) => "Flash profile ELF to ESP Flash.",
                        };
                        render_scrolling_line(
                            hint_text,
                            frame,
                            scrolling_text_area,
                            &mut self.popup_desc_scroll,
                        );
                    }
                } else {
                    frame.render_stateful_widget(
                        esp::espflash_buttons(&mut self.popup_table_state),
                        settings_area,
                        &mut self.popup_table_state,
                    );
                    frame.render_widget(&line_block, new_seperator);
                    frame.render_widget(
                        self.espflash.profiles_table(&mut self.popup_table_state),
                        bins_area,
                    );

                    let hints = [
                        "Attempt to remotely reset the chip.",
                        "Attempt to reboot into bootloader. Shift/Ctrl to skip check.",
                        "Query ESP for Flash Size, MAC Address, etc.",
                        "Erase all flash contents.",
                    ];
                    if let Some(idx) = self.popup_table_state.selected() {
                        if let Some(&hint_text) = hints.get(idx) {
                            render_scrolling_line(
                                hint_text,
                                frame,
                                scrolling_text_area,
                                &mut self.popup_desc_scroll,
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
        let disconnect_prompt_shown = prompt == TerminalPrompt::DisconnectPrompt;
        let [terminal_area, line_area, input_area] = vertical![*=1, ==1, ==1].areas(area);
        let [input_symbol_area, input_area] = horizontal![==1, *=1].areas(input_area);

        // let text = Text::from(buffer);
        // let buffer = self.buffer.lines();
        // let lines: Vec<_> = buffer.collect();

        // let text = Paragraph::new(buffer.to_owned());

        // let vert_scroll = self.buffer_scroll as u16;
        // let para = self.buffer.terminal_paragraph(self.buffer_wrapping);
        // let para = para.scroll((vert_scroll, 0));
        // frame.render_widget(para, terminal_area);
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

        {
            let port_name_line = Line::raw(port_text).centered();
            frame.render_widget(port_name_line, line_area);

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

        let should_position_cursor = !disconnect_prompt_shown && self.popup.is_none();

        if self.user_input.input_box.value().is_empty() {
            // Leading space leaves room for full-width cursors.
            // TODO make binding hint dynamic (should maybe cache?)
            let port_settings_combo = self
                .keybinds
                .keybindings
                .iter()
                .find(|(kc, m)| m.as_str() == SHOW_PORTSETTINGS)
                .map(|(kc, m)| kc.to_compact_string())
                .unwrap_or_else(|| CompactString::const_new("UNBOUND"));
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

        if disconnect_prompt_shown {
            // let area = centered_rect(30, 30, area);
            // let save_device_prompt =
            //     DisconnectPrompt::prompt_table_block("Disconnect from port?", Style::new().blue());
            DisconnectPrompt::render_prompt_block_popup(
                Some("Disconnect from port?"),
                None,
                Style::new().blue(),
                frame,
                area,
                &mut self.table_state,
            );
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
        // self.scratch = ScratchSpace {
        //     behavior: self.settings.behavior.clone(),
        //     port: self.serial.port_settings.load().as_ref().clone(),
        // }
        self.scratch = self.settings.clone();
    }
    fn dismiss_popup(&mut self) {
        self.refresh_scratch();
        self.popup.take();
        self.macros.categories_selector.active = false;
        // self.macros.ui_state = MacrosPrompt::None;
        self.popup_single_line_state.active = false;
        self.popup_table_state.select(None);
    }
    fn scroll_popup(&mut self, next: bool) {
        let Some(popup) = &mut self.popup else {
            return;
        };

        let mut new_popup = if next { popup.next() } else { popup.prev() };
        match &popup {
            PopupMenu::Macros => self.macros.categories_selector.active = false,
            _ => (),
        };
        // match &new_popup {

        // };

        std::mem::swap(popup, &mut new_popup);

        self.refresh_scratch();
        self.popup_desc_scroll = -2;
        self.macros.search_input.reset();
        #[cfg(feature = "espflash")]
        {
            self.espflash.profiles_selected = false;
        }
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
