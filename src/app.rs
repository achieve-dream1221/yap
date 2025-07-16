#[cfg(feature = "defmt")]
use std::sync::Arc;
use std::{
    borrow::Cow,
    collections::VecDeque,
    thread::JoinHandle,
    time::{Duration, Instant},
};

use bstr::ByteVec;
#[cfg(feature = "defmt")]
use camino::Utf8Path;
use camino::Utf8PathBuf;
use color_eyre::eyre::Result;
use compact_str::{CompactString, ToCompactString};
use crokey::{KeyCombination, key};
use crossbeam::channel::{Receiver, Select, Sender, TrySendError};
use enum_rotate::EnumRotate;
use fs_err as fs;

use ratatui::{
    Frame, Terminal,
    crossterm::event::{KeyCode, KeyEvent, KeyModifiers},
    layout::{Constraint, Layout, Margin, Offset, Rect, Size},
    prelude::Backend,
    style::{Color, Modifier, Style, Stylize},
    text::{Line, Span, ToLine},
    widgets::{
        Block, Borders, Clear, HighlightSpacing, Paragraph, Row, Scrollbar, ScrollbarOrientation,
        ScrollbarState, Table, TableState,
    },
};
#[cfg(feature = "defmt")]
use ratatui_explorer::FileExplorer;
use ratatui_macros::{horizontal, line, span, vertical};
use serialport::{SerialPortInfo, SerialPortType};
use struct_table::{ArrowKey, StructTable};
use strum::{VariantArray, VariantNames};
use takeable::Takeable;

use tracing::{debug, error, info, trace};
use tui_big_text::{BigText, PixelSize};
use tui_input::{Input, StateChanged, backend::crossterm::EventHandler};

#[cfg(feature = "defmt")]
#[cfg(feature = "logging")]
use crate::buffer::LoggingHandle;
#[cfg(feature = "defmt")]
#[cfg(feature = "defmt_watch")]
use crate::buffer::defmt::elf_watcher::ElfWatchHandle;
use crate::{
    buffer::Buffer,
    event_carousel::CarouselHandle,
    history::UserInput,
    is_ctrl_c,
    keybinds::{Action, AppAction, BaseAction, Keybinds, PortAction, ShowPopupAction},
    notifications::{EMERGE_TIME, EXPAND_TIME, EXPIRE_TIME, Notifications, PAUSE_TIME},
    serial::{
        PrintablePortInfo, ReconnectType, Reconnections, SerialDisconnectReason, SerialEvent,
        handle::SerialHandle,
        worker::{InnerPortStatus, MOCK_PORT_NAME},
    },
    settings::{Behavior, PortSettings, Rendering, Settings},
    traits::{FirstChars, LastIndex, LineHelpers, RequiresPort, ToggleBool},
    tui::{
        centered_rect_size,
        defmt::DefmtRecentError,
        prompts::{AttemptReconnectPrompt, DisconnectPrompt, PromptKeybind, PromptTable},
        show_keybinds,
        single_line_selector::{SingleLineSelector, SingleLineSelectorState, StateBottomed},
    },
};

#[cfg(feature = "defmt")]
use crate::{
    buffer::defmt::{DefmtDecoder, DefmtLoadError, LocationsError},
    tui::defmt::DefmtRecentElfs,
};
#[cfg(feature = "defmt")]
use crate::{keybinds::DefmtSelectAction, settings::Defmt, tui::defmt::DefmtHelpers};

#[cfg(feature = "defmt_watch")]
use crate::buffer::defmt::elf_watcher::ElfWatchEvent;

#[cfg(feature = "macros")]
use crate::{
    config_adjacent_path,
    macros::{MacroNameTag, MacroNotFound, Macros},
    tui::ALWAYS_PRESENT_SELECTOR_COUNT,
};

#[cfg(feature = "macros")]
use crate::keybinds::MacroBuiltinAction;

#[cfg(feature = "logging")]
use crate::settings::Logging;
#[cfg(feature = "logging")]
use crate::tui::logging::sync_logs_button;
#[cfg(feature = "logging")]
use crate::{buffer::LoggingEvent, keybinds::LoggingAction};

#[cfg(feature = "espflash")]
use crate::serial::esp::{EspEvent, EspRestartType};

#[cfg(feature = "espflash")]
use crate::tui::esp::{self, EspFlashHelper};
#[cfg(feature = "espflash")]
use crate::{errors::HandleResult, keybinds::EspAction};

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
    RxBuffer(Vec<u8>),
    Tick(Tick),
    #[cfg(feature = "logging")]
    Logging(LoggingEvent),
    #[cfg(feature = "defmt_watch")]
    DefmtElfWatch(ElfWatchEvent),
    #[cfg(feature = "defmt")]
    DefmtFromFilePicker(Utf8PathBuf),
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

#[derive(Debug, Clone, PartialEq, Eq, EnumRotate, VariantArray, VariantNames)]
#[repr(u8)]
#[strum(serialize_all = "title_case")]
pub enum SettingsMenu {
    SerialPort,
    Rendering,
    Behavior,
    #[cfg(feature = "logging")]
    Logging,
    #[cfg(feature = "defmt")]
    #[strum(serialize = "defmt")]
    Defmt,
}

#[cfg(any(feature = "espflash", feature = "macros"))]
#[derive(Debug, Clone, PartialEq, Eq, EnumRotate, VariantArray, VariantNames)]
#[repr(u8)]
#[strum(serialize_all = "title_case")]
pub enum ToolMenu {
    #[cfg(feature = "macros")]
    Macros,
    #[cfg(feature = "espflash")]
    #[strum(serialize = "ESP32 Flashing")]
    EspFlash,
}

#[allow(clippy::large_enum_variant)]
#[derive(Debug, PartialEq, Eq)]
enum Popup {
    SettingsMenu(SettingsMenu),
    #[cfg(any(feature = "espflash", feature = "macros"))]
    ToolMenu(ToolMenu),
    CurrentKeybinds,
    ErrorMessage(String),
    #[cfg(feature = "defmt")]
    DefmtNewElf(FileExplorer),
    #[cfg(feature = "defmt")]
    DefmtRecentElf,
}

#[cfg(any(feature = "espflash", feature = "macros"))]
impl From<ToolMenu> for Popup {
    fn from(value: ToolMenu) -> Self {
        Self::ToolMenu(value)
    }
}
impl From<SettingsMenu> for Popup {
    fn from(value: SettingsMenu) -> Self {
        Self::SettingsMenu(value)
    }
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

#[derive(Debug, thiserror::Error)]
enum NoSenders {
    #[error("Serial Buffer sender has hung up unexpectedly!")]
    SerialRx,
    #[error("All event senders have hung up unexpectedly!")]
    Events,
}

// Maybe have the buffer in a TUI struct?

pub struct App {
    state: RunningState,
    menu: Menu,

    event_tx: Sender<Event>,
    event_rx: Receiver<Event>,
    serial_buf_rx: Receiver<Vec<u8>>,

    table_state: TableState,
    baud_selection_state: SingleLineSelectorState,
    popup: Option<Popup>,
    popup_menu_scroll: usize,
    popup_hint_scroll: i32,
    popup_table_state: TableState,
    // popup_category_selector_state: SingleLineSelectorState,
    // popup_scrollbar_state: ScrollbarState,
    // popup_single_line_state: SingleLineSelectorState,
    notifs: Notifications,
    ports: Vec<SerialPortInfo>,
    serial: SerialHandle,
    serial_thread: Takeable<JoinHandle<()>>,

    carousel: CarouselHandle,
    carousel_thread: Takeable<JoinHandle<()>>,

    user_input: UserInput,

    pub buffer: Buffer,
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
    espflash: EspFlashHelper,

