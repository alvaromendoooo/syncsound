//! Wire protocol shared by server and client.
//!
//! Every message is a single line of JSON terminated by `\n` ("JSON Lines").
//! The only exception is [`Response::Audio`]: its JSON line is followed by
//! exactly `len` raw bytes containing the encoded audio file.

use std::io::{self, BufRead, Write};

use serde::{Deserialize, Serialize, de::DeserializeOwned};

/// Messages sent from client to server. Each connection sends exactly one.
#[derive(Debug, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Request {
    /// Ask which rooms exist.
    ListRooms,
    /// Subscribe to a room. The connection then stays open and the server
    /// keeps pushing `NowPlaying` / `Sync` messages through it.
    Join { room: String },
    /// Download the audio file of a song.
    Fetch { song_id: String },
}

/// Messages sent from server to client.
#[derive(Debug, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Response {
    Rooms { rooms: Vec<RoomInfo> },
    /// A song is playing. `position_ms` is how far into the song the room was
    /// at the instant the server wrote this message.
    NowPlaying { song: SongInfo, position_ms: u64 },
    /// Periodic heartbeat so clients can correct drift.
    Sync { song_id: String, position_ms: u64 },
    /// Header for an audio download; followed by `len` raw bytes.
    Audio { song_id: String, len: u64 },
    Error { message: String },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SongInfo {
    pub id: String,
    pub title: String,
    pub artist: String,
    pub genre: String,
    pub duration_ms: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RoomInfo {
    pub name: String,
    pub songs: usize,
    pub listeners: usize,
    pub now_playing: Option<String>,
}

/// Serializes `msg` into a single newline-terminated line.
pub fn encode<T: Serialize>(msg: &T) -> Vec<u8> {
    let mut line = serde_json::to_vec(msg).expect("protocol types always serialize");
    line.push(b'\n');
    line
}

pub fn send<T: Serialize>(writer: &mut impl Write, msg: &T) -> io::Result<()> {
    writer.write_all(&encode(msg))
}

/// Reads one message. Returns `Ok(None)` when the peer closed the connection.
pub fn recv<T: DeserializeOwned>(reader: &mut impl BufRead) -> io::Result<Option<T>> {
    let mut line = String::new();
    if reader.read_line(&mut line)? == 0 {
        return Ok(None);
    }
    serde_json::from_str(&line)
        .map(Some)
        .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))
}

#[cfg(test)]
mod tests {
    use std::io::{BufReader, Read};

    use super::*;

    #[test]
    fn raw_bytes_after_header_survive_buffering() {
        let audio = b"\x00\x01binary\npayload";
        let mut wire = Vec::new();
        send(&mut wire, &Response::Audio { song_id: "s1".into(), len: audio.len() as u64 }).unwrap();
        wire.extend_from_slice(audio);
        send(&mut wire, &Response::Error { message: "next".into() }).unwrap();

        // The BufReader slurps header and payload in one read; the payload
        // must still be readable through it.
        let mut reader = BufReader::new(wire.as_slice());
        let Some(Response::Audio { len, .. }) = recv(&mut reader).unwrap() else { panic!() };
        let mut payload = vec![0; len as usize];
        reader.read_exact(&mut payload).unwrap();
        assert_eq!(payload, audio);
        assert!(matches!(recv(&mut reader).unwrap(), Some(Response::Error { .. })));
        assert!(recv::<Response>(&mut reader).unwrap().is_none());
    }
}
