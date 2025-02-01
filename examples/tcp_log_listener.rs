use std::{io::Write, net::TcpListener};

// Awesome idea yoinked from fasterthanlime,
// https://youtu.be/Yr9qy9reLCc?t=261
fn main() {
    let addr = "127.0.0.1:7331";
    println!("TCP Log Listener started on {addr}!");
    let listener = TcpListener::bind(addr).unwrap();

    loop {
        let (mut stream, addr) = listener.accept().unwrap();
        println!("New client connected! {addr:?}");

        let mut stdout = std::io::stdout();
        if let Err(e) = std::io::copy(&mut stream, &mut stdout) {
            stdout.flush().unwrap();
            println!();
            println!("Error, re-listening. {e:?}");
        }
    }
}
