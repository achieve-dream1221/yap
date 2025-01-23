use ratatui::widgets::{Block, Table};
use serialport::SerialPortInfo;

pub fn port_selection(ports: &[SerialPortInfo]) -> Block {
    Block::default()
        .title("Port Selection")
        .borders(ratatui::widgets::Borders::ALL)
}
