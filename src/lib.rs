//! Lossless VidCut - a fast desktop tool for cutting and joining videos
//! without re-encoding them.
//!
//! The crate is split into small, focused modules. [`app`] contains the
//! application shell, [`editor`] the unified editor with the preview, the
//! source timeline and the sequence track, [`project`] the model of the media
//! and the clips, [`player`] the playback engine and [`processor`] the FFmpeg
//! command builders.

pub mod app;
pub mod editor;
pub mod ffmpeg_tools;
pub mod i18n;
pub mod jobs;
pub mod loader;
pub mod player;
pub mod probe;
pub mod processor;
pub mod project;
pub mod sequence;
pub mod settings;
pub mod shortcut;
pub mod sys;
pub mod timeline;
pub mod toast;
pub mod util;
