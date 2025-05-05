use std::{io::Write, net::TcpListener};

// Awesome idea yoinked from fasterthanlime,
// https://youtu.be/Yr9qy9reLCc?t=261
fn main() -> color_eyre::Result<()> {
    let addr = "127.0.0.1:7331";
    println!("TCP Log Listener started on {addr}!");
    let listener = TcpListener::bind(addr)?;

    loop {
        let (mut stream, addr) = listener.accept()?;
        print_separator()?;
        println!("New client connected! {addr:?}");

        let mut stdout = std::io::stdout();
        if let Err(e) = std::io::copy(&mut stream, &mut stdout) {
            stdout.flush()?;
            println!();
            println!("Error, re-listening. {e:?}");
        }
        print_separator()?;
    }
}

fn print_separator() -> Result<(), std::io::Error> {
    let (x, _y) = ratatui::crossterm::terminal::size()?;
    let separator: String = format!(
        // ANSI Dark Gray then Reset
        "\x1b[90m{}\x1b[0m",
        std::iter::repeat("#").take(x as usize).collect::<String>()
    );
    println!("{}", separator);
    Ok(())
}

// TODO ctrl-c handler for zed's windows terminal :eyeroll:
