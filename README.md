# syncsound

Listen to the same music, at the same moment, with everyone else in the room.

`syncsound` is a multithreaded client–server music player written in Rust over plain TCP. The server runs one endless "radio station" per music genre. Clients join a room and play the current song from the exact point the room has reached, then keep correcting their position so all listeners stay in sync.

It's a learning project about Rust concurrency: thread pools, `Arc`/`Mutex`, scoped threads and channels.

```text
$ syncsound rooms
ROOM            SONGS  LISTENERS  NOW PLAYING
chill               2          0  T. Schürger - SoundHelix Song 8
electronic          3          5  T. Schürger - SoundHelix Song 1

$ syncsound join electronic
Joined room 'electronic'. Press Ctrl+C to leave.

♪ T. Schürger - SoundHelix Song 1
[##----------------------------] 00:34 / 06:12  drift -18ms
```

## Features

- **Genre rooms:** each genre in the catalog becomes a room that plays its songs in a loop, whether or not anyone is listening.
- **Synchronized playback:** a client that joins mid-song starts at the room's current position. Heartbeats every 5 s correct any drift above 200 ms.
- **JSON catalog:** songs come from local files or direct URLs. URLs are downloaded once and cached.
- **Single CLI:** one `syncsound` binary for the server and the client.

## Getting started

You need Rust 1.88 or newer. On Linux, audio playback also needs the ALSA headers (`sudo apt install libasound2-dev` on Debian/Ubuntu).

```sh
git clone https://github.com/alvaromendoooo/syncsound.git
cd syncsound/syncsound
cargo build --release
```

Start the server (the first start downloads the sample songs):

```sh
cargo run --release -- server
```

In other terminals, list rooms and join one:

```sh
cargo run --release -- rooms
cargo run --release -- join electronic
```

## Usage

```text
syncsound server [--addr 127.0.0.1:7878] [--catalog catalog.json] [--cache-dir cache] [--threads 4]
syncsound rooms  [--addr 127.0.0.1:7878]
syncsound join <ROOM> [--addr 127.0.0.1:7878]
```

| Option | Default | Description |
| --- | --- | --- |
| `--addr` | `127.0.0.1:7878` | Address to listen on or connect to. Use `0.0.0.0:7878` on the server to accept clients from your LAN. |
| `--catalog` | `catalog.json` | JSON file describing the songs. |
| `--cache-dir` | `cache` | Where songs given by URL are downloaded (relative to the current directory). |
| `--threads` | `4` | Number of worker threads handling requests. |

## Song catalog

```json
{
  "songs": [
    {
      "id": "helix-1",
      "title": "SoundHelix Song 1",
      "artist": "T. Schürger",
      "genre": "electronic",
      "source": "https://www.soundhelix.com/examples/mp3/SoundHelix-Song-1.mp3"
    },
    {
      "id": "my-song",
      "title": "My Song",
      "artist": "Me",
      "genre": "rock",
      "source": "music/my-song.mp3"
    }
  ]
}
```

- `id` must be unique.
- `genre` decides the room. It's case-insensitive, so `Rock` and `rock` share a room.
- `source` is an `http(s)` URL or a path relative to the catalog file. MP3, FLAC, WAV, Ogg Vorbis and AAC/MP4 are supported.
- Songs that fail to download or decode are skipped with a warning.

The included catalog uses the [SoundHelix](https://www.soundhelix.com) demo tracks. They're all algorithmic electronic music, so the `electronic` and `chill` rooms are placeholders. Swap in your own songs to get real genre rooms.

### Where songs are stored

- **Server:** URL songs are saved in `--cache-dir` and reused on the next start. Delete the folder to free the space; they'll be downloaded again.
- **Client:** nothing is written to disk. Songs are downloaded from the server into memory and freed when the song changes.

## How it works

```text
SERVER                                          CLIENT
main thread: accept() ─► ThreadPool (N)         network thread ──mpsc──► main thread ──► rodio audio thread
                           │ LIST  → reply        (blocks on socket)     (owns Player,
                           │ FETCH → send file                            draws progress bar)
                           │ JOIN  → hand socket to room, return
                           ▼
              Room "electronic": Mutex<RoomState> ◄── DJ thread (one per room)
              Room "chill":      Mutex<RoomState> ◄── DJ thread
```

### Server

- **Worker pool for short jobs only:** workers answer `ListRooms` and `Fetch` requests. For `Join`, a worker hands the socket to the room and returns to the pool immediately. Listeners never occupy a worker, so a 4-thread pool serves any number of them.
- **One DJ thread per room:** it advances the queue in real time and pushes `NowPlaying` and `Sync` messages to every listener.
- **Room state in a `Mutex`:** each room's queue, current song start time and listener sockets are behind a `Mutex`, shared with the workers through an `Arc`. The DJ never sleeps while holding the lock.
- **Slow clients are dropped:** listeners whose sockets fail or block for more than 2 s are removed, so one bad client can't stall a room.
- **Parallel loading:** at startup, songs are downloaded and measured in parallel with `std::thread::scope`.

### Client

- **Two threads and a channel:** a network thread reads messages and passes them over an `mpsc` channel, stamped with the moment they arrived. The main thread owns the audio player, so no locks are needed.
- **Separate download connection:** songs come over a second, short-lived connection, so room updates keep flowing during a download.

### Synchronization

The server never sends a clock time, only a position: "song X, 83,412 ms in". The client adds the time that passed since the message arrived (download and decode included) and seeks there. Every 5 seconds, a `Sync` heartbeat lets the client re-seek if it's more than 200 ms off. Client and server clocks never need to agree.

Network latency isn't compensated. That's negligible on a LAN (under 1 ms) but noticeable over the internet.

### Protocol

Newline-delimited JSON over TCP. Each connection sends one request.

| Request | Response |
| --- | --- |
| `{"type":"list_rooms"}` | `{"type":"rooms","rooms":[...]}` |
| `{"type":"join","room":"rock"}` | `now_playing`, then a stream of `now_playing` / `sync` messages |
| `{"type":"fetch","song_id":"s1"}` | `{"type":"audio","song_id":"s1","len":N}` followed by `N` raw bytes |

Errors come back as `{"type":"error","message":"..."}`.

## Project layout

```text
syncsound/src/
├── main.rs          CLI (clap)
├── protocol.rs      Message types and JSON-lines framing
├── catalog.rs       Catalog loading, downloads, duration probing
├── audio.rs         rodio decoder helpers
├── client.rs        rooms / join commands
└── server/
    ├── mod.rs       Accept loop, worker pool, request handling
    └── room.rs      Room state and DJ thread
```

## Roadmap ideas

- Compensate network latency with NTP-style round-trip measurement.
- Client commands: skip votes, volume, switching rooms without reconnecting.
- Shuffle and per-room playlists in the catalog.
- Importing songs from the [Jamendo API](https://developer.jamendo.com/) (needs a free client ID).
- Graceful server shutdown.

## Built with

[rodio](https://github.com/RustAudio/rodio) for audio, [clap](https://github.com/clap-rs/clap) for the CLI, [serde](https://serde.rs) and serde_json for the protocol, [ureq](https://github.com/algesten/ureq) for downloads, and [threadpool](https://github.com/rust-threadpool/rust-threadpool) for the worker pool.
