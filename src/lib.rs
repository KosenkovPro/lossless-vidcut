//! Lossless VidCut - a fast desktop tool for cutting and joining videos
//! without re-encoding them.
//!
//! The crate is split into small, focused modules. [`app`] contains the
//! application shell, [`splitter`] and [`joiner`] the two tabs, [`player`]
//! the playback engine and [`processor`] the FFmpeg command builders.

pub mod app;
pub mod ffmpeg_tools;
pub mod jobs;
pub mod joiner;
pub mod loader;
pub mod player;
pub mod probe;
pub mod processor;
pub mod settings;
pub mod splitter;
pub mod timeline;
pub mod toast;
pub mod util;
