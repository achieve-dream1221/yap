use std::sync::mpsc::Receiver;

use color_eyre::eyre::Result;
use ratatui::{
    crossterm::event::{KeyCode, KeyEvent},
    layout::{Constraint, Layout, Rect},
    prelude::Backend,
    style::{Style, Stylize},
    widgets::{Block, Row, Table, TableState, Widget},
    Frame, Terminal,
};
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
    pub const fn new(rx: Receiver<Event>, ports: Vec<SerialPortInfo>) -> Self {
        Self {
            state: RunningState::Running,
            menu: Menu::PortSelection,
            rx,
            table_state: TableState::new(),
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
                'q' => self.state = RunningState::Finished,
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
            }
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
                frame,
                vertical_slices[1],
                &mut self.table_state,
            ),
        }
    }
}

// pub fn terminal_menu(frame: &mut Frame, area: Rect, state: &mut TableState) {}

pub fn port_selection(
    ports: &[SerialPortInfo],
    frame: &mut Frame,
    area: Rect,
    state: &mut TableState,
) {
    let block = Block::default()
        .title("Port Selection")
        .borders(ratatui::widgets::Borders::ALL);

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
        .block(block)
        .row_highlight_style(Style::new().reversed())
        .highlight_symbol(">>");
    // .widths(&[Constraint::Percentage(50), Constraint::Percentage(50)]);

    frame.render_stateful_widget(table, area, state);
}
