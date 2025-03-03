use std::{
    borrow::Cow,
    i32,
    sync::mpsc::{Receiver, Sender},
    thread::JoinHandle,
    time::{Duration, Instant},
};

use arboard::Clipboard;
use color_eyre::{eyre::Result, owo_colors::OwoColorize};
use ratatui::{
    crossterm::event::{KeyCode, KeyEvent, KeyModifiers},
    layout::{Constraint, Layout, Offset, Rect, Size},
    prelude::Backend,
    style::{Style, Stylize},
    text::{Line, Text},
    widgets::{
        Block, Borders, Clear, Paragraph, Row, Scrollbar, ScrollbarOrientation, ScrollbarState,
        Table, TableState, Widget, Wrap,
    },
    Frame, Terminal,
};
use ratatui_macros::{horizontal, line, vertical};
use serialport::{SerialPortInfo, SerialPortType};
use takeable::Takeable;
use tracing::{debug, error, info, instrument};
use tui_input::{backend::crossterm::EventHandler, Input, StateChanged};

use crate::{
    buffer::Buffer,
    event_carousel::{self, CarouselHandle},
    history::{History, UserInput},
    serial::{PrintablePortInfo, SerialEvent, SerialHandle},
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

#[derive(Clone, Debug)]
pub enum Event {
    Crossterm(CrosstermEvent),
    Serial(SerialEvent),
    Tick(Tick),
    Quit,
}

#[derive(Clone, Debug)]
enum Tick {
    // Sent every second
    PerSecond,
    // When just trying to update the UI a little early
    Requested,
}

#[derive(Debug, Default, Clone, Copy)]
pub enum Menu {
    #[default]
    PortSelection,
    Terminal,
}

#[derive(Debug, Default, PartialEq, Eq)]
pub enum RunningState {
    #[default]
    Running,
    Finished,
}

// 0 is for a custom baud rate
const COMMON_BAUD: &[u32] = &[
    4800, 9600, 19200, 38400, 57600, 115200, 230400, 460800, 921600, 0,
];
const COMMON_BAUD_DEFAULT: usize = 5;

pub const LINE_ENDINGS: &[&str] = &["\n", "\r", "\r\n"];
pub const LINE_ENDINGS_DEFAULT: usize = 0;

const FAILED_SEND_VISUAL_TIME: Duration = Duration::from_millis(750);

// Maybe have the buffer in the TUI struct?

pub struct App {
    state: RunningState,
    menu: Menu,
    rx: Receiver<Event>,
    table_state: TableState,
    ports: Vec<SerialPortInfo>,
    serial: SerialHandle,
    serial_thread: Takeable<JoinHandle<()>>,
    // Might ArcBool this in the Handle later
    // Might be worth an enum instead?
    // Or a SerialStatus struct with current_port_info and health status
    serial_healthy: bool,

    carousel: CarouselHandle<Event>,
    carousel_thread: Takeable<JoinHandle<()>>,

    user_input: UserInput,

    buffer: Buffer,
    // Tempted to move these into Buffer, or a new BufferState
    buffer_scroll: usize,
    buffer_scroll_state: ScrollbarState,
    buffer_stick_to_bottom: bool,
    buffer_wrapping: bool,
    // Filled in while drawing UI
    // buffer_rendered_lines: usize,
    repeating_line_flip: bool,
    failed_send_at: Option<Instant>,
}

impl App {
    pub fn new(tx: Sender<Event>, rx: Receiver<Event>) -> Self {
        let (event_carousel, carousel_thread) = CarouselHandle::new(tx.clone());
        let (serial_handle, serial_thread) = SerialHandle::new(tx);

        event_carousel.add_repeating(Event::Tick(Tick::PerSecond), Duration::from_secs(1));
        Self {
            state: RunningState::Running,
            menu: Menu::PortSelection,
            rx,
            table_state: TableState::new().with_selected(Some(0)),
            ports: Vec::new(),

            carousel: event_carousel,
            carousel_thread: Takeable::new(carousel_thread),

            serial: serial_handle,
            serial_thread: Takeable::new(serial_thread),
            serial_healthy: false,

            user_input: UserInput::default(),

            buffer: Buffer::new(),
            buffer_scroll: 0,
            buffer_scroll_state: ScrollbarState::default(),
            buffer_stick_to_bottom: true,
            // buffer_rendered_lines: 0,
            buffer_wrapping: true,

            repeating_line_flip: false,
            failed_send_at: None,
            // failed_send_at: Instant::now(),
        }
    }
    fn is_running(&self) -> bool {
        self.state == RunningState::Running
    }
    pub fn run(&mut self, mut terminal: Terminal<impl Backend>) -> Result<()> {
        while self.is_running() {
            self.draw(&mut terminal)?;
            let msg = self.rx.recv().unwrap();
            // TODO SecondTick event
            match msg {
                Event::Quit => self.state = RunningState::Finished,

                Event::Crossterm(CrosstermEvent::Resize) => {
                    terminal.autoresize()?;
                    if let Ok(size) = terminal.size() {
                        self.buffer.last_terminal_size = size;
                    } else {
                        error!("Failed to query terminal size!");
                    }
                    self.buffer.update_line_count();
                    self.scroll_buffer(0);
                }
                Event::Crossterm(CrosstermEvent::KeyPress(key)) => self.handle_key_press(key),
                Event::Crossterm(CrosstermEvent::MouseScroll { up }) => {
                    let amount = if up { 1 } else { -1 };
                    self.scroll_buffer(amount);
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

                Event::Serial(SerialEvent::Connected) => {
                    info!("Connected!");
                    self.scroll_buffer(0);
                    self.serial_healthy = true;
                }
                Event::Serial(SerialEvent::Disconnected) => {
                    // self.menu = Menu::PortSelection;
                    self.serial_healthy = false;
                }
                Event::Serial(SerialEvent::RxBuffer(mut data)) => {
                    self.buffer.append_bytes(&mut data);
                    self.scroll_buffer(0);

                    self.repeating_line_flip = !self.repeating_line_flip;
                }
                Event::Serial(SerialEvent::Ports(ports)) => {
                    self.ports = ports;
                    if self.table_state.selected().is_none() {
                        self.table_state = TableState::new().with_selected(Some(0));
                    }
                }
                Event::Tick(Tick::PerSecond) => match self.menu {
                    Menu::Terminal => {
                        if !self.serial_healthy {
                            self.repeating_line_flip = !self.repeating_line_flip;
                            self.serial.request_reconnect();
                        }
                    }
                    Menu::PortSelection => {
                        self.serial.request_port_scan();
                    }
                },
                Event::Tick(Tick::Requested) => {
                    debug!("Requested tick recieved.");
                    self.failed_send_at
                        .take_if(|i| i.elapsed() >= FAILED_SEND_VISUAL_TIME);
                }
            }
        }
        // Shutting down worker threads, with timeouts
        if self.serial.shutdown().is_ok() {
            let serial_thread = self.serial_thread.take();
            if let Err(_) = serial_thread.join() {
                error!("Serial thread closed with an error!");
            }
        }
        if self.carousel.shutdown().is_ok() {
            let carousel = self.carousel_thread.take();
            if let Err(_) = carousel.join() {
                error!("Carousel thread closed with an error!");
            }
        }
        Ok(())
    }
    fn handle_key_press(&mut self, key: KeyEvent) {
        let ctrl_pressed = key.modifiers.contains(KeyModifiers::CONTROL);
        let shift_pressed = key.modifiers.contains(KeyModifiers::SHIFT);

        // let at_port_selection = matches!(self.menu, Menu::PortSelection);
        let mut at_port_selection = false;
        match self.menu {
            Menu::Terminal => {
                match self
                    .user_input
                    .input_box
                    .handle_event(&ratatui::crossterm::event::Event::Key(key))
                {
                    // If we changed something in the value when handling the key event,
                    // we should clear the user_history selection.
                    Some(StateChanged {
                        value,
                        cursor: _cursor,
                    }) if value => {
                        self.user_input.history.clear_selection();
                    }
                    _ => (),
                }
            }
            Menu::PortSelection => at_port_selection = true,
        }
        match key.code {
            KeyCode::Char(char) => match char {
                'q' | 'Q' if at_port_selection => self.state = RunningState::Finished,
                'c' | 'C' if ctrl_pressed => {
                    // TODO Quit prompt when connected?
                    self.state = RunningState::Finished
                }
                'w' | 'W' if ctrl_pressed => {
                    self.buffer_wrapping = !self.buffer_wrapping;
                    self.scroll_buffer(0);
                }
                _ => {
                    // self.user_input
                    //     .handle_event(&ratatui::crossterm::event::Event::Key(key));
                }
            },
            KeyCode::PageUp if ctrl_pressed || shift_pressed => self.scroll_buffer(i32::MAX),
            KeyCode::PageDown if ctrl_pressed || shift_pressed => self.scroll_buffer(i32::MIN),
            KeyCode::Delete if ctrl_pressed && shift_pressed => {
                self.user_input.reset();
            }
            // TODO reactive page up/down amounts based on last_size
            KeyCode::PageUp => self.scroll_buffer(10),
            KeyCode::PageDown => self.scroll_buffer(-10),
            // KeyCode::End => self
            //     .event_carousel
            //     .add_event(Event::TickSecond, Duration::from_secs(3)),
            KeyCode::Up => self.up_pressed(),
            KeyCode::Down => self.down_pressed(),
            KeyCode::Left | KeyCode::Right => (),
            KeyCode::Enter => self.enter_pressed(),
            KeyCode::Esc => self.state = RunningState::Finished,
            _ => (),
        }
    }
    fn up_pressed(&mut self) {
        match self.menu {
            Menu::PortSelection => self.scroll_menu_up(),
            Menu::Terminal => self.user_input.scroll(true),
        }
    }
    fn down_pressed(&mut self) {
        match self.menu {
            Menu::PortSelection => self.scroll_menu_down(),
            Menu::Terminal => self.user_input.scroll(false),
        }
    }
    // consider making these some kind of trait method?
    // for the different menus and selections
    // not sure, things are gonna get interesting with the key presses
    fn scroll_menu_up(&mut self) {
        self.table_state.scroll_up_by(1);
    }
    fn scroll_menu_down(&mut self) {
        // self.table_state.select(Some(0));
        self.table_state.scroll_down_by(1);
    }
    fn enter_pressed(&mut self) {
        match self.menu {
            Menu::PortSelection => {
                let selected = self.ports.get(self.table_state.selected().unwrap());
                if let Some(info) = selected {
                    info!("Port {}", info.port_name);

                    self.serial.connect(&info);

                    self.menu = Menu::Terminal;
                }
            }
            Menu::Terminal => {
                if self.serial_healthy {
                    let user_input = self.user_input.input_box.value();
                    self.serial.send_str(user_input);
                    self.buffer.append_user_text(user_input);
                    self.user_input.history.push(user_input);
                    self.user_input.reset();

                    self.repeating_line_flip = !self.repeating_line_flip;
                    // Scroll all the way down
                    // TODO: Make this behavior a toggle
                    self.scroll_buffer(i32::MIN);
                } else {
                    self.failed_send_at = Some(Instant::now());
                    self.carousel
                        .add_oneshot(Event::Tick(Tick::Requested), FAILED_SEND_VISUAL_TIME);
                    // Temporarily show text on red background when trying to send while unhealthy
                }
            }
        }
    }
    pub fn draw(&mut self, terminal: &mut Terminal<impl Backend>) -> Result<()> {
        terminal.draw(|frame| self.render_app(frame))?;
        Ok(())
    }
    fn render_app(&mut self, frame: &mut Frame) {
        self.buffer.last_terminal_size = frame.area().as_size();

        let vertical_slices = Layout::vertical([
            Constraint::Fill(1),
            Constraint::Fill(4),
            Constraint::Fill(1),
        ])
        .split(frame.area());

        match self.menu {
            Menu::PortSelection => port_selection(
                &self.ports,
                COMMON_BAUD[COMMON_BAUD_DEFAULT],
                frame,
                vertical_slices[1],
                &mut self.table_state,
            ),
            Menu::Terminal => self.terminal_menu(frame, frame.area()),
        }
    }

    // #[instrument(skip(self))]
    fn scroll_buffer(&mut self, up: i32) {
        match up {
            0 => (), // Used to trigger scroll update actions from non-user scrolling events.
            // TODO do this proper when wrapping is toggleable
            // Scroll all the way up
            i32::MAX => {
                self.buffer_scroll = 0;
                self.buffer_stick_to_bottom = false;
            }
            // Scroll all the way down
            i32::MIN => self.buffer_scroll = self.line_count(),

            // Scroll up
            x if up > 0 => {
                self.buffer_scroll = self.buffer_scroll.saturating_sub(x as usize);
            }
            // Scroll down
            x if up < 0 => {
                self.buffer_scroll = self.buffer_scroll.saturating_add(x.abs() as usize);
            }
            _ => unreachable!(),
        }

        let last_size = {
            let mut size = self.buffer.last_terminal_size;
            // "2" is the lines from the repeating_pattern_widget and the input buffer.
            // Might need to make more dynamic later?
            size.height = size.height.saturating_sub(2);
            size
        };
        let total_lines = self.line_count();
        let more_lines_than_height = total_lines > last_size.height as usize;

        if up > 0 && more_lines_than_height {
            self.buffer_stick_to_bottom = false;
        } else if self.buffer_scroll + last_size.height as usize >= self.line_count() {
            self.buffer_scroll = self.line_count();
            self.buffer_stick_to_bottom = true;
        }

        if self.buffer_stick_to_bottom {
            let new_pos = total_lines.saturating_sub(last_size.height as usize);
            self.buffer_scroll = new_pos;

            // if more_lines_than_height {
            // }

            // let last_size = self.last_terminal_size;

            // info!(
            //     "total rendered lines: {total_lines}, line vec count: {}",
            //     self.buffer.strings.len()
            // );

            // info!("{}", total_lines);

            // if self.buffer_stick_to_bottom {
            // }

            // TODO Maybe update buffer_scroll_state.content_length in here?
            // But that would require using Paragraph::line_count outside of rendering...
            // if let Some(true) = amount {

            // self.buffer_stick_to_bottom = false;
            // }
        }
        self.buffer_scroll_state = self
            .buffer_scroll_state
            .position(self.buffer_scroll)
            .content_length(self.line_count().saturating_sub(last_size.height as usize));
    }

    fn line_count(&self) -> usize {
        if self.buffer_wrapping {
            self.buffer.line_count()
        } else {
            self.buffer.lines.len()
        }
    }

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
        // buffer: impl Iterator<Item = Line<'a>>,
        // state: &mut TableState
    ) {
        let [terminal_area, line_area, input_area] = vertical![*=1, ==1, ==1].areas(area);

        // let text = Text::from(buffer);
        // let buffer = self.buffer.lines();
        // let lines: Vec<_> = buffer.collect();

        let vert_scroll = self.buffer_scroll as u16;

        // let text = Paragraph::new(buffer.to_owned());

        let para = self.buffer.terminal_paragraph(self.buffer_wrapping);

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

        let para = para.scroll((vert_scroll, 0));

        // frame.render_widget(Clear, terminal);
        frame.render_widget(para, terminal_area);

        // self.buffer_scroll_state = self.buffer_scroll_state.content_length(total_lines);
        // self.buffer_rendered_lines = total_lines;
        // maybe debug_assert this when we roll our own line-counting?

        if !self.buffer_stick_to_bottom {
            let scroll_notice = Line::raw("More... Shift+PgDn to jump to newest").dark_gray();
            let notice_area = {
                let mut rect = terminal_area.clone();
                rect.y = rect.bottom().saturating_sub(1);
                rect.height = 1;
                rect
            };
            frame.render_widget(Clear, notice_area);
            frame.render_widget(scroll_notice, notice_area);
        }

        frame.render_stateful_widget(
            Scrollbar::new(ScrollbarOrientation::VerticalRight)
                .begin_symbol(Some("↑"))
                .end_symbol(Some("↓")),
            terminal_area,
            &mut self.buffer_scroll_state,
        );
        repeating_pattern_widget(
            frame,
            line_area,
            self.repeating_line_flip,
            self.serial_healthy,
        );

        {
            let current_port = self.serial.current_port.load();
            let port_text = match &*current_port {
                Some(port) => {
                    let info = port.info_as_string();
                    if self.serial_healthy {
                        info
                    } else {
                        // Might remove later
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
        }

        if self.user_input.input_box.value().is_empty() {
            let input_hint = Line::raw("Input goes here.").dark_gray();
            frame.render_widget(input_hint, input_area.offset(Offset { x: 1, y: 0 }));
            frame.set_cursor_position(input_area.as_position());
        } else {
            let width = input_area.width.max(1) - 1; // So the cursor doesn't bleed off the edge
            let scroll = self.user_input.input_box.visual_scroll(width as usize);
            let style = match &self.failed_send_at {
                Some(instant) if instant.elapsed() < FAILED_SEND_VISUAL_TIME => {
                    Style::new().on_red()
                }
                _ => Style::new(),
            };
            let input_text = Paragraph::new(self.user_input.input_box.value())
                .scroll((0, scroll as u16))
                .style(style);
            frame.render_widget(input_text, input_area);
            frame.set_cursor_position((
                // Put cursor past the end of the input text
                input_area.x
                    + ((self.user_input.input_box.visual_cursor()).max(scroll) - scroll) as u16,
                input_area.y,
            ));
        }
    }
}

pub fn port_selection(
    ports: &[SerialPortInfo],
    current_baud: u32,
    frame: &mut Frame,
    given_area: Rect,
    state: &mut TableState,
) {
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

    let rows: Vec<Row> = ports
        .iter()
        .map(|p| {
            Row::new(vec![
                &p.port_name,
                match &p.port_type {
                    SerialPortType::UsbPort(usb) => usb.serial_number.as_ref().unwrap(),
                    _ => "",
                },
            ])
        })
        .collect();
    let widths = [Constraint::Percentage(50), Constraint::Percentage(50)];

    let table = Table::new(rows, widths)
        .row_highlight_style(Style::new().reversed())
        .highlight_symbol(">>");

    let [table_area, _filler, baud] = vertical![*=1, ==1, ==1].areas(block.inner(area));

    let static_baud = line![format!("← {current_baud} →")];

    frame.render_widget(block, area);

    frame.render_stateful_widget(table, table_area, state);

    frame.render_widget(static_baud.centered(), baud);
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
