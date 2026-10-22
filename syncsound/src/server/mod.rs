//! Multithreaded music server.
//!
//! Threads in play:
//! - **main thread**: accepts TCP connections and hands each one to the pool.
//! - **worker pool** (fixed size): reads the request of a connection, answers
//!   it, and moves on. Workers never block on a long-lived client.
//! - **one DJ thread per room**: advances songs and pushes updates
//!   (see [`room`]).

mod room;

use std::{
    collections::{BTreeMap, HashMap},
    error::Error,
    fs::File,
    io::{self, BufReader},
    net::{TcpListener, TcpStream},
    path::Path,
    sync::Arc,
};

use threadpool::ThreadPool;

use crate::{
    catalog::{self, Song},
    protocol::{self, Request, Response},
};
use room::Room;

/// Everything the workers need, built once at startup.
///
/// Nothing in here changes after startup (rooms protect their own mutable
/// state internally), so it can be shared through a plain `Arc` without a
/// `Mutex`. Read-only data never needs a lock.
struct Library {
    rooms: BTreeMap<String, Arc<Room>>,
    songs: HashMap<String, Arc<Song>>,
}

pub fn run(addr: &str, catalog_path: &Path, cache_dir: &Path, threads: usize) -> Result<(), Box<dyn Error>> {
    let songs = catalog::load(catalog_path, cache_dir).map_err(|e| e as Box<dyn Error>)?;
    if songs.is_empty() {
        return Err("the catalog has no playable songs".into());
    }
    let library = Arc::new(build_library(songs));

    let listener = TcpListener::bind(addr)?;
    let pool = ThreadPool::new(threads);
    println!("Listening on {addr} with {threads} workers. Rooms: {:?}", library.rooms.keys().collect::<Vec<_>>());

    for stream in listener.incoming() {
        match stream {
            Ok(stream) => {
                // Cloning an Arc only bumps a counter; the library itself
                // is not copied.
                let library = Arc::clone(&library);
                pool.execute(move || {
                    if let Err(e) = handle_connection(stream, &library) {
                        eprintln!("Connection error: {e}");
                    }
                });
            }
            Err(e) => eprintln!("Error accepting connection: {e}"),
        }
    }
    Ok(())
}

/// Groups songs by genre: one room per genre.
fn build_library(songs: Vec<Song>) -> Library {
    let mut by_genre: BTreeMap<String, Vec<Arc<Song>>> = BTreeMap::new();
    let mut by_id = HashMap::new();
    for song in songs {
        let song = Arc::new(song);
        by_id.insert(song.info.id.clone(), Arc::clone(&song));
        by_genre.entry(song.info.genre.clone()).or_default().push(song);
    }
    let rooms = by_genre
        .into_iter()
        .map(|(genre, songs)| (genre.clone(), Room::start(genre, songs)))
        .collect();
    Library { rooms, songs: by_id }
}

fn handle_connection(stream: TcpStream, library: &Library) -> io::Result<()> {
    let mut reader = BufReader::new(stream.try_clone()?);
    let mut writer = stream;
    let Some(request) = protocol::recv::<Request>(&mut reader)? else {
        return Ok(()); // client connected and left without saying anything
    };

    match request {
        Request::ListRooms => {
            let rooms = library.rooms.values().map(|room| room.info()).collect();
            protocol::send(&mut writer, &Response::Rooms { rooms })
        }

        // Your first version kept each client inside its worker. With 4
        // workers, the 5th listener would wait forever. Here the worker only
        // hands the socket to the room and returns to the pool right away.
        // From then on the room's DJ thread writes to it.
        Request::Join { room } => match library.rooms.get(&room) {
            Some(room) => room.join(writer),
            None => protocol::send(&mut writer, &Response::Error { message: format!("no room named '{room}'") }),
        },

        Request::Fetch { song_id } => match library.songs.get(&song_id) {
            Some(song) => {
                let mut file = File::open(&song.path)?;
                let len = file.metadata()?.len();
                protocol::send(&mut writer, &Response::Audio { song_id, len })?;
                io::copy(&mut file, &mut writer)?;
                Ok(())
            }
            None => protocol::send(&mut writer, &Response::Error { message: format!("no song with id '{song_id}'") }),
        },
    }
}