    #[cfg(feature = "defmt")]
    pub defmt_helpers: DefmtHelpers,
    // TODO
    // error_message: Option<String>,
    ctrl_c_tx: Sender<()>,
}

impl App {
    pub fn build(
        event_tx: Sender<Event>,
        event_rx: Receiver<Event>,
        ctrl_c_tx: Sender<()>,
        settings: Settings,
    ) -> Result<Self> {
        let keybinds = Keybinds::build()?;

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

        let (serial_buf_tx, serial_buf_rx) = crossbeam::channel::unbounded();

        let (event_carousel, carousel_thread) = CarouselHandle::new();
        let (serial_handle, serial_thread) = SerialHandle::build(
            event_tx.clone(),
            serial_buf_tx,
            settings.last_port_settings.clone(),
            settings.ignored.clone(),
        )
        .expect("Failed to build serial worker!");

        let tick_tx = event_tx.clone();
        event_carousel.add_repeating(
            "PerSecond",
            Box::new(move || {
                tick_tx
                    .send(Tick::PerSecond.into())
                    .map_err(|e| e.to_string())
            }),
            Duration::from_secs(1),
        )?;

        let line_ending = settings.last_port_settings.rx_line_ending.as_bytes();

        #[cfg(feature = "defmt")]
        let mut defmt_helpers = DefmtHelpers::build(
            #[cfg(feature = "defmt_watch")]
            event_tx.clone(),
        )?;

        let mut buffer = Buffer::build(
            line_ending,
            settings.rendering.clone(),
            #[cfg(feature = "logging")]
            settings.logging.clone(),
            #[cfg(feature = "logging")]
            event_tx.clone(),
            #[cfg(feature = "defmt")]
            settings.defmt.clone(),
        )?;

        #[cfg(feature = "defmt")]
        if let Some(last_elf) = defmt_helpers.recent_elfs.last().map(ToOwned::to_owned)
            && last_elf.is_file()
        {
            match _try_load_defmt_elf(
                &last_elf,
                &mut buffer.defmt_decoder,
                &mut defmt_helpers.recent_elfs,
                #[cfg(feature = "logging")]
                &buffer.log_handle,
                #[cfg(feature = "defmt_watch")]
                &mut defmt_helpers.watcher_handle,
            ) {
                Ok(None) => (),
                Ok(Some(locs_err)) => {
                    use tracing::warn;
                    // TODO notify_str when we have history, so this won't clobber
                    // anything important.
                    warn!("Location Error from loaded ELF: {locs_err}");
                }
                Err(e) => {
                    let text = format!("loading last defmt ELF failed! {e}");
                    error!("{text}");
                    // self.notifs.notify_str(text, Color::Red);
                }
            }
        }

        #[cfg(feature = "macros")]
        let macros = {
            let (macros, errors) =
                Macros::load_from_folder(config_adjacent_path(crate::macros::MACROS_DIR_PATH))?;
            for e in errors {
                return Err(e)?;
            }
            macros
        };
        // debug!("{buffer:#?}");
        Ok(Self {
            state: RunningState::Running,
            menu: Menu::PortSelection(PortSelectionElement::Ports),
            popup: None,
            popup_hint_scroll: -2,
            table_state: TableState::new().with_selected(Some(0)),
            baud_selection_state: SingleLineSelectorState::new().with_selected(selected_baud_index),
            popup_menu_scroll: 0,
            popup_table_state: TableState::new(),
            // popup_category_selector_state: SingleLineSelectorState::new(),
            // popup_scrollbar_state: ScrollbarState::default(),
            // popup_single_line_state: SingleLineSelectorState::new(),
            ports: Vec::new(),

            carousel: event_carousel,
            carousel_thread: Takeable::new(carousel_thread),

            serial: serial_handle,
            serial_thread: Takeable::new(serial_thread),

            user_input,

            buffer,

            repeating_line_flip: false,
            failed_send_at: None,

            #[cfg(feature = "macros")]
            macros,
            action_queue: VecDeque::new(),
            scratch: settings.clone(),
            settings,
            keybinds,
            notifs: Notifications::new(event_tx.clone()),
            event_tx,
            event_rx,
            serial_buf_rx,

            #[cfg(feature = "espflash")]
            espflash: EspFlashHelper::build()?,

            #[cfg(feature = "defmt")]
            defmt_helpers,

            user_broke_connection: false,

            ctrl_c_tx,
        })
    }
    fn is_running(&self) -> bool {
        self.state == RunningState::Running
    }
    pub fn run(&mut self, mut terminal: Terminal<impl Backend>) -> Result<()> {
        // Get initial size of buffer.
        self.buffer.update_terminal_size(&mut terminal)?;
        let mut max_draw = Duration::default();
        let mut max_rx_handle = Duration::default();
        let mut max_event_handle = Duration::default();
        let final_app_result = loop {
            let start = Instant::now();
            // TODO performance widget?
            self.draw(&mut terminal)?;
            let end = Instant::now();
            let end1 = end.saturating_duration_since(start);
            max_draw = max_draw.max(end1);

            // Waiting until we either get a normal app event
            // or an incoming serial buffer.
            //
            // Serial devices can sometimes just go *wild* and spit data like crazy,
            // and having other events like port (dis)connection, input handling, etc.,
            // potentially sitting behind queued serial buffers isn't ideal.
            // So Serial RX buffers have their own channel, and when either wakes up the Select,
            // we check both channels, draw, and wait again.
            let mut channel_notifier = Select::new();
            channel_notifier.recv(&self.event_rx);
            channel_notifier.recv(&self.serial_buf_rx);
            // Waiting...
            let _ready_index = channel_notifier.ready();
            // A channel is ready!
            let start2 = Instant::now();

            match self.serial_buf_rx.try_recv() {
                Ok(buf) => self.handle_event(Event::RxBuffer(buf), &mut terminal)?,
                Err(crossbeam::channel::TryRecvError::Empty) => (),
                Err(crossbeam::channel::TryRecvError::Disconnected) => {
                    return Err(NoSenders::SerialRx)?;
                }
            }

            let end2 = start2.elapsed();
            let start3 = Instant::now();

            match self.event_rx.try_recv() {
                Ok(event) => self.handle_event(event, &mut terminal)?,
                Err(crossbeam::channel::TryRecvError::Empty) => (),
                Err(crossbeam::channel::TryRecvError::Disconnected) => {
                    return Err(NoSenders::Events)?;
                }
            }

            let end3 = start3.elapsed();
            max_rx_handle = max_rx_handle.max(end2);
            max_event_handle = max_event_handle.max(end3);
            trace!(
                "Frame took {:?} to draw (max: {max_draw:?}), {:?} to handle RX (max: {max_rx_handle:?}), {:?} to handle event (max: {max_event_handle:?}) ",
                end1, end2, end3
            );
            // debug!("{msg:?}");

            // Don't wait for another loop iteration to start shutting down workers.
            if !self.is_running() {
                break Ok(());
            }
        };
        // Shutting down worker threads, with timeouts
        debug!("Shutting down Serial worker");
        if self.serial.shutdown().is_ok() {
            let serial_thread = self.serial_thread.take();
            if serial_thread.join().is_err() {
                error!("Serial thread closed with an error!");
            }
        }
        debug!("Shutting down event carousel");
        if self.carousel.shutdown().is_ok() {
            let carousel = self.carousel_thread.take();
            if carousel.join().is_err() {
                error!("Carousel thread closed with an error!");
            }
        }
        final_app_result
    }
    fn handle_event(&mut self, event: Event, terminal: &mut Terminal<impl Backend>) -> Result<()> {
        match event {
            Event::Quit => self.shutdown(),

            Event::RxBuffer(data) => {
                self.buffer.fresh_rx_bytes(data);
                self.buffer.scroll_by(0);

                self.repeating_line_flip.flip();
            }

            // TODO force re-draw every minute or so?
            Event::Crossterm(CrosstermEvent::Resize) => {
                terminal.autoresize()?;
                self.buffer.update_terminal_size(terminal)?;
            }
            Event::Crossterm(CrosstermEvent::KeyPress(key)) => self.handle_key_press(key),
            Event::Crossterm(CrosstermEvent::MouseScroll { up })
                if matches!(self.popup, Some(Popup::CurrentKeybinds)) =>
            {
                if up {
                    self.popup_menu_scroll = self.popup_menu_scroll.saturating_sub(1);
                } else {
                    self.popup_menu_scroll += 1;
                }
            }
            Event::Crossterm(CrosstermEvent::MouseScroll { up }) => {
                let amount = if up { 1 } else { -1 };
                self.buffer.scroll_by(amount);
            }

            Event::Crossterm(CrosstermEvent::RightClick) => {
                if let Some(clipboard) = &mut self.user_input.clipboard {
                    match clipboard.get_text() {
                        Ok(clipboard_text) => {
                            let mut previous_value = self.user_input.input_box.value().to_owned();
                            previous_value.push_str(&clipboard_text);
                            self.user_input.input_box = previous_value.into();
                        }
                        Err(e) => {
                            error!("error getting clipboard text: {e}");
                        }
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

                // Dismiss attempt reconnect prompt if visible.
                if let Menu::Terminal(TerminalPrompt::AttemptReconnectPrompt) = &self.menu {
                    self.menu = Menu::Terminal(TerminalPrompt::None);
                }

                self.buffer.scroll_by(0);
                self.user_broke_connection = false;
            }
            Event::Serial(SerialEvent::ConnectionFailed(err)) => {
                todo!("{err}")
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
            Event::Serial(SerialEvent::Ports(ports)) => {
                self.ports = ports;
                if let Menu::PortSelection(PortSelectionElement::Ports) = &self.menu {
                    if self.table_state.selected().is_none() {
                        self.table_state.select(Some(0));
                    }
                }
            }
            Event::Serial(SerialEvent::UnsentTx(unsent)) => todo!("{unsent:?}"),
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
                    .notify_str("Attempted ESP hard reset!", Color::LightYellow),
                EspEvent::Error(e) => {
                    self.notifs.notify_str(&e, Color::Red);
                    self.action_queue.clear();
                }
                _ => self.espflash.consume_event(esp_event, &self.ctrl_c_tx),
            },
            #[cfg(feature = "logging")]
            Event::Logging(LoggingEvent::FinishedReconsumption) => self
                .notifs
                .notify_str("Finished syncing contents to log!", Color::Green),
            #[cfg(feature = "logging")]
            Event::Logging(LoggingEvent::Error(error)) => self
                .notifs
                .notify_str(format!("Logging error: {error}"), Color::Red),
            Event::Tick(Tick::PerSecond) => match self.menu {
                Menu::Terminal(TerminalPrompt::None) => {
                    let port_status = &self.serial.port_status.load().inner;

                    let reconnections_allowed =
                        self.serial.port_settings.load().reconnections.allowed();
                    if !port_status.is_connected()
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
                    let tx = self.event_tx.clone();
                    self.carousel
                        .add_oneshot(
                            "ScrollText",
                            Box::new(move || {
                                tx.send(Tick::Scroll.into()).map_err(|e| e.to_string())
                            }),
                            Duration::from_millis(400),
                        )
                        .unwrap();
                }
            }
            Event::Tick(Tick::Action) => {
                self.consume_one_queued_action()?;
            }
            Event::Tick(Tick::Notification) => {
                // debug!("notif!");
                if let Some(notif) = &self.notifs.inner {
                    let tx = self.event_tx.clone();
                    let emerging = notif.shown_for() <= EMERGE_TIME;
                    let collapsing = notif.shown_for() >= PAUSE_TIME;
                    let sleep_time = if emerging || collapsing {
                        Duration::from_millis(50)
                    } else if notif.replaced && notif.shown_for() <= EXPAND_TIME {
                        EXPAND_TIME.saturating_sub(notif.shown_for())
                    } else {
                        PAUSE_TIME.saturating_sub(notif.shown_for())
                    };
                    self.carousel
                        .add_oneshot(
                            "Notification",
                            Box::new(move || {
                                tx.send(Tick::Notification.into())
                                    .map_err(|e| e.to_string())
                            }),
                            sleep_time,
                        )
                        .unwrap();
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
            #[cfg(feature = "defmt_watch")]
            Event::DefmtElfWatch(ElfWatchEvent::ElfUpdated(elf_path)) => {
                if self.settings.defmt.watch_elf_for_changes {
                    info!("ELF File Watch triggered, reloading ELF at {elf_path}");

                    self.try_load_defmt_elf(
                        &elf_path,
                        #[cfg(feature = "defmt_watch")]
                        true,
                    );
                } else {
                    trace!("Ignoring ELF file watch event!");
                }
            }
            #[cfg(feature = "defmt_watch")]
            Event::DefmtElfWatch(ElfWatchEvent::Error(err)) => {
                self.notifs.notify_str(err, Color::Red);
            }
            Event::DefmtFromFilePicker(elf_path) => self.try_load_defmt_elf(
                &elf_path,
                #[cfg(feature = "defmt_watch")]
                false,
            ),
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
    ) -> Result<(), MacroNotFound> {
        let (macro_tag, macro_content) = self
            .macros
            .all
            .iter()
            .find(|(tag, _string)| &&macro_ref == tag)
            .ok_or(MacroNotFound)?;

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
        // Intentionally putting this before the Ctrl-C ack-er,
        // so it can be used to break any potential hung instances during flashing
        // (not that I've encountered that behavior *yet*),
        // since I don't think I can unstick the serial worker thread
        // if it's stuck in external crate code.
        #[cfg(feature = "espflash")]
        if self.espflash.popup_active() {
            return;
        }

        if is_ctrl_c(&key) {
            match self.ctrl_c_tx.try_send(()) {
                Ok(()) => (),
                Err(TrySendError::Full(_)) => panic!("Ctrl-C ack buffer full??"),
                Err(TrySendError::Disconnected(_)) => panic!("Failed to acknowledge Ctrl-C!"),
            }
        }

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
            Some(Popup::ToolMenu(ToolMenu::Macros)) => {
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

        #[cfg(feature = "defmt")]
        if let Some(Popup::DefmtNewElf(file_explorer)) = &mut self.popup {
            let input = match key.code {
                KeyCode::Left | KeyCode::Char('h') => ratatui_explorer::Input::Left,
                KeyCode::Down | KeyCode::Char('j') => ratatui_explorer::Input::Down,
                KeyCode::Up | KeyCode::Char('k') => ratatui_explorer::Input::Up,
                KeyCode::Right | KeyCode::Char('l') => ratatui_explorer::Input::Right,
                KeyCode::Enter => ratatui_explorer::Input::Right,
                KeyCode::Backspace | KeyCode::BackTab => ratatui_explorer::Input::Left,
                KeyCode::PageUp => ratatui_explorer::Input::PageUp,
                KeyCode::PageDown => ratatui_explorer::Input::PageDown,
                KeyCode::Home => ratatui_explorer::Input::Home,
                KeyCode::End => ratatui_explorer::Input::End,

                _ => ratatui_explorer::Input::None,
            };
            if let Err(e) = file_explorer.handle(input) {
                error!("File Explorer Error: {e}");
                self.notifs
                    .notify_str(format!("Explorer Error: {e}"), Color::Red);
                self.dismiss_popup();
                return;
            };
            match input {
                ratatui_explorer::Input::None => (),
                ratatui_explorer::Input::Right => {
                    let current = file_explorer.current();
                    let is_file = current.is_file();
                    if is_file {
                        use camino::Utf8PathBuf;

                        let elf_path: Utf8PathBuf = current.path().to_owned().try_into().unwrap();

                        self.try_load_defmt_elf(
                            &elf_path,
                            #[cfg(feature = "defmt_watch")]
                            false,
                        );

                        self.dismiss_popup();
                    }
                    return;
                }
                _ => return,
            }
        }

        match self.menu {
            Menu::Terminal(TerminalPrompt::None) if self.popup.is_none() => {
                at_terminal = true;
                match key_combo {
                    // Consuming Ctrl+A so input_box.handle_event doesn't move my cursor.
                    key!(ctrl - a) => (),
                    key!(del) | key!(backspace) if self.user_input.all_text_selected => (),

                    // TODO move into UserInput impl?
                    _text_input if self.settings.behavior.fake_shell => match self
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

                    _terminal_input => {
                        // if let Ok(term_event) = terminput_crossterm::to_terminput(
                        //     crokey::crossterm::event::Event::Key(key),
                        // ) {
                        //     let mut buf = [0; 16];
                        //     term_event.encode(&mut buf, terminput::Encoding::Xterm);
                        // }
                    }
                }
            }
            Menu::Terminal(TerminalPrompt::None) => (),
            Menu::Terminal(TerminalPrompt::DisconnectPrompt) => {
                if let Some(pressed) = DisconnectPrompt::from_key_code(key.code)
                    && !is_ctrl_c(&key)
                {
                    let shift_pressed = key.modifiers.contains(KeyModifiers::SHIFT);
                    let ctrl_pressed = key.modifiers.contains(KeyModifiers::CONTROL);

                    self.disconnect_prompt_choice(pressed, shift_pressed, ctrl_pressed);
                }
            }
            Menu::Terminal(TerminalPrompt::AttemptReconnectPrompt) => {
                if let Some(pressed) = AttemptReconnectPrompt::from_key_code(key.code)
                    && !is_ctrl_c(&key)
                {
                    let shift_pressed = key.modifiers.contains(KeyModifiers::SHIFT);
                    let ctrl_pressed = key.modifiers.contains(KeyModifiers::CONTROL);

                    self.reconnect_prompt_choice(pressed, shift_pressed, ctrl_pressed);
                }
            }
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
            (_, Some(Popup::ToolMenu(ToolMenu::Macros))) => false,
            (Menu::Terminal(TerminalPrompt::None), None) => false,
            _ => true,
        };
        // TODO split this up into more functions based on menu
        match key_combo {
            // Start of _Hardcoded_ keybinds.
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
                self.popup_menu_scroll = 0;
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
            key!(ctrl - r) if self.popup == Some(Popup::CurrentKeybinds) => {
                self.run_method_from_action(AppAction::Base(BaseAction::ReloadKeybinds))
                    .unwrap();
            }
            #[cfg(feature = "macros")]
            key!(ctrl - r) if self.popup == Some(Popup::ToolMenu(ToolMenu::Macros)) => {
                self.run_method_from_action(AppAction::MacroBuiltin(
                    MacroBuiltinAction::ReloadMacros,
                ))
                .unwrap();
            }
            #[cfg(feature = "espflash")]
            key!(ctrl - r) if self.popup == Some(Popup::ToolMenu(ToolMenu::EspFlash)) => {
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

                debug!("{key_combo}: {actions:?}");

                self.queue_action_set(actions, Some(key_combo)).unwrap();
            }
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

        // If it's just one action, and it's something we can handle now, we should.
        if actions.len() == 1
            && let Some(action) = actions.first()
        {
            let port_status = &self.serial.port_status.load().inner;

            if action.requires_connection() && !port_status.is_connected() {
                self.notifs.notify_str(
                    "Action requires healthy port connection! Not acting...",
                    Color::Red,
                );
                return Ok(());
            } else if action.requires_terminal_view() && !matches!(self.menu, Menu::Terminal(_)) {
                self.notifs.notify_str(
                    "Action requires terminal view active! Not acting...",
                    Color::Red,
                );
                return Ok(());
            }

            self.action_dispatch(actions.pop().unwrap(), key_combo_opt)?;
            return Ok(());
        }

        self.action_queue
            .extend(actions.into_iter().map(|a| (key_combo_opt, a)));

        self.event_tx.send(Tick::Action.into())?;

        Ok(())
    }
    pub fn get_action_from_string(&self, action: &str) -> Option<Action> {
        let action = action.trim();

        if action.is_empty() {
            return None;
        }

        // Check for matching app method
        if let Ok(app_action) = action.parse::<AppAction>() {
            return Some(Action::AppAction(app_action));
        }

        #[cfg(feature = "espflash")]
        // Try to find esp profile by exact name match.
        if self.espflash.profile_from_name(action).is_some() {
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
    // To be used only when chewing through queued actions.
    // Refrain from placing single-action keybind logic here.
    fn consume_one_queued_action(&mut self) -> Result<()> {
        let Some((key_combo_opt, action)) = self.action_queue.front() else {
            return Ok(());
        };

        // if action.requires_port_connection() {
        let port_status_guard = self.serial.port_status.load().inner;
        match port_status_guard {
            InnerPortStatus::Connected => (),
            InnerPortStatus::LentOut => {
                let tx = self.event_tx.clone();
                self.carousel
                    .add_oneshot(
                        "ActionQueue",
                        Box::new(move || tx.send(Tick::Action.into()).map_err(|e| e.to_string())),
                        Duration::from_millis(500),
                    )
                    .unwrap();
                return Ok(());
            }
            // Currently this runs even for action chains that don't need the port,
            // but that's such a niche/rare thing right now, so meh.
            InnerPortStatus::Idle | InnerPortStatus::PrematureDisconnect => {
                // InnerPortStatus::Idle | InnerPortStatus::PrematureDisconnect if action.requires_connection() => {
                let text = if self.action_queue.len() == 1 {
                    "Port isn't ready! Not running action...".into()
                } else {
                    format!(
                        "Port isn't ready! Clearing {} queued actions...",
                        self.action_queue.len()
                    )
                };
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
        let tx = self.event_tx.clone();
        self.carousel
            .add_oneshot(
                "ActionQueue",
                Box::new(move || tx.send(Tick::Action.into()).map_err(|e| e.to_string())),
                next_action_delay,
            )
            .unwrap();

        Ok(())
    }
    // Any Err's from here should be considered fatal.
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
                if let Err(MacroNotFound) = self.send_one_macro(name_tag, key_combo_opt) {
                    todo!()
                }
            }
            #[cfg(feature = "espflash")]
            Action::EspFlashProfile(profile) => {
                let Some(profile) = self.espflash.profile_from_name(&profile) else {
                    todo!()
                };
                self.notifs.notify_str(
                    format!("espflash profile: {}", profile.name()),
                    Color::LightBlue,
                );
                self.esp_flash_profile(profile)?;
            }
        }
        Ok(None)
    }
    #[cfg(feature = "espflash")]
    fn esp_flash_profile(&mut self, profile: esp::EspProfile) -> HandleResult<()> {
        #[cfg(feature = "defmt")]
        let profile_defmt_path = profile.defmt_elf_path();

        self.serial.esp_flash_profile(profile)?;

        #[cfg(feature = "defmt")]
        if let Some(elf_path) = profile_defmt_path {
            self.try_load_defmt_elf(
                &elf_path,
                #[cfg(feature = "defmt_watch")]
                false,
            );
        }
        Ok(())
    }
    fn run_method_from_action(&mut self, action: AppAction) -> Result<()> {
        let pretty_bool = |b: bool| {
            if b { "On" } else { "Off" }
        };
        use AppAction as A;
        match action {
            A::Popup(popup) => self.show_popup_menu(popup),

            A::Port(PortAction::ToggleDtr) => {
                self.serial.toggle_signals(true, false)?;
            }
            A::Port(PortAction::ToggleRts) => {
                self.serial.toggle_signals(false, true)?;
            }
            A::Port(PortAction::AssertDtr) => {
                self.serial.write_signals(Some(true), None)?;
            }
            A::Port(PortAction::DeassertDtr) => {
                self.serial.write_signals(Some(false), None)?;
            }
            A::Port(PortAction::AssertRts) => {
                self.serial.write_signals(None, Some(true))?;
            }
            A::Port(PortAction::DeassertRts) => {
                self.serial.write_signals(None, Some(false))?;
            }
            A::Port(PortAction::AttemptReconnectStrict) => {
                self.serial
                    .request_reconnect(Some(Reconnections::StrictChecks))?;
            }
            A::Port(PortAction::AttemptReconnectLoose) => {
                self.serial
                    .request_reconnect(Some(Reconnections::LooseChecks))?;
            }
            A::Base(BaseAction::ToggleTextwrap) => {
                let state = pretty_bool(self.settings.rendering.wrap_text.flip());
                self.buffer
                    .update_render_settings(self.settings.rendering.clone());
                self.settings.save()?;
                self.notifs
                    .notify_str(format!("Toggled Text Wrapping {state}"), Color::Gray);
            }
            A::Base(BaseAction::ToggleTimestamps) => {
                let state = pretty_bool(self.settings.rendering.timestamps.flip());
                self.buffer
                    .update_render_settings(self.settings.rendering.clone());
                self.settings.save()?;
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

            A::Base(BaseAction::ShowKeybinds) => self.show_popup(Popup::CurrentKeybinds),

            #[cfg(feature = "macros")]
            A::MacroBuiltin(MacroBuiltinAction::ReloadMacros) => {
                match Macros::load_from_folder(config_adjacent_path(crate::macros::MACROS_DIR_PATH))
                {
                    Ok((macros, errors)) => {
                        let err_len = errors.len();
                        self.macros = macros;
                        if errors.is_empty() {
                            self.notifs
                                .notify_str("Reloaded Macros Successfully!", Color::Green);
                        } else {
                            self.notifs.notify_str(
                                format!("Reloaded Macros! {err_len} files had errors!"),
                                Color::Yellow,
                            );
                        }
                    }
                    Err(e) => {
                        self.notifs
                            .notify_str(format!("Error opening macros: {e}!"), Color::Red);
                    }
                }
            }

            A::Base(BaseAction::ReloadColors) => {
                if let Err(e) = self.buffer.reload_color_rules() {
                    self.notifs.notify_str(
                        format!("Error reloading Color Rules: {e}! See log for details."),
                        Color::Red,
                    );
                    let report = color_eyre::Report::new(e);
                    error!("Error reloading Color Rules: {report:#}");
                } else {
                    self.notifs
                        .notify_str("Reloaded Color Rules!", Color::Green);
                }
            }

            A::Base(BaseAction::ReloadKeybinds) => match Keybinds::build() {
                Ok(new) => {
                    self.keybinds = new;
                    self.notifs.notify_str("Reloaded Keybinds!", Color::Green);
                }
                Err(e) => {
                    self.notifs.notify_str(
                        format!("Error reloading Keybinds: {e}! See log for details."),
                        Color::Red,
                    );
                    let report = color_eyre::Report::new(e);
                    error!("Error reloading Keybinds: {report:#}");
                }
            },

            #[cfg(feature = "logging")]
            A::Logging(LoggingAction::Sync) => {
                let port_status_guard = self.serial.port_status.load();
                let Some(port_info) = &port_status_guard.current_port else {
                    self.notifs.notify_str(
                        "Not (previously) connected to port? Unable to sync log.",
                        Color::Yellow,
                    );
                    return Ok(());
                };
                self.buffer.relog_buffer()?;
                self.notifs
                    .notify_str("Requested logging start!", Color::Green);
            }

            #[cfg(feature = "espflash")]
            A::Esp(EspAction::EspHardReset) => {
                self.serial.esp_restart(EspRestartType::UserCode)?;
            }

            #[cfg(feature = "espflash")]
            A::Esp(EspAction::EspBootloader) => {
                self.serial
                    .esp_restart(EspRestartType::Bootloader { active: true })?;
            }

            #[cfg(feature = "espflash")]
            A::Esp(EspAction::EspBootloaderUnchecked) => {
                self.serial
                    .esp_restart(EspRestartType::Bootloader { active: false })?;
            }

            #[cfg(feature = "espflash")]
            A::Esp(EspAction::EspDeviceInfo) => {
                self.serial.esp_device_info()?;
            }

            #[cfg(feature = "espflash")]
            A::Esp(EspAction::EspEraseFlash) => {
                self.serial.esp_erase_flash()?;
            }
            #[cfg(feature = "espflash")]
            A::Esp(EspAction::ReloadProfiles) => {
                assert!(
                    !self.espflash.popup_active(),
                    "Shouldn't be able to reload profiles while using one!"
                );
                match EspFlashHelper::build() {
                    Ok(new_helper) => {
                        self.espflash = new_helper;
                        self.notifs
                            .notify_str("Reloaded espflash profiles!", Color::Green);
                    }
                    Err(e) => {
                        self.notifs.notify_str(
                            format!("Error reloading espflash profiles: {e}! See log for details."),
                            Color::Red,
                        );
                        let report = color_eyre::Report::new(e);
                        error!("Error reloading espflash profiles: {report:#}");
                    }
                }
            }
            #[cfg(feature = "defmt")]
            A::ShowDefmtSelect(DefmtSelectAction::SelectRecent) => {
                self.show_popup(Popup::DefmtRecentElf)
            }
            #[cfg(feature = "defmt")]
            A::ShowDefmtSelect(DefmtSelectAction::SelectTui) => {
                self.show_popup(Popup::DefmtNewElf(create_file_explorer()?))
            }
            #[cfg(feature = "defmt")]
            A::ShowDefmtSelect(DefmtSelectAction::SelectSystem) => {
                let tx = self.event_tx.clone();
                std::thread::spawn(move || {
                    let file_opt_res = native_dialog::DialogBuilder::file()
                        .add_filter("ELF (Executable and Linkable Format)", &["elf"])
                        .add_filter("All Files", &[""])
                        .set_location(&"")
                        .open_single_file()
                        .show();
                    if let Ok(Some(file)) = file_opt_res {
                        if let Ok(file_utf8) = Utf8PathBuf::from_path_buf(file) {
                            _ = tx.send(Event::DefmtFromFilePicker(file_utf8));
                        } else {
                            // TODO show in UI?
                            error!("Chosen file has non-UTF-8 path!");
                        }
                    } else {
                        debug!("No file chosen with system file picker?");
                    }
                });
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
        #[cfg(feature = "defmt")]
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
                    || (!port_status_guard.inner.is_connected()
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

        match &self.popup {
            None => (),
            Some(Popup::ErrorMessage(_)) => (),
            Some(Popup::CurrentKeybinds) => {
                self.popup_menu_scroll = self.popup_menu_scroll.saturating_sub(1);
            }
            #[cfg(not(any(feature = "espflash", feature = "macros")))]
            Some(Popup::SettingsMenu(_)) => {
                self.popup_hint_scroll = -2;
                match self.popup_menu_scroll {
                    0 => self.select_last_popup_item(),
                    _ => self.popup_menu_scroll -= 1,
                }
            }
            #[cfg(any(feature = "espflash", feature = "macros"))]
            Some(Popup::SettingsMenu(_)) | Some(Popup::ToolMenu(_)) => {
                self.popup_hint_scroll = -2;
                match self.popup_menu_scroll {
                    0 => self.select_last_popup_item(),
                    _ => self.popup_menu_scroll -= 1,
                }
            }
            #[cfg(feature = "defmt")]
            Some(Popup::DefmtNewElf(_)) => (),
            #[cfg(feature = "defmt")]
            Some(Popup::DefmtRecentElf) => match self.popup_menu_scroll {
                0 => {
                    self.popup_menu_scroll = self
                        .defmt_helpers
                        .recent_elfs
                        .recents_len()
                        .saturating_sub(1)
                }
                _ => self.popup_menu_scroll -= 1,
            },
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
        match &self.popup {
            None => (),
            Some(Popup::ErrorMessage(_)) => (),
            Some(Popup::CurrentKeybinds) => {
                self.popup_menu_scroll += 1;
            }
            #[cfg(not(any(feature = "espflash", feature = "macros")))]
            Some(Popup::SettingsMenu(_)) => {
                self.popup_hint_scroll = -2;
                match self.popup_menu_scroll {
                    _ if self.last_popup_item_selected() => self.popup_menu_scroll = 0,
                    _ => self.popup_menu_scroll += 1,
                }
            }
            #[cfg(any(feature = "espflash", feature = "macros"))]
            Some(Popup::SettingsMenu(_)) | Some(Popup::ToolMenu(_)) => {
                self.popup_hint_scroll = -2;
                match self.popup_menu_scroll {
                    _ if self.last_popup_item_selected() => self.popup_menu_scroll = 0,
                    _ => self.popup_menu_scroll += 1,
                }
            }
            #[cfg(feature = "defmt")]
            Some(Popup::DefmtNewElf(_)) => (),
            #[cfg(feature = "defmt")]
            Some(Popup::DefmtRecentElf) => match self.popup_menu_scroll {
                _ if self.popup_menu_scroll
                    == self
                        .defmt_helpers
                        .recent_elfs
                        .recents_len()
                        .saturating_sub(1) =>
                {
                    self.popup_menu_scroll = 0
                }
                _ => self.popup_menu_scroll += 1,
            },
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
            Some(Popup::CurrentKeybinds) | Some(Popup::ErrorMessage(_)) => (),

            #[cfg(not(any(feature = "espflash", feature = "macros")))]
            Some(Popup::PopupMenu(popup)) if self.popup_menu_scroll == 0 => {
                self.cycle_settings_menu(false);
            }
            #[cfg(any(feature = "espflash", feature = "macros"))]
            Some(Popup::SettingsMenu(_)) | Some(Popup::ToolMenu(_))
                if self.popup_menu_scroll == 0 =>
            {
                self.cycle_menu_type();
            }
            #[cfg(any(feature = "espflash", feature = "macros"))]
            Some(Popup::SettingsMenu(_)) | Some(Popup::ToolMenu(_))
                if self.popup_menu_scroll == 1 =>
            {
                self.cycle_sub_menu(false);
            }
            Some(Popup::SettingsMenu(SettingsMenu::SerialPort)) => {
                self.scratch
                    .last_port_settings
                    .handle_input(ArrowKey::Left, self.get_corrected_popup_index().unwrap())
                    .unwrap();
            }
            Some(Popup::SettingsMenu(SettingsMenu::Behavior)) => {
                self.scratch
                    .behavior
                    .handle_input(ArrowKey::Left, self.get_corrected_popup_index().unwrap())
                    .unwrap();
            }
            Some(Popup::SettingsMenu(SettingsMenu::Rendering)) => {
                self.scratch
                    .rendering
                    .handle_input(ArrowKey::Left, self.get_corrected_popup_index().unwrap())
                    .unwrap();
            }
            #[cfg(feature = "macros")]
            Some(Popup::ToolMenu(ToolMenu::Macros)) => {
                if !self.macros.search_input.value().is_empty() {
                    return;
                }
                self.macros.categories_selector.prev();
                if self.popup_menu_scroll >= 2 {
                    if self.macros.none_visible() {
                        self.popup_menu_scroll = 1;
                    } else {
                        self.popup_menu_scroll = 2;
                    }
                }
            }
            #[cfg(feature = "espflash")]
            Some(Popup::ToolMenu(ToolMenu::EspFlash)) => (),
            #[cfg(feature = "logging")]
            Some(Popup::SettingsMenu(SettingsMenu::Logging)) => {
                if self.popup_menu_scroll == 1 {
                    return;
                }

                let result = self
                    .scratch
                    .logging
                    .handle_input(ArrowKey::Left, self.get_corrected_popup_index().unwrap())
                    .unwrap();
            }
            #[cfg(feature = "defmt")]
            Some(Popup::SettingsMenu(SettingsMenu::Defmt)) => {
                if self.popup_menu_scroll <= 2 {
                    return;
                }

                let result = self
                    .scratch
                    .defmt
                    .handle_input(ArrowKey::Left, self.get_corrected_popup_index().unwrap())
                    .unwrap();
            }
            #[cfg(feature = "defmt")]
            Some(Popup::DefmtNewElf(_)) => (),
            #[cfg(feature = "defmt")]
            Some(Popup::DefmtRecentElf) => (),
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
            Some(Popup::CurrentKeybinds) | Some(Popup::ErrorMessage(_)) => (),
            #[cfg(not(any(feature = "espflash", feature = "macros")))]
            Some(Popup::PopupMenu(popup)) if self.popup_menu_scroll == 0 => {
                self.cycle_settings_menu(true);
            }
            #[cfg(any(feature = "espflash", feature = "macros"))]
            Some(Popup::SettingsMenu(_)) | Some(Popup::ToolMenu(_))
                if self.popup_menu_scroll == 0 =>
            {
                self.cycle_menu_type();
            }
            #[cfg(any(feature = "espflash", feature = "macros"))]
            Some(Popup::SettingsMenu(_)) | Some(Popup::ToolMenu(_))
                if self.popup_menu_scroll == 1 =>
            {
                self.cycle_sub_menu(true);
            }
            Some(Popup::SettingsMenu(SettingsMenu::SerialPort)) => {
                self.scratch
                    .last_port_settings
                    .handle_input(ArrowKey::Right, self.get_corrected_popup_index().unwrap())
                    .unwrap();
            }
            Some(Popup::SettingsMenu(SettingsMenu::Behavior)) => {
                self.scratch
                    .behavior
                    .handle_input(ArrowKey::Right, self.get_corrected_popup_index().unwrap())
                    .unwrap();
            }
            Some(Popup::SettingsMenu(SettingsMenu::Rendering)) => {
                self.scratch
                    .rendering
                    .handle_input(ArrowKey::Right, self.get_corrected_popup_index().unwrap())
                    .unwrap();
            }
            #[cfg(feature = "macros")]
            Some(Popup::ToolMenu(ToolMenu::Macros)) => {
                if !self.macros.search_input.value().is_empty() {
                    return;
                }
                self.macros.categories_selector.next();
                if self.popup_menu_scroll >= 2 {
                    if self.macros.none_visible() {
                        self.popup_menu_scroll = 1;
                    } else {
                        self.popup_menu_scroll = 2;
                    }
                }
            }
            #[cfg(feature = "espflash")]
            Some(Popup::ToolMenu(ToolMenu::EspFlash)) => (),
            #[cfg(feature = "logging")]
            Some(Popup::SettingsMenu(SettingsMenu::Logging)) => {
                if self.popup_menu_scroll == 1 {
                    return;
                }

                let result = self
                    .scratch
                    .logging
                    .handle_input(ArrowKey::Right, self.get_corrected_popup_index().unwrap())
                    .unwrap();
            }
            #[cfg(feature = "defmt")]
            Some(Popup::SettingsMenu(SettingsMenu::Defmt)) => {
                if self.popup_menu_scroll <= 2 {
                    return;
                }

                let result = self
                    .scratch
                    .defmt
                    .handle_input(ArrowKey::Right, self.get_corrected_popup_index().unwrap())
                    .unwrap();
            }
            #[cfg(feature = "defmt")]
            Some(Popup::DefmtNewElf(_)) => (),
            #[cfg(feature = "defmt")]
            Some(Popup::DefmtRecentElf) => (),
        }
        if self.popup.is_some() {
            return;
        }
        if matches!(self.menu, Menu::PortSelection(_))
            && self.baud_selection_state.active
            && self.baud_selection_state.next() >= COMMON_BAUD.len()
        {
            self.baud_selection_state.select(0);
        }
    }
    fn enter_pressed(&mut self, ctrl_pressed: bool, shift_pressed: bool) {
        let serial_healthy = self.serial.port_status.load().inner.is_connected();
        let popup_was_some = self.popup.is_some();
        // debug!("{:?}", self.menu);
        use PortSelectionElement as Pse;
        match self.popup {
            None => (),
            Some(Popup::SettingsMenu(_))
                if self.popup_menu_scroll < ALWAYS_PRESENT_SELECTOR_COUNT =>
            {
                return;
            }
            #[cfg(any(feature = "espflash", feature = "macros"))]
            Some(Popup::ToolMenu(_)) if self.popup_menu_scroll < ALWAYS_PRESENT_SELECTOR_COUNT => {
                return;
            }
            Some(Popup::ErrorMessage(_)) | Some(Popup::CurrentKeybinds) => self.dismiss_popup(),
            Some(Popup::SettingsMenu(SettingsMenu::SerialPort)) => {
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
            Some(Popup::SettingsMenu(SettingsMenu::Behavior)) => {
                self.settings.behavior = self.scratch.behavior.clone();

                self.settings.save().unwrap();
                self.dismiss_popup();
                self.notifs
                    .notify_str("Behavior settings saved!", Color::Green);
            }
            Some(Popup::SettingsMenu(SettingsMenu::Rendering)) => {
                self.settings.rendering = self.scratch.rendering.clone();
                self.buffer
                    .update_render_settings(self.settings.rendering.clone());

                self.settings.save().unwrap();
                self.dismiss_popup();
                self.notifs
                    .notify_str("Rendering settings saved!", Color::Green);
            }
            #[cfg(feature = "macros")]
            Some(Popup::ToolMenu(ToolMenu::Macros)) => {
                let index = self.get_corrected_popup_index().unwrap();
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
            Some(Popup::ToolMenu(ToolMenu::EspFlash)) => {
                if !serial_healthy {
                    self.notifs.notify_str("Port isn't ready!", Color::Red);
                    return;
                }
                let selected = self.get_corrected_popup_index().unwrap();
                // If a profile is selected
                if self.popup_menu_scroll
                    >= esp::ESPFLASH_BUTTON_COUNT + ALWAYS_PRESENT_SELECTOR_COUNT
                {
                    assert!(
                        !self.espflash.is_empty(),
                        "shouldn't have selected a non-existant flash profile"
                    );

                    self.esp_flash_profile(self.espflash.profile_from_index(selected).unwrap())
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
                            if self.settings.espflash.easy_erase_flash
                                || (shift_pressed || ctrl_pressed)
                            {
                                self.run_method_from_action(EspAction::EspEraseFlash.into())
                                    .unwrap();
                            } else {
                                self.notifs.notify_str(
                                    "Press Shift/Ctrl+Enter to erase flash!",
                                    Color::Yellow,
                                );
                            }
                        }
                        unknown => unreachable!("unknown espflash command index {unknown}"),
                    }
                }
            }
            #[cfg(feature = "logging")]
            Some(Popup::SettingsMenu(SettingsMenu::Logging)) => {
                // if Sync Logs button was selected
                if self.popup_menu_scroll == ALWAYS_PRESENT_SELECTOR_COUNT + Logging::VISIBLE_FIELDS
                {
                    self.action_dispatch(
                        Action::AppAction(AppAction::Logging(LoggingAction::Sync)),
                        None,
                    )
                    .unwrap();
                    return;
                }
                // Otherwise, save settings.

                self.settings.logging = self.scratch.logging.clone();
                // let current_port = {
                //     let port_status_guard = self.serial.port_status.load();
                //     port_status_guard.current_port.clone()
                // };
                self.buffer
                    .update_logging_settings(self.settings.logging.clone());

                self.settings.save().unwrap();
                self.dismiss_popup();
                self.notifs
                    .notify_str("Logging settings saved!", Color::Green);
            }
            #[cfg(feature = "defmt")]
            Some(Popup::SettingsMenu(SettingsMenu::Defmt)) => {
                if self.popup_menu_scroll == 2 {
                    // open file selector
                    if shift_pressed || ctrl_pressed {
                        self.action_dispatch(
                            Action::AppAction(AppAction::ShowDefmtSelect(
                                DefmtSelectAction::SelectSystem,
                            )),
                            None,
                        )
                    } else {
                        self.action_dispatch(
                            Action::AppAction(AppAction::ShowDefmtSelect(
                                DefmtSelectAction::SelectTui,
                            )),
                            None,
                        )
                    }
                    .unwrap();

                    return;
                } else if self.popup_menu_scroll == 3 {
                    // open recent selector
                    self.show_popup(Popup::DefmtRecentElf);
                    return;
                }
                // Otherwise, save settings.

                self.settings.defmt = self.scratch.defmt.clone();

                self.buffer
                    .update_defmt_settings(self.settings.defmt.clone());

                self.settings.save().unwrap();
                self.dismiss_popup();
                self.notifs
                    .notify_str("defmt settings saved!", Color::Green);
            }
            #[cfg(feature = "defmt")]
            Some(Popup::DefmtNewElf(_)) => (),
            #[cfg(feature = "defmt")]
            Some(Popup::DefmtRecentElf) => {
                if let Some(selected) = self.popup_table_state.selected() {
                    let elf_path = self
                        .defmt_helpers
                        .recent_elfs
                        .nth_path(selected)
                        .unwrap()
                        .to_owned();
                    self.try_load_defmt_elf(
                        &elf_path,
                        #[cfg(feature = "defmt_watch")]
                        false,
                    );
                    self.dismiss_popup();
                }
            }
        }
        if self.popup.is_some() || popup_was_some {
            return;
        }
        match self.menu {
            Menu::PortSelection(Pse::Ports) => {
                if let Some(info) = self
                    .table_state
                    .selected()
                    .and_then(|index| self.ports.get(index))
                {
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
                        .connect(info, self.scratch.last_port_settings.clone())
                        .unwrap();

                    self.menu = Menu::Terminal(TerminalPrompt::None);
                }
            }
            Menu::PortSelection(Pse::MoreOptions) => {
                self.show_popup_menu(ShowPopupAction::ShowPortSettings)
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
                    let tx = self.event_tx.clone();
                    self.carousel
                        .add_oneshot(
                            "UnhealthyTxUi",
                            Box::new(move || {
                                tx.send(Tick::Requested("Unhealthy TX Background Removal").into())
                                    .map_err(|e| e.to_string())
                            }),
                            FAILED_SEND_VISUAL_TIME,
                        )
                        .unwrap();
                }
            }
            Menu::Terminal(TerminalPrompt::DisconnectPrompt) => {
                if self.table_state.selected().is_none() {
                    return;
                }
                let index = self.table_state.selected().unwrap() as u8;
                self.disconnect_prompt_choice(
                    DisconnectPrompt::try_from(index).unwrap(),
                    shift_pressed,
                    ctrl_pressed,
                );
            }
            Menu::Terminal(TerminalPrompt::AttemptReconnectPrompt) => {
                if self.table_state.selected().is_none() {
                    return;
                }
                let index = self.table_state.selected().unwrap() as u8;
                self.reconnect_prompt_choice(
                    AttemptReconnectPrompt::try_from(index).unwrap(),
                    shift_pressed,
                    ctrl_pressed,
                );
            }
        }
    }
    fn disconnect_prompt_choice(
        &mut self,
        choice: DisconnectPrompt,
        shift_pressed: bool,
        ctrl_pressed: bool,
    ) {
        use PortSelectionElement as Pse;
        match choice {
            DisconnectPrompt::ExitApp => self.shutdown(),
            DisconnectPrompt::Cancel => self.menu = Menu::Terminal(TerminalPrompt::None),
            DisconnectPrompt::OpenPortSettings => {
                self.show_popup_menu(ShowPopupAction::ShowPortSettings);
                self.menu = Menu::Terminal(TerminalPrompt::None);
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
    fn reconnect_prompt_choice(
        &mut self,
        choice: AttemptReconnectPrompt,
        shift_pressed: bool,
        ctrl_pressed: bool,
    ) {
        use PortSelectionElement as Pse;
        match choice {
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
            AttemptReconnectPrompt::Cancel => self.menu = Menu::Terminal(TerminalPrompt::None),
            AttemptReconnectPrompt::OpenPortSettings => {
                self.show_popup_menu(ShowPopupAction::ShowPortSettings);
                self.menu = Menu::Terminal(TerminalPrompt::None);
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
    fn current_popup_selectable_item_count(&self) -> usize {
        match &self.popup {
            #[cfg(feature = "defmt")]
            Some(Popup::DefmtRecentElf) => self.defmt_helpers.recent_elfs.recents_len(),
            Some(Popup::SettingsMenu(settings)) => match settings {
                SettingsMenu::SerialPort => PortSettings::VISIBLE_FIELDS,
                SettingsMenu::Behavior => Behavior::VISIBLE_FIELDS,
                SettingsMenu::Rendering => Rendering::VISIBLE_FIELDS,
                #[cfg(feature = "logging")]
                SettingsMenu::Logging => {
                    1 + // Start/Stop Logging button
                Logging::VISIBLE_FIELDS
                }
                #[cfg(feature = "defmt")]
                SettingsMenu::Defmt => {
                    2 + // Select New/Recent ELF buttons
                Defmt::VISIBLE_FIELDS
                }
            },
            #[cfg(any(feature = "espflash", feature = "macros"))]
            Some(Popup::ToolMenu(tool)) => match tool {
                #[cfg(feature = "macros")]
                ToolMenu::Macros => {
                    1 + // Macros' category selector
                    self.macros.visible_len()
                }

                #[cfg(feature = "espflash")]
                // TODO proper scrollbar for espflash profiles
                ToolMenu::EspFlash => esp::ESPFLASH_BUTTON_COUNT + self.espflash.len(),
            },
            _ => unreachable!("popup {:?} has no item count", self.popup),
        }
    }
    /// Gets corrected index of selected element.
    ///
    /// Returns None if the popup category or subcategory selectors are active.
    ///
    /// Used to select the current active element in tables.
    ///
    /// Panics if no popup is active.
    fn get_corrected_popup_index(&self) -> Option<usize> {
        let Some(popup) = &self.popup else {
            unreachable!("popup {:?} has no item count", self.popup);
        };

        let raw_scroll = self.popup_menu_scroll;

        match (popup, raw_scroll) {
            // Menu selectors active
            (_, 0) => None,
            (_, 1) => None,
            #[cfg(feature = "macros")]
            // Macro Categories selector
            (Popup::ToolMenu(ToolMenu::Macros), ALWAYS_PRESENT_SELECTOR_COUNT) => None,

            // Normal settings menus
            // Just correct for the category selector
            (Popup::SettingsMenu(SettingsMenu::SerialPort), _)
            | (Popup::SettingsMenu(SettingsMenu::Rendering), _)
            | (Popup::SettingsMenu(SettingsMenu::Behavior), _) => {
                Some(self.popup_menu_scroll - ALWAYS_PRESENT_SELECTOR_COUNT)
            }

            #[cfg(feature = "macros")]
            // Macros selection
            (Popup::ToolMenu(ToolMenu::Macros), _) => {
                Some(raw_scroll - (1 + ALWAYS_PRESENT_SELECTOR_COUNT))
            }

            #[cfg(feature = "logging")]
            // Logging settings and sync button
            (Popup::SettingsMenu(SettingsMenu::Logging), _) => {
                Some(raw_scroll - ALWAYS_PRESENT_SELECTOR_COUNT)
            }

            #[cfg(feature = "espflash")]
            // espflash user profiles
            (Popup::ToolMenu(ToolMenu::EspFlash), _)
                if raw_scroll >= esp::ESPFLASH_BUTTON_COUNT + ALWAYS_PRESENT_SELECTOR_COUNT =>
            {
                Some(raw_scroll - (esp::ESPFLASH_BUTTON_COUNT + ALWAYS_PRESENT_SELECTOR_COUNT))
            }
            #[cfg(feature = "espflash")]
            // espflash pre-set action buttons
            (Popup::ToolMenu(ToolMenu::EspFlash), _) => {
                Some(raw_scroll - ALWAYS_PRESENT_SELECTOR_COUNT)
            }

            #[cfg(feature = "defmt")]
            // defmt settings
            (Popup::SettingsMenu(SettingsMenu::Defmt), _)
                if raw_scroll >= 2 + ALWAYS_PRESENT_SELECTOR_COUNT =>
            {
                Some(raw_scroll - (2 + ALWAYS_PRESENT_SELECTOR_COUNT))
            }
            #[cfg(feature = "defmt")]
            // defmt select new/recent elf buttons
            (Popup::SettingsMenu(SettingsMenu::Defmt), _) => {
                Some(raw_scroll - ALWAYS_PRESENT_SELECTOR_COUNT)
            }
            _ => unreachable!("popup {:?} has no item count", self.popup),
        }
    }
    /// Returns true if the final item in a popup is selected,
    /// used for item select wrapping purposes.
    ///
    /// Panics if no popup is active.
    fn last_popup_item_selected(&self) -> bool {
        let Some(popup) = &self.popup else {
            unreachable!("popup {:?} has no item count", self.popup);
        };

        let raw_scroll = self.popup_menu_scroll;
        let selector_corrected_scroll = raw_scroll.saturating_sub(ALWAYS_PRESENT_SELECTOR_COUNT);
        let current_popup_item_count = self.current_popup_selectable_item_count();
        // debug!("{raw_scroll}, {selector_corrected_scroll}, {current_popup_item_count}");
        selector_corrected_scroll >= current_popup_item_count.saturating_sub(1)
    }
    /// Returns true if the final item in a popup is selected,
    /// used for item select wrapping purposes.
    ///
    /// Panics if no popup is active.
    fn select_last_popup_item(&mut self) {
        let Some(popup) = &self.popup else {
            unreachable!("popup {:?} has no item count", self.popup);
        };

        self.popup_menu_scroll = (self.current_popup_selectable_item_count()
            + ALWAYS_PRESENT_SELECTOR_COUNT)
            .saturating_sub(1);
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
        match &self.popup {
            None => (),
            Some(Popup::SettingsMenu(_)) => self.render_popup_menus(frame, area),
            #[cfg(any(feature = "espflash", feature = "macros"))]
            Some(Popup::ToolMenu(_)) => self.render_popup_menus(frame, area),
            Some(Popup::CurrentKeybinds) => {
                let mut scroll: u16 = self.popup_menu_scroll as u16;
                show_keybinds(&self.keybinds, &mut scroll, frame, area, self);
                self.popup_menu_scroll = scroll as usize;
            }
            Some(Popup::ErrorMessage(_)) => todo!(),
            #[cfg(feature = "defmt")]
            Some(Popup::DefmtNewElf(file_explorer)) => {
                let area = centered_rect_size(
                    Size {
                        width: 70,
                        height: 20,
                    },
                    area,
                );
                frame.render_widget(Clear, area);
                frame.render_widget(&file_explorer.widget(), area);
            }
            #[cfg(feature = "defmt")]
            Some(Popup::DefmtRecentElf) => {
                let area = centered_rect_size(
                    Size {
                        width: 80,
                        height: 15,
                    },
                    area,
                );

                let title = Line::raw(" Select from recently used ELFs: ")
                    .centered()
                    .reset();

                let block = Block::bordered()
                    .border_style(Style::new().light_red())
                    .title_top(title);

                let inner = block.inner(area);

                self.popup_table_state.select(Some(self.popup_menu_scroll));

                frame.render_widget(Clear, area);
                frame.render_widget(block, area);
                frame.render_stateful_widget(
                    self.defmt_helpers.recent_elfs.as_table(),
                    inner,
                    &mut self.popup_table_state,
                );
            }
        }
    }
    fn render_popup_menus(&mut self, frame: &mut Frame, area: Rect) {
        let popup_color = match &self.popup {
            Some(Popup::SettingsMenu(SettingsMenu::Rendering)) => Color::Red,
            Some(Popup::SettingsMenu(SettingsMenu::Behavior)) => Color::Blue,
            Some(Popup::SettingsMenu(SettingsMenu::SerialPort)) => Color::Cyan,
            #[cfg(feature = "espflash")]
            Some(Popup::ToolMenu(ToolMenu::EspFlash)) => Color::Magenta,
            #[cfg(feature = "defmt")]
            Some(Popup::SettingsMenu(SettingsMenu::Defmt)) => Color::LightRed,
            #[cfg(feature = "logging")]
            Some(Popup::SettingsMenu(SettingsMenu::Logging)) => Color::Yellow,
            #[cfg(feature = "macros")]
            Some(Popup::ToolMenu(ToolMenu::Macros)) => Color::Green,
            _ => return,
        };

        let center_area = centered_rect_size(
            Size {
                width: area.width.min(60),
                height: area.height.min(17),
            },
            area,
        );
        frame.render_widget(Clear, center_area);

        let block = Block::bordered().border_style(Style::from(popup_color));

        let block_render_area = {
            let mut area = center_area;
            area.height = area.height.saturating_sub(1);
            area.y += 1;

            area
        };

        frame.render_widget(&block, block_render_area);

        // let title_lines = ;
        let mut menu_selector_state = SingleLineSelectorState::new();
        menu_selector_state.active = self.popup_menu_scroll == 0;
        let popup_menu_title_selector = SingleLineSelector::new([
            "Settings".italic(),
            #[cfg(any(feature = "espflash", feature = "macros"))]
            "Tools".italic(),
        ])
        .with_next_symbol(">")
        .with_prev_symbol("<")
        .with_space_padding(true);

        let menu_category_selector_area = {
            let mut line = center_area;
            line.height = 1;
            line
        };

        let category_index = match &self.popup {
            Some(Popup::SettingsMenu(_)) => 0,
            #[cfg(any(feature = "espflash", feature = "macros"))]
            Some(Popup::ToolMenu(_)) => 1,
            _ => unreachable!("popup isnt a settings or tool menu"),
        };

        menu_selector_state.select(category_index);
        frame.render_stateful_widget(
            &popup_menu_title_selector,
            menu_category_selector_area,
            &mut menu_selector_state,
        );
        match &self.popup {
            Some(Popup::SettingsMenu(_)) => {
                self.render_settings_popup(frame, block.inner(center_area), popup_color)
            }
            #[cfg(any(feature = "espflash", feature = "macros"))]
            Some(Popup::ToolMenu(_)) => {
                self.render_tool_popup(frame, block.inner(center_area), popup_color)
            }
            _ => unreachable!("popup isnt a settings or tool menu"),
        }
        // assert selector states
        // #[cfg(not(feature = "macros"))]
        // {
        //     assert!(
        //         menu_selector_state.active == (self.popup_menu_scroll == 0),
        //         "Either a table element needs to be selected, or the menu title widget, but never both or neither."
        //     );
        // }
        // #[cfg(feature = "macros")]
        // {
        //     assert!(
        //         (menu_selector_state.active || self.macros.categories_selector.active)
        //             == selector_range.contains(&self.popup_menu_scroll),
        //         "Either a table element needs to be selected, or the menu title widget, but never both or neither."
        //     );

        //     assert_eq!(
        //         menu_selector_state.active && self.macros.categories_selector.active,
        //         false,
        //         "Both selectors can't be active."
        //     );
        // }
    }
    fn render_settings_popup(
        &mut self,
        frame: &mut Frame,
        center_inner_area: Rect,
        block_color: Color,
    ) {
        let Some(Popup::SettingsMenu(popup)) = &self.popup else {
            return;
        };

        let block = Block::new()
            .borders(Borders::TOP | Borders::BOTTOM)
            .style(Style::from(block_color));

        // // Set active states based on item index
        // #[cfg(not(feature = "macros"))]
        // {
        //     match self.popup_menu_scroll {
        //         0 => menu_selector_state.active = true,

        //         _ => menu_selector_state.active = false,
        //     }

        //     assert!(
        //         menu_selector_state.active == (self.popup_menu_scroll == 0),
        //         "Either a table element needs to be selected, or the menu title widget, but never both or neither."
        //     );
        // }
        // #[cfg(feature = "macros")]
        // {
        //     match (self.popup_menu_scroll, popup) {
        //         (0, _) => {
        //             menu_selector_state.active = true;
        //             self.macros.categories_selector.active = false;
        //         }
        //         (1, PopupMenu::Macros) => {
        //             menu_selector_state.active = false;
        //             self.macros.categories_selector.active = true;
        //         }
        //         (_, _) => {
        //             menu_selector_state.active = false;
        //             self.macros.categories_selector.active = false;
        //         }
        //     }
        //     // Above match should ensure these pass without issue.
        //     let selector_range = match popup {
        //         PopupMenu::Macros => 0..=1,
        //         _ => 0..=0,
        //     };

        //     assert!(
        //         (menu_selector_state.active || self.macros.categories_selector.active)
        //             == selector_range.contains(&self.popup_menu_scroll),
        //         "Either a table element needs to be selected, or the menu title widget, but never both or neither."
        //     );

        //     assert_eq!(
        //         menu_selector_state.active && self.macros.categories_selector.active,
        //         false,
        //         "Both selectors can't be active."
        //     );
        // }

        let setting_menu_selector =
            SingleLineSelector::new(<SettingsMenu as VariantNames>::VARIANTS.iter().copied())
                .with_next_symbol(">")
                .with_prev_symbol("<")
                .with_space_padding(true);
        let selector_area = {
            let mut line = center_inner_area;
            line.height = 1;
            line
        };
        let mut menu_selector_state = SingleLineSelectorState::new();
        menu_selector_state.select(
            <SettingsMenu as VariantArray>::VARIANTS
                .iter()
                .position(|v| v == popup)
                .expect("current menu must exist within enum variantarray"),
        );
        menu_selector_state.active = self.popup_menu_scroll == 1;
        frame.render_widget(&block, selector_area);
        frame.render_stateful_widget(
            &setting_menu_selector,
            selector_area,
            &mut menu_selector_state,
        );

        let settings_area = {
            let mut area = center_inner_area;
            area.height = area.height.saturating_sub(3);
            area.y += 1;
            area
        };

        let button_hint_text_area = {
            let mut area = center_inner_area;
            area.y = area.bottom();
            area.height = 1;
            area
        };

        let bottom_sep_line_area = {
            let mut area = center_inner_area;
            area.y = area.bottom().saturating_sub(2);
            area.height = 1;
            area
        };
        let scrolling_text_area = {
            let mut area = center_inner_area;
            area.y = area.bottom().saturating_sub(1);
            area.height = 1;
            area
        };

        frame.render_widget(
            Block::new()
                .borders(Borders::TOP)
                .border_style(Style::from(block_color)),
            bottom_sep_line_area,
        );

        let scrollbar_style = Style::new().reset();

        let scrollbar = Scrollbar::new(ScrollbarOrientation::VerticalRight)
            .style(scrollbar_style)
            .begin_symbol(Some("↑"))
            .end_symbol(Some("↓"));

        let height = settings_area.height;

        match popup {
            SettingsMenu::SerialPort => {
                let selected = self.get_corrected_popup_index();
                self.popup_table_state.select(selected);
                self.popup_table_state.select_first_column();

                frame.render_stateful_widget(
                    self.scratch.last_port_settings.as_table(),
                    settings_area,
                    &mut self.popup_table_state,
                );

                let text: &str = self
                    .popup_table_state
                    .selected()
                    .map(|i| PortSettings::DOCSTRINGS[i])
                    .unwrap_or("");
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
                    button_hint_text_area,
                );
            }
            SettingsMenu::Behavior => {
                let selected = self.get_corrected_popup_index();
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
                    .unwrap_or("");
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
                    button_hint_text_area,
                );
            }
            SettingsMenu::Rendering => {
                let selected = self.get_corrected_popup_index();
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
                    .unwrap_or("");
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
                    button_hint_text_area,
                );
            }
            #[cfg(feature = "logging")]
            SettingsMenu::Logging => {
                let button_area = {
                    let mut area = settings_area;
                    area.y = area.bottom().saturating_sub(1);
                    area.height = 1;
                    area
                };
                let new_separator = {
                    let mut area = settings_area;
                    area.y = area.bottom().saturating_sub(2);
                    area.height = 1;
                    area
                };
                let settings_area = {
                    let mut area = settings_area;
                    area.height = area.height.saturating_sub(2);
                    area
                };
                let line_block = Block::new()
                    .borders(Borders::TOP)
                    .border_style(Style::from(block_color));

                let logs_dir = config_adjacent_path("logs/");
                let log_path_text = format!("Saving to: {logs_dir}");
                let log_path_line = Line::raw(log_path_text)
                    .all_spans_styled(Color::DarkGray.into())
                    .centered();

                if log_path_line.width() <= bottom_sep_line_area.width as usize {
                    frame.render_widget(log_path_line, bottom_sep_line_area);
                } else {
                    frame.render_widget(log_path_line.right_aligned(), bottom_sep_line_area);
                }

                frame.render_widget(
                    Line::raw("Esc: Close | Enter: Select/Save")
                        .all_spans_styled(Color::DarkGray.into())
                        .centered(),
                    button_hint_text_area,
                );

                let sync_button = sync_logs_button();
                let log_sync_selected = self.popup_menu_scroll
                    == ALWAYS_PRESENT_SELECTOR_COUNT + Logging::VISIBLE_FIELDS;
                if log_sync_selected {
                    self.popup_table_state.select(Some(0));
                    self.popup_table_state.select_first_column();

                    frame.render_stateful_widget(
                        sync_button,
                        button_area,
                        &mut self.popup_table_state,
                    );
                    frame.render_widget(&line_block, new_separator);
                    frame.render_widget(self.scratch.logging.as_table(), settings_area);

                    let text =
                        "Re-sync active log files with entire buffer content and current settings.";
                    render_scrolling_line(
                        text,
                        frame,
                        scrolling_text_area,
                        &mut self.popup_hint_scroll,
                    );
                } else {
                    let selected = self.get_corrected_popup_index();
                    self.popup_table_state.select(selected);
                    self.popup_table_state.select_first_column();

                    frame.render_widget(sync_button, button_area);
                    frame.render_widget(&line_block, new_separator);
                    frame.render_stateful_widget(
                        self.scratch.logging.as_table(),
                        settings_area,
                        &mut self.popup_table_state,
                    );

                    let text: &str = self
                        .popup_table_state
                        .selected()
                        .map(|i| Logging::DOCSTRINGS[i])
                        .unwrap_or("");
                    render_scrolling_line(
                        text,
                        frame,
                        scrolling_text_area,
                        &mut self.popup_hint_scroll,
                    );
                }
            }
            #[cfg(feature = "defmt")]
            SettingsMenu::Defmt => {
                let new_separator = {
                    let mut area = center_inner_area;
                    area.y = area.top().saturating_add(5);
                    area.height = 1;
                    area
                };
                let defmt_settings_area = {
                    let mut area = center_inner_area;
                    area.y = area.top().saturating_add(6);
                    area.height = area.height.saturating_sub(8);
                    area
                };
                let line_block = Block::new()
                    .borders(Borders::TOP)
                    .border_style(Style::from(block_color));
                frame.render_widget(
                    Line::raw("Powered by knurling-rs/defmt v1.0.0!")
                        .all_spans_styled(Color::DarkGray.into())
                        .centered(),
                    bottom_sep_line_area,
                );

                let [
                    elf_title,
                    current_elf,
                    select_new_elf,
                    select_recent_elf,
                    _rest,
                ] = vertical![==1,==1,==1,==1,*=1].areas(settings_area);

                frame.render_widget(
                    Line::raw("Esc: Close | Enter: Select/Save")
                        .all_spans_styled(Color::DarkGray.into())
                        .centered(),
                    button_hint_text_area,
                );

                let settings_selected = self.popup_menu_scroll >= ALWAYS_PRESENT_SELECTOR_COUNT + 2;

                // frame.render_widget(esp::espflash_buttons(), settings_area);
                frame.render_widget(&line_block, new_separator);

                let current_elf_str = if let Some(decoder) = &self.buffer.defmt_decoder {
                    decoder.elf_path.as_str()
                } else {
                    "None"
                };

                let select_style = if self.popup_menu_scroll == 2 {
                    Style::new().reversed()
                } else {
                    Style::new()
                };
                let recent_style = if self.popup_menu_scroll == 3 {
                    Style::new().reversed()
                } else {
                    Style::new()
                };

                let current_elf_text = if let Some(decoder) = &self.buffer.defmt_decoder {
                    Cow::Owned(format!(
                        "Current ELF MD5: {}",
                        &decoder.elf_md5.as_str()[..8]
                    ))
                } else {
                    Cow::Borrowed("Current ELF:")
                };

                frame.render_widget(
                    Line::raw(current_elf_text).centered().dark_gray(),
                    elf_title,
                );
                frame.render_widget(Line::raw(current_elf_str).centered(), current_elf);
                frame.render_widget(
                    Line::raw("[Select ELF File]")
                        .centered()
                        .all_spans_styled(select_style),
                    select_new_elf,
                );
                frame.render_widget(
                    Line::raw("[Select Recent ELF]")
                        .centered()
                        .all_spans_styled(recent_style),
                    select_recent_elf,
                );

                // if self.popup_menu_item ==
                if settings_selected {
                    let selected = self.get_corrected_popup_index();
                    self.popup_table_state.select(selected);
                    self.popup_table_state.select_first_column();

                    use crate::settings::Defmt;

                    frame.render_stateful_widget(
                        self.scratch.defmt.as_table(),
                        defmt_settings_area,
                        &mut self.popup_table_state,
                    );
                    let text: &str = self
                        .popup_table_state
                        .selected()
                        .map(|i| Defmt::DOCSTRINGS[i])
                        .unwrap_or("");
                    render_scrolling_line(
                        text,
                        frame,
                        scrolling_text_area,
                        &mut self.popup_hint_scroll,
                    );
                } else {
                    self.popup_table_state.select(Some(0));
                    self.popup_table_state.select_first_column();
                    let corrected_index = self.get_corrected_popup_index();

                    // frame.render_stateful_widget(
                    //     esp::espflash_buttons(),
                    //     settings_area,
                    //     &mut TableState::new()
                    //         .with_selected_column(0)
                    //         .with_selected(corrected_index),
                    // );
                    frame.render_widget(self.scratch.defmt.as_table(), defmt_settings_area);

                    let hints = [
                        "Select an ELF file to decode defmt packets with. Shift/Ctrl to use native system file picker.",
                        "Select from a list of recently used ELFs.",
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
                    Line::raw("Settings:")
                        .all_spans_styled(Color::DarkGray.into())
                        .centered(),
                    new_separator,
                );
            }
        }
        // TODO
        // shrink scrollbar and change content length based on if its for a submenu or not
        let content_length = self.current_popup_selectable_item_count();
        let mut scrollbar_state =
            ScrollbarState::new(content_length.saturating_sub(height as usize))
                .position(self.popup_table_state.offset());

        frame.render_stateful_widget(
            scrollbar,
            center_inner_area
                .offset(Offset { x: 1, y: 0 })
                .inner(Margin {
                    horizontal: 0,
                    vertical: 1,
                }),
            &mut scrollbar_state,
        );
    }

    fn render_tool_popup(
        &mut self,
        frame: &mut Frame,
        center_inner_area: Rect,
        block_color: Color,
    ) {
        let Some(Popup::ToolMenu(popup)) = &self.popup else {
            return;
        };

        let mut menu_selector_state = SingleLineSelectorState::new();

        // // Set active states based on item index
        // #[cfg(not(feature = "macros"))]
        // {
        //     match self.popup_menu_scroll {
        //         0 => menu_selector_state.active = true,

        //         _ => menu_selector_state.active = false,
        //     }

        //     assert!(
        //         menu_selector_state.active == (self.popup_menu_scroll == 0),
        //         "Either a table element needs to be selected, or the menu title widget, but never both or neither."
        //     );
        // }
        // #[cfg(feature = "macros")]
        // {
        //     match (self.popup_menu_scroll, popup) {
        //         (0, _) => {
        //             menu_selector_state.active = true;
        //             self.macros.categories_selector.active = false;
        //         }
        //         (1, ToolMenu::Macros) => {
        //             menu_selector_state.active = false;
        //             self.macros.categories_selector.active = true;
        //         }
        //         (_, _) => {
        //             menu_selector_state.active = false;
        //             self.macros.categories_selector.active = false;
        //         }
        //     }
        //     // Above match should ensure these pass without issue.
        //     let selector_range = match popup {
        //         ToolMenu::Macros => 0..=1,
        //         _ => 0..=0,
        //     };

        //     assert!(
        //         (menu_selector_state.active || self.macros.categories_selector.active)
        //             == selector_range.contains(&self.popup_menu_scroll),
        //         "Either a table element needs to be selected, or the menu title widget, but never both or neither."
        //     );

        //     assert_eq!(
        //         menu_selector_state.active && self.macros.categories_selector.active,
        //         false,
        //         "Both selectors can't be active."
        //     );
        // }

        // let title_lines = ;

        let popup_menu_title_selector =
            SingleLineSelector::new(<ToolMenu as VariantNames>::VARIANTS.iter().copied())
                .with_next_symbol(">")
                .with_prev_symbol("<")
                .with_space_padding(true);
        let selector_area = {
            let mut line = center_inner_area;
            line.height = 1;
            line
        };
        menu_selector_state.select(
            <ToolMenu as VariantArray>::VARIANTS
                .iter()
                .position(|v| v == popup)
                .unwrap(),
        );
        menu_selector_state.active = self.popup_menu_scroll == 1;
        frame.render_stateful_widget(
            &popup_menu_title_selector,
            selector_area,
            &mut menu_selector_state,
        );

        let settings_area = {
            let mut area = center_inner_area;
            area.height = area.height.saturating_sub(3);
            area.y += 1;
            area
        };

        let hint_text_area = {
            let mut area = center_inner_area;
            area.y = area.bottom();
            area.height = 1;
            area
        };

        let line_area = {
            let mut area = center_inner_area;
            area.y = area.bottom().saturating_sub(2);
            area.height = 1;
            area
        };
        let scrolling_text_area = {
            let mut area = center_inner_area;
            area.y = area.bottom().saturating_sub(1);
            area.height = 1;
            area
        };

        let macros_table_area = {
            let mut area = center_inner_area;
            area.height = area.height.saturating_sub(5);
            area.y += 3;
            area
        };

        frame.render_widget(
            Block::new()
                .borders(Borders::TOP)
                .border_style(Style::from(block_color)),
            line_area,
        );

        let scrollbar_style = Style::new().reset();

        let scrollbar = Scrollbar::new(ScrollbarOrientation::VerticalRight)
            .style(scrollbar_style)
            .begin_symbol(Some("↑"))
            .end_symbol(Some("↓"));

        let height = match popup {
            // TODO do for other popups?
            #[cfg(feature = "macros")]
            ToolMenu::Macros => macros_table_area.height,
            _ => settings_area.height,
        };

        match popup {
            #[cfg(feature = "macros")]
            ToolMenu::Macros => {
                let new_separator = {
                    let mut area = center_inner_area;
                    area.height = 1;
                    area.y += 2;
                    area
                };
                let categories_area = {
                    let mut area = center_inner_area;
                    area.height = 1;
                    area.y += 1;
                    area
                };
                frame.render_widget(
                    Block::new()
                        .borders(Borders::TOP)
                        .border_style(Style::from(block_color)),
                    new_separator,
                );

                if self.macros.search_input.value().is_empty() {
                    let categories_iter = ["Has Bytes", "Strings Only", "All Macros"]
                        .iter()
                        .copied()
                        .map(String::from)
                        .map(Line::raw)
                        .chain(self.macros.categories().map(String::from).map(Line::raw));
                    let categories_selector = SingleLineSelector::new(categories_iter)
                        .with_next_symbol(">")
                        .with_prev_symbol("<")
                        .with_size_hint(popup_menu_title_selector.max_chars());
                    self.macros.categories_selector.active = self.popup_menu_scroll == 2;
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

                let table = self
                    .macros
                    .as_table(&self.keybinds, self.settings.behavior.fuzzy_macro_match);

                let selected = self.get_corrected_popup_index();
                self.popup_table_state.select(selected);
                self.popup_table_state.select_first_column();
                frame.render_stateful_widget(table, macros_table_area, &mut self.popup_table_state);

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
                } else if self.macros.is_empty() {
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
            #[cfg(feature = "espflash")]
            ToolMenu::EspFlash => {
                let new_separator = {
                    let mut area = center_inner_area;
                    area.height = 1;
                    area.y = area.top().saturating_add(5);
                    area
                };
                let bins_area = {
                    let mut area = center_inner_area;
                    area.height = area.height.saturating_sub(8);
                    area.y = area.top().saturating_add(6);
                    area
                };
                let line_block = Block::new()
                    .borders(Borders::TOP)
                    .border_style(Style::from(block_color));
                frame.render_widget(
                    Line::raw("Powered by esp-rs/espflash v4.0.1!")
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

                let profiles_selected = self.popup_menu_scroll
                    >= esp::ESPFLASH_BUTTON_COUNT + ALWAYS_PRESENT_SELECTOR_COUNT;

                // if self.popup_menu_item ==
                if profiles_selected {
                    let corrected_index = self.get_corrected_popup_index().unwrap();
                    self.popup_table_state.select_first_column();
                    self.popup_table_state.select(Some(corrected_index));

                    frame.render_widget(esp::espflash_buttons(false), settings_area);
                    frame.render_widget(&line_block, new_separator);
                    frame.render_stateful_widget(
                        self.espflash.profiles_table(),
                        bins_area,
                        &mut self.popup_table_state,
                    );
                    if let Some(profile) = self.espflash.profile_from_index(corrected_index) {
                        use crate::tui::esp::{EspBins, EspElf, EspProfile};

                        let upper_chip = |chip: &espflash::target::Chip| {
                            chip.to_compact_string().to_ascii_uppercase()
                        };
                        let chip = match &profile {
                            EspProfile::Bins(EspBins { expected_chip, .. })
                            | EspProfile::Elf(EspElf { expected_chip, .. }) => {
                                if let Some(chip) = expected_chip {
                                    Cow::from(upper_chip(chip))
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
                    let corrected_index = self.get_corrected_popup_index();
                    self.popup_table_state.select_first_column();
                    self.popup_table_state.select(corrected_index);
                    frame.render_stateful_widget(
                        esp::espflash_buttons(false),
                        settings_area,
                        &mut self.popup_table_state,
                    );
                    frame.render_widget(&line_block, new_separator);
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
                    new_separator,
                );
            }
        }
        // TODO
        // shrink scrollbar and change content length based on if its for a submenu or not
        let content_length = self.current_popup_selectable_item_count();
        let mut scrollbar_state =
            ScrollbarState::new(content_length.saturating_sub(height as usize))
                .position(self.popup_table_state.offset());

        frame.render_stateful_widget(
            scrollbar,
            center_inner_area
                .offset(Offset { x: 1, y: 0 })
                .inner(Margin {
                    horizontal: 0,
                    vertical: 1,
                }),
            &mut scrollbar_state,
        );
    }

    pub fn terminal_menu(
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
                    if port_state.is_connected() || port_state.is_lent_out() {
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
        let input_symbol = Span::raw(">").style(if port_state.is_connected() {
            input_style.not_reversed().green()
        } else {
            input_style.red()
        });

        frame.render_widget(input_symbol, input_symbol_area);

        let should_position_cursor = !prompt_shown && self.popup.is_none();

        if self.user_input.input_box.value().is_empty() {
            // Leading space leaves room for full-width cursors.
            let port_settings_combo = self
                .keybinds
                .port_settings_hint
                .as_ref()
                .map(CompactString::as_str)
                .unwrap_or_else(|| "UNBOUND");
            let input_hint = Line::raw(format!(
                " Input goes here. `{port_settings_combo}` for port settings.",
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
            TerminalPrompt::DisconnectPrompt => {
                let reconns_paused = if self.settings.last_port_settings.reconnections.allowed()
                    && port_state.is_premature_disconnect()
                {
                    Some("(Auto-reconnections paused while open)")
                } else {
                    None
                };
                DisconnectPrompt::render_prompt_block_popup(
                    Some("Disconnect from port?"),
                    reconns_paused,
                    Style::new().blue(),
                    frame,
                    area,
                    &mut self.table_state,
                );
            }
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
    fn show_popup(&mut self, popup: Popup) {
        self.popup = Some(popup);
        self.refresh_scratch();
        self.popup_hint_scroll = -2;
        self.popup_menu_scroll = 1;

        self.event_tx
            .send(Tick::Scroll.into())
            .map_err(|e| e.to_string())
            .unwrap();
    }
    fn show_popup_menu(&mut self, popup: ShowPopupAction) {
        let popup_menu = match popup {
            ShowPopupAction::ShowPortSettings => Popup::SettingsMenu(SettingsMenu::SerialPort),
            ShowPopupAction::ShowBehavior => Popup::SettingsMenu(SettingsMenu::Behavior),
            ShowPopupAction::ShowRendering => Popup::SettingsMenu(SettingsMenu::Rendering),
            #[cfg(feature = "macros")]
            ShowPopupAction::ShowMacros => Popup::ToolMenu(ToolMenu::Macros),
            #[cfg(feature = "espflash")]
            ShowPopupAction::ShowEspFlash => Popup::ToolMenu(ToolMenu::EspFlash),
            #[cfg(feature = "logging")]
            ShowPopupAction::ShowLogging => Popup::SettingsMenu(SettingsMenu::Logging),
            #[cfg(feature = "defmt")]
            ShowPopupAction::ShowDefmt => Popup::SettingsMenu(SettingsMenu::Defmt),
        };
        self.show_popup(popup_menu);
    }
    fn dismiss_popup(&mut self) {
        self.refresh_scratch();
        self.popup.take();
        self.popup_menu_scroll = 0;
        self.popup_hint_scroll = -2;
    }
    fn cycle_sub_menu(&mut self, next: bool) {
        match &mut self.popup {
            Some(Popup::SettingsMenu(popup)) => {
                let mut new_popup = if next { popup.next() } else { popup.prev() };
                std::mem::swap(popup, &mut new_popup);
            }
            #[cfg(any(feature = "espflash", feature = "macros"))]
            Some(Popup::ToolMenu(popup)) => {
                let mut new_popup = if next { popup.next() } else { popup.prev() };
                std::mem::swap(popup, &mut new_popup);
            }
            _ => return,
        }

        self.refresh_scratch();
        self.popup_hint_scroll = -2;
        #[cfg(feature = "macros")]
        self.macros.search_input.reset();
    }
    #[cfg(any(feature = "espflash", feature = "macros"))]
    fn cycle_menu_type(&mut self) {
        match &self.popup {
            Some(Popup::SettingsMenu(_)) => self.popup.insert(Popup::ToolMenu(
                <ToolMenu as VariantArray>::VARIANTS[0].clone(),
            )),
            Some(Popup::ToolMenu(_)) => self.popup.insert(Popup::SettingsMenu(
                <SettingsMenu as VariantArray>::VARIANTS[0].clone(),
            )),
            _ => return,
        };

        self.refresh_scratch();
        self.popup_hint_scroll = -2;
        #[cfg(feature = "macros")]
        self.macros.search_input.reset();
    }
    pub fn try_cli_connect(
        &mut self,
        port_info: SerialPortInfo,
        baud: Option<u32>,
    ) -> color_eyre::Result<()> {
        let connect_result_rx =
            self.serial
                .cli_connect(port_info, self.settings.last_port_settings.clone(), baud)?;

        match connect_result_rx.recv_timeout(Duration::from_secs(15)) {
            // Got a connection result, if it's Ok(()), we'll keep going, otherwise it'll bail early.
            Ok(connect_result) => connect_result?,
            Err(crossbeam::channel::RecvTimeoutError::Timeout) => {
                color_eyre::eyre::bail!("Connection to supplied port timed out!")
            }
            Err(crossbeam::channel::RecvTimeoutError::Disconnected) => {
                color_eyre::eyre::bail!("Serial worker closed unexpectedly!")
            }
        };

        self.menu = Menu::Terminal(TerminalPrompt::None);

        Ok(())
    }
    #[cfg(feature = "defmt")]
    /// Try to load the given file path as a defmt elf,
    /// if successful, loads decoder into Buffer and Logger
    /// and informs the ELF Watcher about this latest file.
    ///
    /// If you want to handle any actual errors from this process,
    /// use `_try_load_defmt_elf`
    pub fn try_load_defmt_elf(
        &mut self,
        path: &Utf8Path,
        #[cfg(feature = "defmt_watch")] reload: bool,
    ) {
        let success_text = {
            #[cfg(feature = "defmt_watch")]
            if reload {
                "defmt ELF reloaded due to file update!"
            } else {
                "defmt ELF loaded successfully!"
            }
            #[cfg(not(feature = "defmt_watch"))]
            "defmt ELF loaded successfully!"
        };
        let fail_text = {
            #[cfg(feature = "defmt_watch")]
            if reload {
                "defmt ELF auto-reload failed!"
            } else {
                "defmt ELF load failed!"
            }
            #[cfg(not(feature = "defmt_watch"))]
            "defmt ELF load failed!"
        };

        match _try_load_defmt_elf(
            path,
            &mut self.buffer.defmt_decoder,
            &mut self.defmt_helpers.recent_elfs,
            #[cfg(feature = "logging")]
            &self.buffer.log_handle,
            #[cfg(feature = "defmt_watch")]
            &mut self.defmt_helpers.watcher_handle,
        ) {
            Ok(None) => {
                self.notifs.notify_str(success_text, Color::Green);
            }
            Ok(Some(locs_err)) => {
                self.notifs.notify_str(
                    format!("defmt ELF had location data err: {locs_err}"),
                    Color::Yellow,
                );
            }
            Err(e) => {
                let text = format!("{fail_text} {e}");
                error!("{text}");
                self.notifs.notify_str(text, Color::Red);
            }
        }
    }
}
// #[cfg(feature = "defmt")]
// /// Try to load the given file path as a defmt elf,
// /// if successful, loads decoder into Buffer and Logger
// /// and informs the ELF Watcher about this latest file.
// pub fn _try_load_defmt_elf(
//     &mut self,
//     path: &Utf8Path,
// ) -> Result<Option<LocationsError>, YapLoadDefmtError> {
//     _try_load_defmt_elf(
//         path,
//         &mut self.buffer.defmt_decoder,
//         &mut self.defmt_helpers.recent_elfs,
//         #[cfg(feature = "logging")]
//         &self.buffer.log_handle,
//         #[cfg(feature = "defmt_watch")]
//         &mut self.defmt_helpers.watcher_handle,
//     )
// }

#[cfg(feature = "defmt")]
#[derive(Debug, thiserror::Error)]
pub enum YapLoadDefmtError {
    #[error("error adding elf to recents")]
    Recents(#[from] DefmtRecentError),
    #[error("failed parsing defmt from elf")]
    DefmtLoad(#[from] DefmtLoadError),
}

#[cfg(feature = "defmt")]
pub fn _try_load_defmt_elf(
    path: &Utf8Path,
    decoder_opt: &mut Option<Arc<DefmtDecoder>>,
    recent_elfs: &mut DefmtRecentElfs,
    #[cfg(feature = "logging")] logging: &LoggingHandle,
    #[cfg(feature = "defmt_watch")] watcher_handle: &mut ElfWatchHandle,
) -> Result<Option<LocationsError>, YapLoadDefmtError> {
    let new_decoder = DefmtDecoder::from_elf_path(path);
    match new_decoder {
        Ok((new_decoder, locations_err_opt)) => {
            let decoder_arc = Arc::new(new_decoder);
            let _ = decoder_opt.insert(decoder_arc.clone());
            // self.notifs
            //     .notify_str("defmt data parsed from ELF!", Color::Green);
            recent_elfs.elf_loaded(path)?;
            #[cfg(feature = "logging")]
            logging
                .update_defmt_decoder(Some(decoder_arc.clone()))
                .unwrap();
            #[cfg(feature = "defmt_watch")]
            watcher_handle.begin_watch(path).unwrap();

            Ok(locations_err_opt)
        }
        Err(e) => {
            error!("error loading defmt elf {e}");
            return Err(e)?;
            // self.notifs
            //     .notify_str(format!("defmt Error: {e}"), Color::Red);
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
    let orig_area = area;
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
                _left_pause if *scroll <= (overflow_amount as i32) + 3 => (overflow_amount, 0),
                to_right if *scroll <= (overflow_amount as i32) + 3 + (overflow_amount as i32) => (
                    (overflow_amount) - ((*to_right as u16) - ((overflow_amount) + 3)),
                    0,
                ),
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
    let para = Paragraph::new(line).scroll((0, if offset_x > 0 { 0 } else { scroll_x }));
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

#[cfg(feature = "defmt")]
fn create_file_explorer() -> Result<FileExplorer, std::io::Error> {
    use ratatui_explorer::FileExplorer;

    let explorer_theme = ratatui_explorer::Theme::default()
        .with_scroll_padding(1)
        .add_default_title();

    // let root_path = std::path::PathBuf::from("");

    // let base_dirs_opt = directories::BaseDirs::new();

    // let starting_dir = base_dirs_opt
    //     .as_ref()
    //     .map(|base_dirs| base_dirs.home_dir())
    //     .unwrap_or(&root_path);

    FileExplorer::with_theme(explorer_theme)
    // .and_then(|mut e| e.set_cwd(starting_dir).map(|_| e))
}
