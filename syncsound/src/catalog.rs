//! Song catalog loaded from a JSON file.
//!
//! ```json
//! { "songs": [
//!     { "id": "s1", "title": "...", "artist": "...", "genre": "rock",
//!       "source": "music/s1.mp3" },
//!     { "id": "s2", "title": "...", "artist": "...", "genre": "jazz",
//!       "source": "https://example.com/s2.mp3" }
//! ] }
//! ```
//!
//! `source` is either a path (relative to the catalog file) or an http(s) URL.
//! URLs are downloaded once into a cache directory, so after loading every
//! song is just a local file.

use std::{
    error::Error,
    fs::{self, File},
    io::BufReader,
    path::{Path, PathBuf},
    thread,
};

use serde::Deserialize;

use crate::{audio, protocol::SongInfo};

type BoxError = Box<dyn Error + Send + Sync>;

const MAX_DOWNLOAD_BYTES: u64 = 100 * 1024 * 1024;

#[derive(Deserialize)]
struct CatalogFile {
    songs: Vec<SongEntry>,
}

#[derive(Deserialize)]
struct SongEntry {
    id: String,
    title: String,
    artist: String,
    genre: String,
    source: String,
}

/// A song ready to be served: its metadata plus where its audio lives on disk.
#[derive(Debug)]
pub struct Song {
    pub info: SongInfo,
    pub path: PathBuf,
}

/// Loads the catalog, downloading and measuring every song in parallel.
///
/// Songs that fail to load are reported and skipped instead of aborting the
/// whole server.
pub fn load(catalog_path: &Path, cache_dir: &Path) -> Result<Vec<Song>, BoxError> {
    let file: CatalogFile = serde_json::from_str(&fs::read_to_string(catalog_path)?)?;
    let base_dir = catalog_path.parent().unwrap_or(Path::new("."));
    fs::create_dir_all(cache_dir)?;

    // `thread::scope` lets the spawned threads borrow `base_dir`, `cache_dir`
    // and each `entry` from this stack frame. That is impossible with plain
    // `thread::spawn`, which requires `'static` data. The scope guarantees
    // every thread is joined before it returns, so the borrows stay valid.
    let results: Vec<(String, Result<Song, BoxError>)> = thread::scope(|scope| {
        let handles: Vec<_> = file
            .songs
            .into_iter()
            .map(|entry| {
                scope.spawn(move || {
                    let id = entry.id.clone();
                    (id, prepare(entry, base_dir, cache_dir))
                })
            })
            .collect();
        // Join in spawn order so the catalog order is preserved.
        handles.into_iter().map(|h| h.join().expect("loader thread panicked")).collect()
    });

    let mut songs = Vec::new();
    for (id, result) in results {
        match result {
            Ok(song) => songs.push(song),
            Err(e) => eprintln!("Skipping song '{id}': {e}"),
        }
    }
    Ok(songs)
}

fn prepare(entry: SongEntry, base_dir: &Path, cache_dir: &Path) -> Result<Song, BoxError> {
    let path = if entry.source.starts_with("http://") || entry.source.starts_with("https://") {
        download(&entry.id, &entry.source, cache_dir)?
    } else {
        base_dir.join(&entry.source)
    };

    let file = File::open(&path).map_err(|e| format!("{}: {e}", path.display()))?;
    let len = file.metadata()?.len();
    let duration = audio::duration(BufReader::new(file), len)?;

    println!("Loaded '{}' ({}s)", entry.title, duration.as_secs());
    Ok(Song {
        info: SongInfo {
            id: entry.id,
            title: entry.title,
            artist: entry.artist,
            genre: entry.genre.to_lowercase(),
            duration_ms: duration.as_millis() as u64,
        },
        path,
    })
}

fn download(id: &str, url: &str, cache_dir: &Path) -> Result<PathBuf, BoxError> {
    let extension = Path::new(url)
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("audio");
    let path = cache_dir.join(format!("{id}.{extension}"));
    if path.exists() {
        return Ok(path);
    }

    println!("Downloading {url}");
    // Trust the OS certificate store (like a browser does) instead of the
    // Mozilla roots bundled with ureq.
    let agent: ureq::Agent = ureq::Agent::config_builder()
        .tls_config(
            ureq::tls::TlsConfig::builder()
                .root_certs(ureq::tls::RootCerts::PlatformVerifier)
                .build(),
        )
        .build()
        .into();
    let bytes = agent
        .get(url)
        .call()?
        .body_mut()
        .with_config()
        .limit(MAX_DOWNLOAD_BYTES)
        .read_to_vec()?;
    // Write to a temp file and rename, so an interrupted download never
    // leaves a truncated file that looks valid on the next start.
    let tmp = path.with_extension("part");
    fs::write(&tmp, bytes)?;
    fs::rename(&tmp, &path)?;
    Ok(path)
}
