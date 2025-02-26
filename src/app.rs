use std::{
    borrow::Cow,
    i32,
    sync::mpsc::{Receiver, Sender},
};

use color_eyre::eyre::Result;
use ratatui::{
    crossterm::event::{KeyCode, KeyEvent, KeyModifiers},
    layout::{Constraint, Layout, Rect, Size},
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
use tracing::{error, info, instrument};

use crate::{
    buffer::Buffer,
    serial::{SerialEvent, SerialHandle},
};

pub enum CrosstermEvent {
    Resize,
    KeyPress(KeyEvent),
    MouseScroll { up: bool },
}

pub enum Event {
    Crossterm(CrosstermEvent),
    Serial(SerialEvent),
    Quit,
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

// Maybe have the buffer in the TUI struct?

pub struct App<'a> {
    state: RunningState,
    menu: Menu,
    rx: Receiver<Event>,
    table_state: TableState,
    ports: Vec<SerialPortInfo>,
    serial: SerialHandle,

    last_terminal_size: Size,

    buffer: Buffer<'a>,
    buffer_scroll: usize,
    buffer_scroll_state: ScrollbarState,
    buffer_stick_to_bottom: bool,
    // Filled in while drawing UI
    buffer_rendered_lines: usize,
    buffer_wrapping: bool,
}

impl App<'_> {
    pub fn new(tx: Sender<Event>, rx: Receiver<Event>, ports: Vec<SerialPortInfo>) -> Self {
        Self {
            state: RunningState::Running,
            menu: Menu::PortSelection,
            rx,
            table_state: TableState::new().with_selected(Some(0)),
            ports,
            serial: SerialHandle::new(tx),
            last_terminal_size: Size::default(),
            buffer: Buffer::new(),
            buffer_scroll: 0,
            buffer_scroll_state: ScrollbarState::default(),
            buffer_stick_to_bottom: true,
            buffer_rendered_lines: 0,
            buffer_wrapping: true,
        }
    }
    fn is_running(&self) -> bool {
        self.state == RunningState::Running
    }
    pub fn run(&mut self, mut terminal: Terminal<impl Backend>) -> Result<()> {
        while self.is_running() {
            self.draw(&mut terminal)?;
            let msg = self.rx.recv().unwrap();
            match msg {
                Event::Quit => self.state = RunningState::Finished,

                Event::Crossterm(CrosstermEvent::Resize) => {
                    terminal.autoresize()?;
                    if let Ok(size) = terminal.size() {
                        self.last_terminal_size = size;
                    } else {
                        error!("Failed to query terminal size!");
                    }
                }
                Event::Crossterm(CrosstermEvent::KeyPress(key)) => self.handle_key_press(key),
                Event::Crossterm(CrosstermEvent::MouseScroll { up }) => {
                    let amount = if up { 1 } else { -1 };
                    self.scroll_buffer(amount);
                }

                Event::Serial(SerialEvent::Connected) => info!("Connected!"),
                Event::Serial(SerialEvent::Disconnected) => self.menu = Menu::PortSelection,
                Event::Serial(SerialEvent::RxBuffer(mut data)) => {
                    self.buffer.append_bytes(&mut data);
                    self.scroll_buffer(0);
                    self.update_line_count();

                    // self.lines.push(Line::raw(self.strings.last().unwrap()));

                    // self.string_buffer += &converted;
                    // info!("{}", self.string_buffer);
                }
            }
        }
        Ok(())
    }
    fn handle_key_press(&mut self, key: KeyEvent) {
        let ctrl_pressed = key.modifiers.contains(KeyModifiers::CONTROL);
        let shift_pressed = key.modifiers.contains(KeyModifiers::SHIFT);
        match key.code {
            KeyCode::Char(char) => match char {
                'q' | 'Q' => self.state = RunningState::Finished,
                'c' | 'C' if ctrl_pressed => {
                    // TODO Quit prompt when connected?
                    self.state = RunningState::Finished
                }
                _ => (),
            },
            KeyCode::PageUp if ctrl_pressed || shift_pressed => self.scroll_buffer(i32::MAX),
            KeyCode::PageDown if ctrl_pressed || shift_pressed => self.scroll_buffer(i32::MIN),
            KeyCode::PageUp => self.scroll_buffer(10),
            KeyCode::PageDown => self.scroll_buffer(-10),
            KeyCode::Up => self.scroll_menu_up(),
            KeyCode::Down => self.scroll_menu_down(),
            KeyCode::Enter => self.enter_pressed(),
            KeyCode::Esc => self.state = RunningState::Finished,
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
        // self.table_state.select(Some(0));
        self.table_state.scroll_down_by(1);
    }
    fn enter_pressed(&mut self) {
        match self.menu {
            Menu::PortSelection => {
                let selected = self.ports.get(self.table_state.selected().unwrap_or(0));
                if let Some(info) = selected {
                    info!("Port {}", info.port_name);

                    self.serial.connect(&info.port_name);

                    self.menu = Menu::Terminal;
                }
            }
            Menu::Terminal => (),
        }
    }
    pub fn draw(&mut self, terminal: &mut Terminal<impl Backend>) -> Result<()> {
        terminal.draw(|frame| self.render_app(frame))?;
        Ok(())
    }
    fn render_app(&mut self, frame: &mut Frame) {
        self.last_terminal_size = frame.area().as_size();

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
        // TODO Unstick from bottom when scrolling up
        // TODO Don't allow scrolling past the contents into the void
        match up {
            0 => (), // Used to trigger scroll update actions from non-user scrolling events.
            // TODO do this proper when wrapping is toggleable
            // Scroll all the way up
            i32::MAX => {
                self.buffer_scroll = 0;
                self.buffer_stick_to_bottom = false;
            }
            // Scroll all the way down
            i32::MIN => self.buffer_scroll = self.buffer_rendered_lines,

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

        if up > 0 {
            self.buffer_stick_to_bottom = false;
        } else if self.buffer_scroll + self.last_terminal_size.height as usize
            > self.buffer_rendered_lines
        {
            self.buffer_scroll = self.buffer_rendered_lines;
            self.buffer_stick_to_bottom = true;
        }

        if self.buffer_stick_to_bottom {
            let last_size = self.last_terminal_size;

            let total_lines = self.line_count();
            let new_pos = total_lines.saturating_sub(last_size.height as usize);

            self.buffer_scroll = new_pos.saturating_add(1);

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
            .content_length(
                self.line_count()
                    .saturating_sub(self.last_terminal_size.height as usize),
            );
    }

    fn line_count(&self) -> usize {
        if self.buffer_wrapping {
            self.buffer.strings.len()
        } else {
            self.buffer_rendered_lines
        }
    }

    fn update_line_count(&mut self) -> usize {
        self.buffer_rendered_lines = if self.buffer_wrapping {
            self.buffer.strings.len()
        } else {
            let paragraph = self.terminal_paragraph(false);
            paragraph.line_count(self.last_terminal_size.width.saturating_sub(1))
        };
        self.buffer_rendered_lines
    }

    // TODO Move this into impl Buffer?
    pub fn terminal_paragraph<'a>(&'a self, styled: bool) -> Paragraph<'a> {
        let coloring = |c: Cow<'a, str>| -> Line<'a> {
            if c.len() < 5 {
                Line::from(c)
            } else {
                let line = Line::from(c);
                // info!("{}", line.spans.len());
                // info!("{}", line.spans[0].content.len());
                // TODO Change this from byte indexing since it might run in the middle of a multi-byte char at some point
                let slice = &&line.spans[0].content[..5];
                match *slice {
                    "Got m" => line.blue(),
                    "ID:0x" => line.green(),
                    "Chan." => line.dark_gray(),
                    "Mode:" => line.yellow(),
                    "Power" => line.red(),
                    _ => line,
                }
            }
        };
        let lines: Vec<_> = self
            .buffer
            .strings
            .iter()
            .map(|s| Cow::Borrowed(s.as_str()))
            .map(|c| if styled { coloring(c) } else { Line::raw(c) })
            .collect();
        // let lines = self.buffer.lines.iter();

        let para = Paragraph::new(lines).block(Block::new().borders(Borders::RIGHT));
        if self.buffer_wrapping {
            para.wrap(Wrap { trim: false })
        } else {
            para
        }
    }

    pub fn terminal_menu<'a>(
        &mut self,
        frame: &mut Frame,
        area: Rect,
        // buffer: impl Iterator<Item = Line<'a>>,
        // state: &mut TableState
    ) {
        let [terminal, line, input] = vertical![*=1, ==1, ==1].areas(area);

        // let text = Text::from(buffer);
        // let buffer = self.buffer.lines();
        // let lines: Vec<_> = buffer.collect();

        let vert_scroll = self.buffer_scroll as u16;

        // let text = Paragraph::new(buffer.to_owned());

        let para = self.terminal_paragraph(true);

        let total_lines = para.line_count(terminal.width.saturating_sub(1));

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

        info!("scroll: {vert_scroll}, lines: {}", self.line_count());

        let para = para.scroll((vert_scroll, 0));

        // frame.render_widget(Clear, terminal);
        frame.render_widget(para, terminal);

        // self.buffer_scroll_state = self.buffer_scroll_state.content_length(total_lines);
        self.buffer_rendered_lines = total_lines;

        // TODO Fix scrollbar, not sure if I need to half it or what.
        // (It's only reaching the bottom when the entirety is off the screen)
        // Need to flip..?
        frame.render_stateful_widget(
            Scrollbar::new(ScrollbarOrientation::VerticalRight)
                .begin_symbol(Some("↑"))
                .end_symbol(Some("↓")),
            terminal,
            &mut self.buffer_scroll_state,
        );
        repeating_pattern_widget(frame, line, false);
    }
}

pub fn port_selection(
    ports: &[SerialPortInfo],
    current_baud: u32,
    frame: &mut Frame,
    area: Rect,
    state: &mut TableState,
) {
    // TODO Width detection for minimum area
    let [_, area, _] = horizontal![==25%, ==50%, ==25%].areas(area);
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

pub fn repeating_pattern_widget(frame: &mut Frame, area: Rect, swap: bool) {
    let repeat_count = area.width as usize / 2;
    let remainder = area.width as usize % 2;
    let base_pattern = if swap { "-~" } else { "~-" };

    let pattern = if remainder == 0 {
        base_pattern.repeat(repeat_count)
    } else {
        base_pattern.repeat(repeat_count) + &base_pattern[..1]
    };

    let pattern_widget = ratatui::widgets::Paragraph::new(pattern);
    frame.render_widget(pattern_widget, area);
}
