use std::sync::mpsc::Receiver;

use color_eyre::{eyre::Result, owo_colors::OwoColorize};
use ratatui::{
    crossterm::event::{KeyCode, KeyEvent, KeyModifiers},
    layout::{Constraint, Layout, Rect},
    prelude::Backend,
    style::{Style, Stylize},
    widgets::{Block, Row, Table, TableState, Widget},
    Frame, Terminal,
};
use ratatui_macros::{horizontal, line, vertical};
use serialport::{SerialPortInfo, SerialPortType};
use tracing::info;

pub enum Event {
    Resize,
    KeyPress(KeyEvent),
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
const DEFAULT_BAUD_INDEX: usize = 5;

// Maybe have the buffer in the TUI struct?

pub struct App {
    state: RunningState,
    menu: Menu,
    rx: Receiver<Event>,
    table_state: TableState,
    ports: Vec<SerialPortInfo>,
}

impl App {
    pub fn new(rx: Receiver<Event>, ports: Vec<SerialPortInfo>) -> Self {
        Self {
            state: RunningState::Running,
            menu: Menu::PortSelection,
            rx,
            table_state: TableState::new().with_selected(Some(0)),
            ports,
        }
    }
    fn is_running(&self) -> bool {
        self.state == RunningState::Running
    }
    pub fn run(&mut self, mut terminal: Terminal<impl Backend>) -> Result<()> {
        self.draw(&mut terminal)?;

        while self.is_running() {
            let msg = self.rx.recv().unwrap();
            match msg {
                Event::Quit => self.state = RunningState::Finished,
                Event::Resize => (),
                Event::KeyPress(key) => self.handle_key_press(key),
            }
            self.draw(&mut terminal)?
        }
        Ok(())
    }
    fn handle_key_press(&mut self, key: KeyEvent) {
        match key.code {
            KeyCode::Char(char) => match char {
                'q' | 'Q' => self.state = RunningState::Finished,
                'c' | 'C' if key.modifiers.contains(KeyModifiers::CONTROL) => {
                    // TODO Quit prompt when connected?
                    self.state = RunningState::Finished
                }
                _ => (),
            },
            KeyCode::Up => self.scroll_up(),
            KeyCode::Down => self.scroll_down(),
            KeyCode::Enter => self.enter_pressed(),
            _ => (),
        }
    }
    // consider making these some kind of trait method?
    // for the different menus and selections
    // not sure, things are gonna get interesting with the key presses
    fn scroll_up(&mut self) {
        self.table_state.scroll_up_by(1);
    }
    fn scroll_down(&mut self) {
        // self.table_state.select(Some(0));
        self.table_state.scroll_down_by(1);
    }
    fn enter_pressed(&mut self) {
        match self.menu {
            Menu::PortSelection => {
                let selected = self.ports.get(self.table_state.selected().unwrap_or(0));
                if let Some(info) = selected {
                    info!("Port {}", info.port_name);
                }
                // connect to port
                self.menu = Menu::Terminal;
            }
            Menu::Terminal => (),
        }
    }
    pub fn draw(&mut self, terminal: &mut Terminal<impl Backend>) -> Result<()> {
        terminal.draw(|frame| self.render_app(frame))?;
        Ok(())
    }
    fn render_app(&mut self, frame: &mut Frame) {
        let vertical_slices = Layout::vertical([
            Constraint::Fill(1),
            Constraint::Fill(2),
            Constraint::Fill(1),
        ])
        .split(frame.area());

        match self.menu {
            Menu::PortSelection => port_selection(
                &self.ports,
                COMMON_BAUD[DEFAULT_BAUD_INDEX],
                frame,
                vertical_slices[1],
                &mut self.table_state,
            ),
            Menu::Terminal => terminal_menu(frame, frame.area()),
        }
    }
}

pub fn terminal_menu(
    frame: &mut Frame,
    area: Rect,
    // state: &mut TableState
) {
    let [terminal, line, input] = vertical![*=1, ==1, ==1].areas(area);

    repeating_pattern_widget(frame, line, false);
}

pub fn port_selection(
    ports: &[SerialPortInfo],
    current_baud: u32,
    frame: &mut Frame,
    area: Rect,
    state: &mut TableState,
) {
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
