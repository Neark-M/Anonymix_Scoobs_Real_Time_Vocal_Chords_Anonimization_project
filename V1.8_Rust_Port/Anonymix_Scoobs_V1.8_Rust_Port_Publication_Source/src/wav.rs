//! The fixed PCM16 RIFF header and two-part write from Wav.cpp.
//! take.rs connects the export worker; the harness supplies its executable-relative path.
use std::io::{self, Write};

/// Header fields use explicit little-endian encoding, matching Windows x64's
/// original 44-byte WAVHeader layout without relying on Rust struct padding.
pub fn pcm16_wav_header(bytes: usize, channels: u16, sample_rate: u32) -> io::Result<[u8; 44]> {
    if bytes == 0
        || channels == 0
        || bytes % (2 * channels as usize) != 0
        || bytes > u32::MAX as usize - 36
    {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "invalid PCM16 RIFF size or channels",
        ));
    }
    let mut header = [0; 44];
    header[0..4].copy_from_slice(b"RIFF");
    header[4..8].copy_from_slice(&(bytes as u32 + 36).to_le_bytes());
    header[8..12].copy_from_slice(b"WAVE");
    header[12..16].copy_from_slice(b"fmt ");
    header[16..20].copy_from_slice(&16_u32.to_le_bytes());
    header[20..22].copy_from_slice(&1_u16.to_le_bytes());
    header[22..24].copy_from_slice(&channels.to_le_bytes());
    header[24..28].copy_from_slice(&sample_rate.to_le_bytes());
    header[28..32].copy_from_slice(&(sample_rate * channels as u32 * 16 / 8).to_le_bytes());
    header[32..34].copy_from_slice(&(channels * 16 / 8).to_le_bytes());
    header[34..36].copy_from_slice(&16_u16.to_le_bytes());
    header[36..40].copy_from_slice(b"data");
    header[40..44].copy_from_slice(&(bytes as u32).to_le_bytes());
    Ok(header)
}

/// The worker supplies the complete processed snapshot, just as in V1.8.
pub fn write_pcm16_wav(
    writer: &mut impl Write,
    bytes: &[u8],
    channels: u16,
    sample_rate: u32,
) -> io::Result<()> {
    let header = pcm16_wav_header(bytes.len(), channels, sample_rate)?;
    writer.write_all(&header)?;
    writer.write_all(bytes)
}

/// Same local YYYYMMDD_HHMMSS suffix as Wav.cpp. GetLocalTime avoids adding a
/// calendar dependency for this Windows-only harness; it never runs in audio.
#[cfg(all(windows, target_pointer_width = "64"))]
pub fn local_wav_filename(prefix: &str) -> String {
    #[repr(C)]
    #[derive(Default)]
    struct SystemTime {
        year: u16,
        month: u16,
        weekday: u16,
        day: u16,
        hour: u16,
        minute: u16,
        second: u16,
        milliseconds: u16,
    }
    #[link(name = "kernel32")]
    extern "system" {
        fn GetLocalTime(time: *mut SystemTime);
    }
    let mut time = SystemTime::default();
    // SAFETY: SYSTEMTIME is eight u16 fields, initialized and writable.
    unsafe {
        GetLocalTime(&mut time);
    }
    format!(
        "{prefix}_{:04}{:02}{:02}_{:02}{:02}{:02}.wav",
        time.year, time.month, time.day, time.hour, time.minute, time.second
    )
}
