//! Creates a simple client
//! 
//! It connects to server and receives data of the music being played

use std::{io::{Read, Write}, net::TcpStream};

pub fn create_server_conn() {
    match TcpStream::connect("127.0.0.1:7878") {
        Ok(mut stream) =>  {
            println!("Client successfully connected to server");
            let msg = b"Hello server\n"; // Don't forget the \n
            stream.write(msg).unwrap();
            println!("Sent message");
            let mut buffer = [0; 512];
            match stream.read(&mut buffer) {
                Ok(n) => {
                    println!("Received: {}", String::from_utf8_lossy(&buffer[..n]));
                }
                Err(e) => {
                    println!("Failed to receive data: {}", e);
                }
            }
        },
        Err(e) => println!("Failed to connect: {}", e)
    }
}

pub fn main() {
    println!("Starting client side");

    create_server_conn();
}