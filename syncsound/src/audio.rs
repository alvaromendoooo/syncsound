//! Thin wrapper around rodio's decoder, shared by server and client.

use std::{
    io::{Read, Seek},
    time::Duration,
};

use rodio::{Decoder, Source, decoder::DecoderError};

/// Builds a seekable decoder. Passing the byte length lets symphonia compute
/// the duration of formats with no length header (most MP3s) and seek in them.
pub fn decoder<R>(data: R, byte_len: u64) -> Result<Decoder<R>, DecoderError>
where
    R: Read + Seek + Send + Sync + 'static,
{
    Decoder::builder()
        .with_data(data)
        .with_byte_len(byte_len)
        .with_seekable(true)
        .build()
}

/// Returns the duration of an encoded audio stream.
///
/// Uses the container metadata when available and otherwise decodes the
/// whole stream and counts samples. That is slow, but runs once per song.
pub fn duration<R>(data: R, byte_len: u64) -> Result<Duration, DecoderError>
where
    R: Read + Seek + Send + Sync + 'static,
{
    let decoder = decoder(data, byte_len)?;
    if let Some(total) = decoder.total_duration() {
        return Ok(total);
    }
    let samples_per_sec = decoder.sample_rate().get() as f64 * decoder.channels().get() as f64;
    let samples = decoder.count() as f64;
    Ok(Duration::from_secs_f64(samples / samples_per_sec))
}
