//! Client side: list rooms, or join one and play in sync.
//!
//! When joined, the client runs two threads of its own (plus rodio's audio
//! thread):
//!
//! ```text
//!  network thread ──mpsc::Sender<Event>──▶ main thread ──append/seek──▶ rodio audio thread
//!  (blocks reading the socket)             (owns the Player, draws UI)    (feeds the sound card)
//! ```
//!
//! The network thread stamps every message with the `Instant` it arrived.
//! Any delay after that (queueing, downloading, decoding) can then be
//! measured and added to the position the server reported.

use std::{
    error::Error,
    io::{self, BufReader, Cursor, Read, Write},
    net::TcpStream,
    sync::mpsc::{self, RecvTimeoutError, Sender},
    thread,
    time::{Duration, Instant},
};

use rodio::{DeviceSinkBuilder, Player};

use crate::{
    audio,
    protocol::{self, Request, Response, SongInfo},
};

/// If playback is further than this from where the room is, re-seek. Smaller
/// values keep listeners tighter but cause more audible seek glitches.
const MAX_DRIFT: Duration = Duration::from_millis(200);
const UI_REFRESH: Duration = Duration::from_millis(500);

enum Event {
    Message { msg: Response, received_at: Instant },
    Disconnected(String),
}

struct NowPlaying {
    song: SongInfo,
    /// Dropping a `Player` stops its sound, so it lives exactly as long as
    /// the song is current.
    player: Player,
    last_drift_ms: Option<i64>,
}

pub fn list_rooms(addr: &str) -> Result<(), Box<dyn Error>> {
    let stream = TcpStream::connect(addr)?;
    protocol::send(&mut &stream, &Request::ListRooms)?;
    match protocol::recv(&mut BufReader::new(&stream))? {
        Some(Response::Rooms { rooms }) => {
            println!("{:<14} {:>6} {:>10}  NOW PLAYING", "ROOM", "SONGS", "LISTENERS");
            for room in rooms {
                println!(
                    "{:<14} {:>6} {:>10}  {}",
                    room.name,
                    room.songs,
                    room.listeners,
                    room.now_playing.unwrap_or_default()
                );
            }
            Ok(())
        }
        other => Err(format!("unexpected reply: {other:?}").into()),
    }
}

pub fn join(addr: &str, room: &str) -> Result<(), Box<dyn Error>> {
    let mut stream = TcpStream::connect(addr)?;
    protocol::send(&mut stream, &Request::Join { room: room.to_string() })?;
    println!("Joined room '{room}'. Press Ctrl+C to leave.");

    // `mpsc` = multi-producer, single-consumer channel. The `Sender` moves
    // into the network thread and the `Receiver` stays here. The channel is
    // the only thing the two threads share, so no Mutex is needed.
    let (tx, rx) = mpsc::channel();
    thread::Builder::new()
        .name("network".into())
        .spawn(move || network_loop(stream, tx))?;

    // Opens the sound card. Audio stops when this handle drops, so it has to
    // live for the whole function.
    let device = DeviceSinkBuilder::open_default_sink()?;
    let mut current: Option<NowPlaying> = None;

    loop {
        // `recv_timeout` instead of `recv`: wake up at least every
        // UI_REFRESH to redraw the progress bar even with no new messages.
        match rx.recv_timeout(UI_REFRESH) {
            Ok(Event::Message { msg, received_at }) => match msg {
                Response::NowPlaying { song, position_ms } => {
                    // Drop the previous player first so the old song stops now.
                    drop(current.take());
                    println!("\n♪ {} - {}", song.artist, song.title);
                    let player = Player::connect_new(device.mixer());
                    let bytes = fetch(addr, &song.id)?;
                    let len = bytes.len() as u64;
                    player.append(audio::decoder(Cursor::new(bytes), len)?);
                    // Measure the delay right before seeking: it includes the
                    // download and decode time.
                    let target = Duration::from_millis(position_ms) + received_at.elapsed();
                    if let Err(e) = player.try_seek(target) {
                        eprintln!("\nCould not seek, playing from the start: {e}");
                    }
                    current = Some(NowPlaying { song, player, last_drift_ms: None });
                }
                Response::Sync { song_id, position_ms } => {
                    if let Some(now) = current.as_mut()
                        && now.song.id == song_id
                    {
                        resync(now, Duration::from_millis(position_ms), received_at);
                    }
                }
                Response::Error { message } => return Err(message.into()),
                other => eprintln!("\nUnexpected message: {other:?}"),
            },
            Ok(Event::Disconnected(reason)) => return Err(format!("disconnected: {reason}").into()),
            Err(RecvTimeoutError::Timeout) => {}
            Err(RecvTimeoutError::Disconnected) => return Err("network thread stopped".into()),
        }

        if let Some(now) = &current {
            draw_status(now);
        }
    }
}

/// Reads messages from the server forever and forwards them to the main thread.
fn network_loop(stream: TcpStream, tx: Sender<Event>) {
    let mut reader = BufReader::new(stream);
    loop {
        let event = match protocol::recv::<Response>(&mut reader) {
            Ok(Some(msg)) => Event::Message { msg, received_at: Instant::now() },
            Ok(None) => Event::Disconnected("server closed the connection".into()),
            Err(e) => Event::Disconnected(e.to_string()),
        };
        let stop = matches!(event, Event::Disconnected(_));
        // `send` only fails if the receiver was dropped (main thread done).
        if tx.send(event).is_err() || stop {
            return;
        }
    }
}

/// Compares where we are with where the room is, and seeks if too far off.
fn resync(now: &mut NowPlaying, server_pos: Duration, received_at: Instant) {
    let expected = server_pos + received_at.elapsed();
    let actual = now.player.get_pos();
    let drift_ms = actual.as_millis() as i64 - expected.as_millis() as i64;
    now.last_drift_ms = Some(drift_ms);
    if drift_ms.unsigned_abs() > MAX_DRIFT.as_millis() as u64 {
        let _ = now.player.try_seek(server_pos + received_at.elapsed());
    }
}

/// Downloads a song over a separate, short-lived connection, so the
/// long-lived room connection keeps delivering messages meanwhile.
fn fetch(addr: &str, song_id: &str) -> io::Result<Vec<u8>> {
    let mut stream = TcpStream::connect(addr)?;
    protocol::send(&mut stream, &Request::Fetch { song_id: song_id.to_string() })?;
    let mut reader = BufReader::new(stream);
    match protocol::recv(&mut reader)? {
        Some(Response::Audio { len, .. }) => {
            // Read the raw bytes through the SAME BufReader. It may already
            // have buffered the first audio bytes along with the JSON line.
            let mut bytes = vec![0; len as usize];
            reader.read_exact(&mut bytes)?;
            Ok(bytes)
        }
        Some(Response::Error { message }) => Err(io::Error::other(message)),
        other => Err(io::Error::other(format!("unexpected reply: {other:?}"))),
    }
}

fn draw_status(now: &NowPlaying) {
    const WIDTH: u64 = 30;
    let pos = now.player.get_pos().as_millis() as u64;
    let total = now.song.duration_ms.max(1);
    let filled = (pos.min(total) * WIDTH / total) as usize;
    let drift = now.last_drift_ms.map(|d| format!("drift {d:+}ms")).unwrap_or_default();
    print!(
        "\r[{}{}] {} / {}  {drift:<16}",
        "#".repeat(filled),
        "-".repeat(WIDTH as usize - filled),
        mmss(pos),
        mmss(total)
    );
    let _ = io::stdout().flush();
}

fn mmss(ms: u64) -> String {
    format!("{:02}:{:02}", ms / 60_000, ms / 1000 % 60)
}
