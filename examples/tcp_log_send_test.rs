use std::{io::Write, net::TcpStream};

// Awesome idea yoinked from fasterthanlime,
// https://youtu.be/Yr9qy9reLCc?t=261
fn main() {
    let mut stream = TcpStream::connect("127.0.0.1:7331").unwrap();
    stream.write("meow?\n".as_bytes()).unwrap();
}
