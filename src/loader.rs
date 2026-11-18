//! Asynchronous loading of media metadata.
//!
//! Probing a file, collecting its key frames and rendering its waveform takes
//! noticeably longer than a frame of the user interface. The work therefore
//! happens on a blocking thread and the result is handed back through a
//! channel that the user interface polls once per frame.

use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, Receiver};

use anyhow::Result;

use crate::ffmpeg_tools::Tools;
use crate::probe::{self, MediaInfo};

/// Everything that is known about a loaded media file.
#[derive(Debug, Clone)]
pub struct LoadedMedia {
    /// The file that was loaded.
    pub path: PathBuf,
    /// Container and stream metadata.
    pub info: MediaInfo,
    /// Timestamps of all key frames.
    pub keyframes: Vec<f64>,
    /// Normalised peak amplitudes of the audio track.
    pub waveform: Vec<f32>,
}

/// Number of waveform buckets per second of audio.
const WAVEFORM_BUCKETS_PER_SECOND: usize = 20;
/// Upper limit for the number of waveform buckets.
const WAVEFORM_MAX_BUCKETS: usize = 20_000;

/// Load `path` in the background and return a channel with the result.
pub fn load_async(tools: Tools, path: PathBuf) -> Receiver<Result<LoadedMedia>> {
    let (sender, receiver) = mpsc::channel();
    crate::jobs::runtime().spawn_blocking(move || {
        let result = load_blocking(&tools, &path);
        let _ = sender.send(result);
    });
    receiver
}

/// Load `path` synchronously.
pub fn load_blocking(tools: &Tools, path: &Path) -> Result<LoadedMedia> {
    let info = probe::probe(&tools.ffprobe, path)?;
    let keyframes = probe::keyframes(&tools.ffprobe, path)?;
    let buckets = ((info.duration * WAVEFORM_BUCKETS_PER_SECOND as f64).ceil() as usize)
        .clamp(1, WAVEFORM_MAX_BUCKETS);
    let waveform = if info.has_audio() {
        probe::waveform(&tools.ffmpeg, path, info.duration, buckets)?
    } else {
        Vec::new()
    };
    Ok(LoadedMedia {
        path: path.to_path_buf(),
        info,
        keyframes,
        waveform,
    })
}

/// Probe `path` in the background and return a channel with the result.
pub fn probe_async(tools: Tools, path: PathBuf) -> Receiver<Result<MediaInfo>> {
    let (sender, receiver) = mpsc::channel();
    crate::jobs::runtime().spawn_blocking(move || {
        let result = probe::probe(&tools.ffprobe, &path);
        let _ = sender.send(result);
    });
    receiver
}
