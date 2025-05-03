use std::{
    borrow::Cow,
    collections::VecDeque,
    i32,
    sync::mpsc::{Receiver, Sender},
    thread::JoinHandle,
    time::{Duration, Instant},
};

use arboard::Clipboard;
use color_eyre::{eyre::Result, owo_colors::OwoColorize};
use crokey::{KeyCombination, key};
use enum_rotate::EnumRotate;
use itertools::Itertools;
use ratatui::{
    Frame, Terminal,
    crossterm::event::{KeyCode, KeyEvent, KeyModifiers},
    layout::{Constraint, Layout, Margin, Offset, Rect, Size},
    prelude::Backend,
    style::{Color, Modifier, Style, Stylize},
    text::{Line, Span, Text, ToLine},
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
use tracing::{debug, error, info, instrument, warn};
use tui_big_text::{BigText, PixelSize};
use tui_input::{Input, StateChanged, backend::crossterm::EventHandler};
use unicode_width::UnicodeWidthStr;

use crate::{
    event_carousel::{self, CarouselHandle},
    history::{History, UserInput},
    keybinds::{
        Keybinds, SHOW_MACROS, SHOW_PORTSETTINGS, TOGGLE_DTR, TOGGLE_RTS, TOGGLE_TEXTWRAP,
        TOGGLE_TIMESTAMPS,
    },
    macros::{Macro, MacroContent, MacroRef, Macros, MacrosPrompt},
    notifications::{
        EMERGE_TIME, EXPAND_TIME, EXPIRE_TIME, Notification, Notifications, PAUSE_TIME,
    },
    serial::{
        MOCK_PORT_NAME, PortSettings, PrintablePortInfo, ReconnectType, Reconnections, SerialEvent,
        SerialHandle,
    },
    settings::{Behavior, Settings},
    traits::{LastIndex, LineHelpers, ToggleBool},
    tui::{
        buffer::Buffer,
        centered_rect_size,
        prompts::{DisconnectPrompt, PromptTable, centered_rect},
        single_line_selector::{SingleLineSelector, SingleLineSelectorState, StateBottomed},
    },
};

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
    ///
    /// TODO: I hate this.
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
    BehaviorSettings,
    #[cfg(feature = "espflash")]
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
    // Might ArcBool this in the Handle later
    // Might be worth an enum instead?
    // Or a SerialStatus struct with current_port_info and health status
    serial_healthy: bool,
    scratch: ScratchSpace,
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
    macros_tx_queue: VecDeque<(Option<KeyCombination>, MacroRef)>,

    settings: Settings,
    keybinds: Keybinds,
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
        let (serial_handle, serial_thread) =
            SerialHandle::new(tx.clone(), settings.last_port_settings.clone());

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

        let serial_signal_tick_handle = serial_handle.clone();
        event_carousel.add_repeating(
            "SerialSignals",
            Box::new(move || {
                serial_signal_tick_handle
                    .read_signals()
                    .map_err(|e| e.to_string())
            }),
            Duration::from_millis(100),
        );

        let line_ending = &settings.last_port_settings.line_ending;
        let buffer = Buffer::new(
            &line_ending,
            settings.behavior.wrap_text,
            settings.behavior.timestamps,
            settings.behavior.echo_user_text,
        );
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
            serial_healthy: false,
            scratch: ScratchSpace {
                // TODO reconsider, since this should be loaded anyway?
                port: settings.last_port_settings.clone(),
                behavior: settings.behavior.clone(),
            },

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
            settings,
            keybinds: Keybinds::new(),
            notifs: Notifications::new(tx.clone()),
            tx,
            rx,
        }
    }
    fn is_running(&self) -> bool {
        self.state == RunningState::Running
    }
    pub fn run(&mut self, mut terminal: Terminal<impl Backend>) -> Result<()> {
        while self.is_running() {
            let start = Instant::now();
            self.draw(&mut terminal)?;
            let end = Instant::now();
            let msg = self.rx.recv()?;
            let start2 = Instant::now();
            self.handle_event(msg, &mut terminal)?;
            debug!(
                "Frame took {:?} to draw, {:?} to handle ",
                end.saturating_duration_since(start),
                start2.elapsed()
            );
            // debug!("{msg:?}");
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
                terminal.autoresize()?;
                if let Ok(size) = terminal.size() {
                    self.buffer.update_terminal_size(size);
                    // self.buffer.update_wrapped_line_count();
                    // self.buffer.scroll_by(0);
                } else {
                    error!("Failed to query terminal size!");
                }
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
                info!("Connected!");
                self.buffer.scroll_by(0);
                self.serial_healthy = true;
                let text = match reconnect {
                    Some(ReconnectType::PerfectMatch) => "Reconnected to same device!",
                    Some(ReconnectType::UsbStrict) => "Reconnected to same device?",
                    Some(ReconnectType::UsbLoose) => "Connected to similar USB device.",
                    Some(ReconnectType::LastDitch) => "Connected to COM port by name.",
                    None => "",
                    // None => "Connected to port!",
                };
                self.notifs.notify(text, Color::Green);
            }
            Event::Serial(SerialEvent::Disconnected(reason)) => {
                // self.menu = Menu::PortSelection;
                self.serial_healthy = false;
                // if let Some(reason) = reason {
                //     self.notify(format!("Disconnected from port! {reason}"), Color::Red);
                // }
                if reason.is_some() {
                    let reconnect_text = match &self.settings.last_port_settings.reconnections {
                        Reconnections::Disabled => "Not attempting to reconnect.",
                        Reconnections::LooseChecks => "Attempting to reconnect (loose checks).",
                        Reconnections::StrictChecks => "Attempting to reconnect (strict checks).",
                    };
                    self.notifs.notify(
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
            Event::Tick(Tick::PerSecond) => match self.menu {
                Menu::Terminal(TerminalPrompt::None) => {
                    let reconnections_allowed =
                        self.serial.port_settings.load().reconnections != Reconnections::Disabled;
                    if !self.serial_healthy && reconnections_allowed {
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
                        Duration::from_millis(300),
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
        let Some((key_combo_opt, macro_ref)) = self.macros_tx_queue.pop_front() else {
            return;
        };

        let macro_binding = self
            .macros
            .all
            .iter()
            .find(|m| macro_ref.eq_macro(m))
            .expect("Failed to find referenced Macro");

        let (notif_text, notif_color) = match (key_combo_opt, &macro_binding.content) {
            (_, MacroContent::Empty) => {
                ("Macro {macro_binding} is empty!".to_owned(), Color::Yellow)
            }
            (Some(a), _) => (format!("Macro: {macro_binding} [{a}]"), Color::Green),

            (None, _) => (format!("Macro: {macro_binding}"), Color::Green),
        };

        match &macro_binding.content {
            MacroContent::Empty => (),
            MacroContent::Bytes { content, preview } => {
                self.serial
                    .send_bytes(content.clone(), Some(self.buffer.line_ending.as_str()))
                    .unwrap();

                debug!("{}", format!("Sending Macro Bytes: {:02X?}", content));
                self.buffer.append_user_bytes(&content);
            }
            MacroContent::Text(text) => {
                self.serial
                    .send_str(text, self.buffer.line_ending.as_str())
                    .unwrap();
                self.buffer.append_user_text(text);

                debug!("{}", format!("Sending Macro Text: {}", text.escape_debug()));
            }
        };

        self.notifs.notify(notif_text, notif_color);
    }
    // TODO fuzz this
    fn handle_key_press(&mut self, key: KeyEvent) {
        let key_combo = KeyCombination::from(key);
        let ctrl_pressed = key.modifiers.contains(KeyModifiers::CONTROL);
        let shift_pressed = key.modifiers.contains(KeyModifiers::SHIFT);

        // TODO vim-style hjkl menu scroll behaviors

        // let at_port_selection = matches!(self.menu, Menu::PortSelection);
        // TODO soon, redo this variable's name + use
        let mut at_port_selection = false;
        let mut at_terminal = false;
        // Filter for when we decide to handle user *text input*.
        match self.menu {
            Menu::Terminal(TerminalPrompt::None) => {
                if self.popup.is_none() {
                    at_terminal = true;
                }
                match key_combo {
                    // Consuming Ctrl+A so input_box.handle_event doesn't move my cursor.
                    key!(a) if ctrl_pressed => (),
                    key!(del) | key!(backspace) if self.user_input.all_text_selected => (),

                    // TODO move into UserInput impl?
                    _ if self.popup.is_none() => match self
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
        // TODO split this up into more functions based on menu
        match key_combo {
            // Start of hardcoded keybinds.
            key!(q) if at_port_selection => self.shutdown(),
            key!(ctrl - shift - c) => self.shutdown(),
            // move into ctrl-c func?
            key!(ctrl - c) => match self.menu {
                Menu::Terminal(TerminalPrompt::DisconnectPrompt) => self.shutdown(),
                Menu::Terminal(TerminalPrompt::None) => {
                    self.dismiss_popup();
                    self.menu = TerminalPrompt::DisconnectPrompt.into();
                }
                _ => self.shutdown(),
            },
            key!(ctrl - a) if at_terminal && !self.user_input.input_box.value().is_empty() => {
                self.user_input.all_text_selected = true;
            }
            key!(home) if self.popup.is_some() => {
                // TODO make this not as hardcoded here.
                self.macros.ui_state = MacrosPrompt::None;

                self.popup_table_state.select(None);
                self.popup_single_line_state.active = true;
            }
            // TODO ctrl+backspace remove a word
            key!(pageup) if ctrl_pressed || shift_pressed => self.buffer.scroll_by(i32::MAX),
            key!(pagedown) if ctrl_pressed || shift_pressed => self.buffer.scroll_by(i32::MIN),
            key!(delete) | key!(backspace)
                if (ctrl_pressed && shift_pressed) || self.user_input.all_text_selected =>
            {
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
            key!(up) => self.up_pressed(),
            key!(down) => self.down_pressed(),
            key!(left) => self.left_pressed(),
            key!(right) => self.right_pressed(),
            key!(enter) => self.enter_pressed(ctrl_pressed, shift_pressed),
            key!(tab) if at_terminal && self.popup.is_none() => {
                self.user_input.find_input_in_history();
            }
            // KeyCode::Tab => self.tab_pressed(),
            key!(esc) => self.esc_pressed(),
            key_combo => {
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
                // TODO just do a macro queue with KeybindMacro or something
                // or some other object that refers to Macros without having their content.
                match self.macros.macro_from_key_combo(
                    key_combo,
                    &self.keybinds.macros,
                    self.settings.behavior.fuzzy_macro_match,
                ) {
                    Ok(somes) if self.serial_healthy => {
                        if !somes.is_empty() {
                            self.macros_tx_queue.extend(
                                somes
                                    .into_iter()
                                    .map(MacroRef::from)
                                    .map(|m| (Some(key_combo), m)),
                            );
                            self.tx.send(Tick::MacroTx.into()).unwrap();
                        }
                    }
                    Ok(somes) => {
                        let unsent = somes
                            .into_iter()
                            .map(|km| format!("\"{}\"", km.title))
                            .join(", ");
                        self.notifs.notify(
                            format!("Macro: {unsent} [{key_combo}] (Not Sent)"),
                            Color::Yellow,
                        );
                    }
                    Err(Some(nones)) => {
                        let missed = nones.into_iter().map(|km| km.to_string()).join(", ");
                        self.notifs
                            .notify(format!("Macro search failed for: {missed}"), Color::Yellow);
                    }
                    // No macros found.
                    Err(None) => (),
                }
            }
        }
    }
    fn run_method_from_string(&mut self, method: &str) -> Result<()> {
        let m = method;
        match m {
            _ if m == TOGGLE_TEXTWRAP => {
                self.buffer
                    .set_line_wrap(self.settings.behavior.wrap_text.flip());
                self.settings.save().unwrap();
                self.notifs.notify("Toggled Text Wrapping", Color::Gray);
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
                self.settings.behavior.timestamps.flip();
                self.buffer
                    .show_timestamps(self.settings.behavior.timestamps);
                self.settings.save().unwrap();
                self.notifs.notify("Toggled Timestamps", Color::Gray);
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
            unknown => warn!("Unknown keybind: {unknown}"),
        };
        Ok(())
    }
    // fn tab_pressed(&mut self) {}
    fn esc_pressed(&mut self) {
        match self.popup {
            None => (),
            Some(PopupMenu::BehaviorSettings) => {
                self.dismiss_popup();
                return;
            }
            Some(PopupMenu::PortSettings) => {
                self.dismiss_popup();
                return;
            }
            Some(PopupMenu::Macros) => match &self.macros.ui_state {
                MacrosPrompt::None => {
                    self.dismiss_popup();
                    return;
                }
                _ => {
                    self.popup.replace(PopupMenu::Macros);
                }
            },
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
                    PopupMenu::Macros if self.macros.is_empty() => return,
                    _ => (),
                }
                self.popup_single_line_state.active = false;
                self.popup_table_state.select_last();
            }
            Some(PopupMenu::PortSettings) => {
                match self
                    .scratch
                    .port
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
            Some(PopupMenu::Macros) if self.popup_table_state.selected() == Some(0) => {
                self.popup_table_state.select(None);
                self.macros.categories_selector.active = true;
            }
            Some(PopupMenu::Macros) => match &self.macros.ui_state {
                MacrosPrompt::None => {
                    self.scroll_menu_up();
                }
                _ => (),
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
            Menu::Terminal(TerminalPrompt::DisconnectPrompt) => self.scroll_menu_up(),
        }
        self.post_menu_vert_scroll(true);
    }
    fn down_pressed(&mut self) {
        self.user_input.all_text_selected = false;
        match &self.popup {
            None => (),
            Some(popup) if self.macros.categories_selector.active => {
                match popup {
                    PopupMenu::Macros if self.macros.is_empty() => return,
                    _ => (),
                }
                self.macros.categories_selector.active = false;
                self.popup_table_state.select_first();
            }
            Some(PopupMenu::Macros) if self.popup_single_line_state.active => {
                self.popup_single_line_state.active = false;
                self.macros.categories_selector.active = true;
            }

            Some(popup) if self.popup_single_line_state.active => {
                self.popup_single_line_state.active = false;
                self.popup_table_state.select_first();
            }
            Some(PopupMenu::PortSettings) => {
                self.popup_desc_scroll = -2;
                match self
                    .scratch
                    .port
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
                self.popup_desc_scroll = -2;
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
            Some(PopupMenu::Macros)
                if self.popup_table_state.selected()
                    >= Some(self.macros.len().saturating_sub(1)) =>
            {
                self.popup_table_state.select(None);
                self.popup_single_line_state.active = true;
            }
            Some(PopupMenu::Macros) => match &self.macros.ui_state {
                MacrosPrompt::None => {
                    self.popup_desc_scroll = -2;
                    self.scroll_menu_down();
                }
                _ => (),
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
            Menu::Terminal(TerminalPrompt::DisconnectPrompt) => self.scroll_menu_down(),
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
                    .port
                    .handle_input(ArrowKey::Left, &mut self.popup_table_state)
                    .unwrap();
            }
            Some(PopupMenu::BehaviorSettings) => {
                self.scratch
                    .behavior
                    .handle_input(ArrowKey::Left, &mut self.popup_table_state)
                    .unwrap();
            }
            Some(PopupMenu::Macros) => {
                self.macros.categories_selector.prev();
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
            Some(popup) if self.popup_single_line_state.active => {
                self.scroll_popup(true);
            }
            Some(PopupMenu::PortSettings) => {
                self.scratch
                    .port
                    .handle_input(ArrowKey::Right, &mut self.popup_table_state)
                    .unwrap();
            }

            Some(PopupMenu::BehaviorSettings) => {
                self.scratch
                    .behavior
                    .handle_input(ArrowKey::Right, &mut self.popup_table_state)
                    .unwrap();
            }
            Some(PopupMenu::Macros) => {
                self.macros.categories_selector.next();
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
    fn enter_pressed(&mut self, ctrl_pressed: bool, shift_pressed: bool) {
        // debug!("{:?}", self.menu);
        use PortSelectionElement as Pse;
        match self.popup {
            None => (),
            Some(PopupMenu::PortSettings) => {
                self.settings.last_port_settings = self.scratch.port.clone();

                self.buffer.line_ending = self.scratch.port.line_ending.clone();
                self.serial
                    .update_settings(self.scratch.port.clone())
                    .unwrap();

                self.settings.save().unwrap();
                self.dismiss_popup();
                self.notifs.notify("Port settings saved!", Color::Green);
                return;
            }
            Some(PopupMenu::BehaviorSettings) => {
                self.settings.behavior = self.scratch.behavior.clone();
                // TODO update_settings methods?
                self.buffer
                    .show_timestamps(self.settings.behavior.timestamps);
                self.buffer.set_line_wrap(self.settings.behavior.wrap_text);
                self.buffer
                    .set_user_lines(self.settings.behavior.echo_user_text);

                self.settings.save().unwrap();
                self.dismiss_popup();
                self.notifs.notify("Behavior settings saved!", Color::Green);
                return;
            }
            Some(PopupMenu::Macros) => {
                if self.popup_single_line_state.active || self.macros.categories_selector.active {
                    return;
                }
                if !self.serial_healthy {
                    self.notifs.notify("Port isn't ready!", Color::Red);
                    return;
                }
                let Some(index) = self.popup_table_state.selected() else {
                    unreachable!();
                };
                let macro_binding = self.macros.category_filtered_macros().nth(index).unwrap();
                if ctrl_pressed || shift_pressed {
                    // Putting macro content into buffer.
                    match &macro_binding.content {
                        MacroContent::Empty => (),
                        MacroContent::Bytes { content, preview } => {
                            todo!()
                        }
                        MacroContent::Text(text) => {
                            self.user_input.clear();
                            self.user_input.input_box = text.as_str().into();
                            self.dismiss_popup();
                            return;
                        }
                    }
                } else {
                    match &macro_binding.content {
                        MacroContent::Empty => self.notifs.notify("Macro is empty!", Color::Yellow),
                        MacroContent::Bytes { .. } | MacroContent::Text(_) => {
                            self.macros_tx_queue.push_back((None, macro_binding.into()));
                            self.tx.send(Tick::MacroTx.into()).unwrap();
                        }
                    }
                }
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

                    self.scratch.port.baud_rate = baud_rate;

                    self.settings.last_port_settings = self.scratch.port.clone();
                    self.settings.save().unwrap();

                    self.serial
                        .connect(&info, self.scratch.port.clone())
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
                if self.serial_healthy {
                    let user_input = self.user_input.input_box.value();

                    if self.settings.behavior.fake_shell {
                        self.serial
                            .send_str(user_input, self.buffer.line_ending.as_str())
                            .unwrap();
                        self.buffer.append_user_text(user_input);
                        self.user_input.history.push(user_input);

                        self.user_input.clear();
                    } else {
                        self.serial.send_str(user_input, "").unwrap();
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
                    DisconnectPrompt::Disconnect => {
                        self.serial.disconnect().unwrap();
                        // Refresh port listings
                        self.ports.clear();
                        self.serial.request_port_scan().unwrap();

                        self.buffer.clear();
                        // Clear the input box, but keep the user history!
                        self.user_input.clear();

                        self.menu = Menu::PortSelection(Pse::Ports);
                    }
                }
            }
        }
    }
    pub fn draw(&mut self, terminal: &mut Terminal<impl Backend>) -> Result<()> {
        terminal.draw(|frame| self.render_app(frame))?;
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

        self.render_popups(frame, frame.area());

        self.render_notifs(frame, frame.area());

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
        if let Some(popup) = &self.popup {
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
            let area = centered_rect_size(
                Size {
                    width: area.width.min(38),
                    height: area.height.min(16),
                },
                area,
            );
            frame.render_widget(Clear, area);

            let block = Block::bordered().border_style(Style::new().cyan());

            frame.render_widget(&block, area);

            // let title_lines = ;

            let popup_menu_title_selector =
                SingleLineSelector::new(<PopupMenu as VariantNames>::VARIANTS.iter().map(|s| *s))
                    .with_next_symbol(">")
                    .with_prev_symbol("<")
                    .with_space_padding(true);

            let title = {
                let mut line = area.clone();
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

            let area = block.inner(area);

            let settings_area = {
                let mut area = area.clone();
                area.height = area.height.saturating_sub(2);
                area
            };

            let hint_text_area = {
                let mut area = area.clone();
                area.y = area.bottom();
                area.height = 1;
                area
            };

            let line_area = {
                let mut area = area.clone();
                area.y = area.bottom().saturating_sub(2);
                area.height = 1;
                area
            };
            let scrolling_text_area = {
                let mut area = area.clone();
                area.y = area.bottom().saturating_sub(1);
                area.height = 1;
                area
            };

            let macros_table_area = {
                let mut area = area.clone();
                area.height = area.height.saturating_sub(4);
                area.y = area.y.saturating_add(2);
                area
            };

            frame.render_widget(
                Block::new()
                    .borders(Borders::TOP)
                    .border_style(Style::new().cyan()),
                line_area,
            );

            let scrollbar = Scrollbar::new(ScrollbarOrientation::VerticalRight)
                .style(Style::new().reset())
                .begin_symbol(Some("↑"))
                .end_symbol(Some("↓"));

            let content_length = match popup {
                PopupMenu::Macros => self.macros.len(),
                // TODO find more clear way than checking this length
                PopupMenu::PortSettings => PortSettings::DOCSTRINGS.len(),
                PopupMenu::BehaviorSettings => Behavior::DOCSTRINGS.len(),
            };

            let height = match popup {
                PopupMenu::Macros => macros_table_area.height,
                PopupMenu::PortSettings | PopupMenu::BehaviorSettings => settings_area.height,
            };

            match popup {
                PopupMenu::PortSettings => {
                    frame.render_stateful_widget(
                        self.scratch.port.as_table(&mut self.popup_table_state),
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
                PopupMenu::Macros => {
                    // TODO categories selector
                    let new_seperator = {
                        let mut area = area.clone();
                        area.y = area.top().saturating_add(1);
                        area.height = 1;
                        area
                    };
                    let categories_area = {
                        let mut area = area.clone();
                        area.y = area.top();
                        area.height = 1;
                        area
                    };
                    frame.render_widget(
                        Block::new()
                            .borders(Borders::TOP)
                            .border_style(Style::new().cyan()),
                        new_seperator,
                    );
                    // frame.render_widget(
                    //     Line::raw(" <     All Macros    > ").centered(),
                    //     categories_area,
                    // );
                    let categories_iter = ["All Bytes", "All Strings", "All Macros"]
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

                    frame.render_stateful_widget(
                        self.macros
                            .as_table(&self.keybinds, self.settings.behavior.fuzzy_macro_match),
                        macros_table_area,
                        &mut self.popup_table_state,
                    );

                    if let Some(index) = self.popup_table_state.selected() {
                        // let text: &str = self
                        //     .popup_table_state
                        //     .selected()
                        //     .map(|i| )
                        //     .unwrap_or(&"");

                        let macro_binding =
                            self.macros.category_filtered_macros().nth(index).unwrap();
                        let macro_preview = macro_binding.preview();
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
                        frame.render_widget(
                            Line::raw("Select macro to preview.").centered(),
                            scrolling_text_area,
                        );
                    }

                    frame.render_widget(
                        Line::raw("Del: Remove | Ctrl+N: New")
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
                    // match prompt {
                    //     _ => (),
                    // };
                }
            }

            // if render_scrollbar {
            self.popup_scrollbar_state = self
                .popup_scrollbar_state
                .content_length(content_length.saturating_sub(height as usize));

            self.popup_scrollbar_state = self
                .popup_scrollbar_state
                .position(self.popup_table_state.offset());
            frame.render_stateful_widget(
                scrollbar,
                area.offset(Offset { x: 1, y: 0 }).inner(Margin {
                    horizontal: 0,
                    vertical: 1,
                }),
                &mut self.popup_scrollbar_state,
            );
            // }
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
        frame.render_widget(&mut self.buffer, terminal_area);

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

        repeating_pattern_widget(
            frame,
            line_area,
            self.repeating_line_flip,
            self.serial_healthy,
        );

        // #[cfg(debug_assertions)]
        // {
        //     let line = Line::raw(format!(
        //         "Entries: {} | Lines: {}",
        //         self.buffer.lines.len(),
        //         self.buffer.line_count()
        //     ))
        //     .right_aligned();
        //     frame.render_widget(
        //         line,
        //         line_area.inner(Margin {
        //             horizontal: 3,
        //             vertical: 0,
        //         }),
        //     );
        // }

        // #[cfg(not(debug_assertions))]
        // {
        //     let line = Line::raw(format!("Lines: {}", self.buffer.lines.len())).right_aligned();
        //     frame.render_widget(
        //         line,
        //         line_area.inner(Margin {
        //             horizontal: 3,
        //             vertical: 0,
        //         }),
        //     );
        // }

        {
            let port_status_guard = self.serial.port_status.load();
            let port_text = match &port_status_guard.current_port {
                Some(port_info) => {
                    if self.serial_healthy {
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
                let dtr = reversed_if_true(port_status_guard.signals.dtr);
                let rts = reversed_if_true(port_status_guard.signals.rts);

                let cts = reversed_if_true(port_status_guard.signals.cts);
                let dsr = reversed_if_true(port_status_guard.signals.dsr);
                let ri = reversed_if_true(port_status_guard.signals.ri);
                let cd = reversed_if_true(port_status_guard.signals.cd);

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
            frame.render_widget(signals_line, line_area.offset(Offset { x: 3, y: 0 }));
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
        let input_symbol = Span::raw(">").style(if self.serial_healthy {
            input_style.not_reversed().green()
        } else {
            input_style.red()
        });

        frame.render_widget(input_symbol, input_symbol_area);

        let should_position_cursor = !disconnect_prompt_shown && self.popup.is_none();

        if self.user_input.input_box.value().is_empty() {
            // Leading space leaves room for full-width cursors.
            // TODO make binding hint dynamic (should maybe cache?)
            let input_hint = Line::raw(" Input goes here. `Ctrl + .` for port settings.")
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
                "Disconnect from port?",
                Style::new().blue(),
                frame,
                area,
                &mut self.table_state,
            );
            // frame.render_widget(Clear, area);
            // frame.render_stateful_widget(save_device_prompt, area, &mut self.table_state);
        }
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
        self.scratch = ScratchSpace {
            behavior: self.settings.behavior.clone(),
            port: self.serial.port_settings.load().as_ref().clone(),
        }
    }
    fn dismiss_popup(&mut self) {
        self.refresh_scratch();
        self.popup.take();
        self.macros.categories_selector.active = false;
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
    }
}

pub fn repeating_pattern_widget(frame: &mut Frame, area: Rect, swap: bool, healthy: bool) {
    let repeat_count = area.width as usize / 2;
    let remainder = area.width as usize % 2;
    let base_pattern = if swap { "-~" } else { "~-" };

    let pattern = if remainder == 0 {
        base_pattern.repeat(repeat_count)
    } else {
        base_pattern.repeat(repeat_count) + &base_pattern[..1]
    };

    let pattern_widget = ratatui::widgets::Paragraph::new(pattern);
    let pattern_widget = if healthy {
        pattern_widget.green()
    } else {
        pattern_widget.red()
    };
    frame.render_widget(pattern_widget, area);
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
