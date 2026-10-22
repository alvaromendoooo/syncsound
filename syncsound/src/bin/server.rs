//! Creates a single-threaded web server.
//! 
//! Now it only handles the setup and communications.
//! In future it'll run songs.
use std::{io::{BufReader, prelude::*}, net::{TcpListener, TcpStream}};

use threadpool::ThreadPool;


pub fn create_server() {
    let listener = TcpListener::bind("127.0.0.1:7878").unwrap();
    let pool = ThreadPool::new(4);

    for stream in listener.incoming() {
        match stream {
            Ok(stream) => {
                pool.execute(move || {
                    handle_connections(stream);
                });
            },
            Err(e) => {
                println!("Error accepting connexion: {}", e);
            }
        }
    }
}

pub fn handle_connections(mut stream: TcpStream) {
    let peer_address = stream.peer_addr().unwrap();
    let mut buf_reader = BufReader::new(&stream);
    let mut request = String::new();

    // Read content received with end-end communication
    match buf_reader.read_line(&mut request) {
        Ok(_) => {
            println!("[{}] Message received: {}", peer_address, request);

            let response = "Hello client, welcome to music server!";
            stream.write_all(response.as_bytes()).unwrap();
        },
        Err(e) => {
            println!("Error reading client's message: {}", e);
        }
    }
}

pub fn main() {
    println!("Starting server side");
    create_server();
}