//! A room is an endless radio station for one genre.
//!
//! Each room owns a dedicated thread (the "DJ") that walks through the song
//! queue in real time. Clients never ask "what's playing?" repeatedly:
//! the DJ pushes updates to every listener's socket.
//!
//! Shared state and who touches it:
//!
//! ```text
//!   pool thread (JOIN)  ──lock──▶ ┌──────────── Mutex<RoomState> ────────────┐
//!   pool thread (LIST)  ──lock──▶ │ queue · current song + start · listeners │
//!   DJ thread           ──lock──▶ └──────────────────────────────────────────┘
//! ```

use std::{
    collections::VecDeque,
    io::Write,
    net::{SocketAddr, TcpStream},
    sync::{Arc, Mutex},
    thread,
    time::{Duration, Instant},
};

use crate::{
    catalog::Song,
    protocol::{self, Response, RoomInfo},
};

/// How often the DJ sends `Sync` heartbeats while a song plays.
const SYNC_INTERVAL: Duration = Duration::from_secs(5);
/// A listener that can't accept a write within this time gets dropped, so one
/// stalled client can never freeze the whole room.
const WRITE_TIMEOUT: Duration = Duration::from_secs(2);

pub struct Room {
    pub name: String,
    state: Mutex<RoomState>,
}

struct RoomState {
    /// Songs in play order. The front is the current song. After it ends, it
    /// rotates to the back, so the room loops forever.
    queue: VecDeque<Arc<Song>>,
    /// When the current song started, or `None` before the DJ starts.
    started_at: Option<Instant>,
    listeners: Vec<Listener>,
}

struct Listener {
    stream: TcpStream,
    addr: SocketAddr,
}

impl RoomState {
    fn current(&self) -> Option<(&Arc<Song>, Duration)> {
        let started_at = self.started_at?;
        Some((self.queue.front()?, started_at.elapsed()))
    }

    fn now_playing_msg(&self) -> Option<Response> {
        let (song, position) = self.current()?;
        Some(Response::NowPlaying {
            song: song.info.clone(),
            position_ms: position.as_millis() as u64,
        })
    }

    /// Sends `msg` to every listener, dropping the ones whose socket failed
    /// (they disconnected, or are too slow to keep up).
    fn broadcast(&mut self, room: &str, msg: &Response) {
        // Serialize once, write N times.
        let line = protocol::encode(msg);
        self.listeners.retain_mut(|listener| match listener.stream.write_all(&line) {
            Ok(()) => true,
            Err(e) => {
                println!("[{room}] {} left ({e})", listener.addr);
                false
            }
        });
    }
}

impl Room {
    /// Creates the room and starts its DJ thread.
    ///
    /// Returns an `Arc` because the room is shared by the DJ thread and every
    /// connection handler that looks it up. `Arc` (atomic reference count) is
    /// the thread-safe version of `Rc`: the room is freed when the last clone
    /// is dropped, whichever thread that happens on.
    pub fn start(name: String, songs: Vec<Arc<Song>>) -> Arc<Room> {
        assert!(!songs.is_empty(), "a room needs at least one song");
        let room = Arc::new(Room {
            name,
            state: Mutex::new(RoomState {
                queue: songs.into(),
                started_at: None,
                listeners: Vec::new(),
            }),
        });

        let dj = Arc::clone(&room);
        thread::Builder::new()
            .name(format!("dj-{}", room.name))
            .spawn(move || dj.run())
            .expect("failed to spawn DJ thread");
        room
    }

    /// The DJ loop. It runs forever, whether or not anyone is listening,
    /// just like a real radio station.
    fn run(&self) {
        let mut first = true;
        loop {
            // Start the next song: a short critical section.
            let (song, started_at) = {
                let mut state = self.state.lock().unwrap();
                if !first {
                    state.queue.rotate_left(1);
                }
                first = false;

                let started_at = Instant::now();
                state.started_at = Some(started_at);
                let song = Arc::clone(state.queue.front().unwrap());
                let msg = state.now_playing_msg().unwrap();
                state.broadcast(&self.name, &msg);
                (song, started_at)
            }; // <- the MutexGuard drops here. Never hold a lock while sleeping!
            println!("[{}] Now playing: {} - {}", self.name, song.info.artist, song.info.title);

            let ends_at = started_at + Duration::from_millis(song.info.duration_ms);
            loop {
                let now = Instant::now();
                if now >= ends_at {
                    break;
                }
                thread::sleep((ends_at - now).min(SYNC_INTERVAL));
                if Instant::now() < ends_at {
                    let mut state = self.state.lock().unwrap();
                    let msg = Response::Sync {
                        song_id: song.info.id.clone(),
                        position_ms: started_at.elapsed().as_millis() as u64,
                    };
                    state.broadcast(&self.name, &msg);
                }
            }
        }
    }

    /// Subscribes a client and immediately tells it what is playing and where.
    ///
    /// The lock is held while computing the position, sending it, and
    /// registering the listener. Otherwise the DJ could switch songs between
    /// those steps, and the new client would miss the `NowPlaying` for the
    /// next song.
    pub fn join(&self, mut stream: TcpStream) -> std::io::Result<()> {
        let addr = stream.peer_addr()?;
        stream.set_write_timeout(Some(WRITE_TIMEOUT))?;
        stream.set_nodelay(true)?; // small messages: send them right away

        let mut state = self.state.lock().unwrap();
        if let Some(msg) = state.now_playing_msg() {
            protocol::send(&mut stream, &msg)?;
        }
        state.listeners.push(Listener { stream, addr });
        println!("[{}] {addr} joined ({} listening)", self.name, state.listeners.len());
        Ok(())
    }

    pub fn info(&self) -> RoomInfo {
        let state = self.state.lock().unwrap();
        RoomInfo {
            name: self.name.clone(),
            songs: state.queue.len(),
            listeners: state.listeners.len(),
            now_playing: state
                .current()
                .map(|(song, _)| format!("{} - {}", song.info.artist, song.info.title)),
        }
    }
}
