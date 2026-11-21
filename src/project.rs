//! The project model.
//!
//! A project holds the media that was loaded ([`Source`]) and the clips that
//! were cut out of it ([`Clip`]). The clips are ordered and form the timeline
//! that is finally exported. Every change of the clip list can be undone.

use std::path::{Path, PathBuf};

use crate::probe::{self, MediaInfo};
use crate::util::format_fps;

/// Snap a time to the closest key frame when that is wanted.
fn snap_time(keyframes: &[f64], time: f64, snap: bool) -> f64 {
    if snap && !keyframes.is_empty() {
        probe::nearest_keyframe(keyframes, time).unwrap_or(time)
    } else {
        time
    }
}

/// The state of a source file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SourceState {
    /// The metadata is still being read.
    Loading,
    /// The metadata is available.
    Ready,
    /// The file could not be read.
    Failed(String),
}

/// A media file that belongs to the project.
#[derive(Debug, Clone)]
pub struct Source {
    /// Stable identifier of the source.
    pub id: u64,
    /// Path of the file.
    pub path: PathBuf,
    /// Metadata of the file, once it has been read.
    pub info: Option<MediaInfo>,
    /// Timestamps of the key frames.
    pub keyframes: Vec<f64>,
    /// Normalised peak amplitudes of the audio track.
    pub waveform: Vec<f32>,
    /// The state of the source.
    pub state: SourceState,
    /// The start of the range that is marked for cutting.
    pub in_point: Option<f64>,
    /// The end of the range that is marked for cutting.
    pub out_point: Option<f64>,
    /// `true` when the source was removed from the media list.
    ///
    /// The source itself is kept, so that a clip that was cut from it stays
    /// valid and the undo can bring it back.
    pub hidden: bool,
}

impl Source {
    /// The duration of the file in seconds.
    pub fn duration(&self) -> f64 {
        self.info.as_ref().map(|info| info.duration).unwrap_or(0.0)
    }

    /// The name of the file without its directory.
    pub fn name(&self) -> String {
        self.path
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_else(|| self.path.display().to_string())
    }
}

/// A piece of a source file that lives on the timeline.
#[derive(Debug, Clone, PartialEq)]
pub struct Clip {
    /// Stable identifier of the clip.
    pub id: u64,
    /// The source the clip was cut from.
    pub source_id: u64,
    /// Start of the clip inside its source, in seconds.
    pub start: f64,
    /// End of the clip inside its source, in seconds.
    pub end: f64,
}

impl Clip {
    /// The length of the clip in seconds.
    pub fn duration(&self) -> f64 {
        (self.end - self.start).max(0.0)
    }
}

/// Where a clip starts on the timeline.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Placement {
    /// The clip itself.
    pub id: u64,
    /// The start of the clip on the timeline, in seconds.
    pub start: f64,
    /// The length of the clip, in seconds.
    pub duration: f64,
}

/// A state of the timeline that can be restored by the undo.
#[derive(Debug, Clone)]
struct Snapshot {
    /// The clips of the timeline.
    clips: Vec<Clip>,
    /// The sources that were hidden.
    hidden: Vec<u64>,
}

/// The details of clips that cannot be joined with a stream copy.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Incompatibility {
    /// The codecs of the files, separated by commas.
    pub codecs: String,
    /// The resolutions of the files, separated by commas.
    pub resolutions: String,
    /// The frame rates of the files, separated by commas.
    pub rates: String,
    /// The audio formats of the files, separated by commas.
    pub audio: String,
}

/// Everything the editor works with.
#[derive(Debug, Default)]
pub struct Project {
    /// The media files of the project.
    pub sources: Vec<Source>,
    /// The clips of the timeline, in the order they are exported.
    pub clips: Vec<Clip>,
    /// The source that is currently being edited.
    pub selected_source: Option<u64>,
    /// The clips that are selected on the timeline.
    pub selected_clips: Vec<u64>,
    next_id: u64,
    undo: Vec<Snapshot>,
    redo: Vec<Snapshot>,
}

impl Project {
    /// Create an empty project.
    pub fn new() -> Self {
        Self {
            sources: Vec::new(),
            clips: Vec::new(),
            selected_source: None,
            selected_clips: Vec::new(),
            next_id: 1,
            undo: Vec::new(),
            redo: Vec::new(),
        }
    }

    /// Add a file to the project and select it.
    ///
    /// The metadata is read in the background, so the source starts in the
    /// [`SourceState::Loading`] state.
    pub fn add_source(&mut self, path: &Path) -> u64 {
        let id = self.next_id;
        self.next_id += 1;
        self.sources.push(Source {
            id,
            path: path.to_path_buf(),
            info: None,
            keyframes: Vec::new(),
            waveform: Vec::new(),
            state: SourceState::Loading,
            in_point: None,
            out_point: None,
            hidden: false,
        });
        self.selected_source = Some(id);
        id
    }

    /// Store the metadata of a source that finished loading.
    pub fn set_loaded(
        &mut self,
        id: u64,
        info: MediaInfo,
        keyframes: Vec<f64>,
        waveform: Vec<f32>,
    ) {
        if let Some(source) = self.sources.iter_mut().find(|source| source.id == id) {
            source.info = Some(info);
            source.keyframes = keyframes;
            source.waveform = waveform;
            source.state = SourceState::Ready;
        }
    }

    /// Mark a source as broken.
    pub fn set_failed(&mut self, id: u64, message: String) {
        if let Some(source) = self.sources.iter_mut().find(|source| source.id == id) {
            source.state = SourceState::Failed(message);
        }
    }

    /// `true` while at least one source is still being read.
    pub fn is_loading(&self) -> bool {
        self.sources
            .iter()
            .any(|source| source.state == SourceState::Loading)
    }

    /// The source with the given identifier.
    pub fn source(&self, id: u64) -> Option<&Source> {
        self.sources.iter().find(|source| source.id == id)
    }

    /// The sources that are shown in the media list.
    pub fn visible_sources(&self) -> impl Iterator<Item = &Source> {
        self.sources.iter().filter(|source| !source.hidden)
    }

    /// Remove a source and every clip that was cut from it.
    ///
    /// The source itself is only hidden, so that the undo can restore the
    /// clips together with the file they came from.
    pub fn remove_source(&mut self, id: u64) -> bool {
        let Some(index) = self.sources.iter().position(|source| source.id == id) else {
            return false;
        };
        self.remember();
        self.clips.retain(|clip| clip.source_id != id);
        self.clear_selection();
        self.sources[index].hidden = true;
        true
    }

    /// The clip with the given identifier.
    pub fn clip(&self, id: u64) -> Option<&Clip> {
        self.clips.iter().find(|clip| clip.id == id)
    }

    /// The clip that is currently selected.
    ///
    /// When several clips are selected, the first one is the active one.
    pub fn selected_clip(&self) -> Option<&Clip> {
        let id = *self.selected_clips.first()?;
        self.clip(id)
    }

    /// Select a single clip.
    pub fn select_only(&mut self, id: u64) {
        self.selected_clips.clear();
        self.selected_clips.push(id);
    }

    /// Select every clip of the timeline.
    pub fn select_all(&mut self) {
        self.selected_clips = self.clips.iter().map(|clip| clip.id).collect();
    }

    /// Remove the selection.
    pub fn clear_selection(&mut self) {
        self.selected_clips.clear();
    }

    /// `true` when the clip is part of the selection.
    pub fn is_selected(&self, id: u64) -> bool {
        self.selected_clips.contains(&id)
    }

    /// The source that is currently being edited.
    pub fn selected_source(&self) -> Option<&Source> {
        self.selected_source.and_then(|id| self.source(id))
    }

    /// The source a clip was cut from.
    pub fn source_of(&self, clip: &Clip) -> Option<&Source> {
        self.source(clip.source_id)
    }

    /// Move the in marker of a source.
    ///
    /// When `snap` is set, the marker jumps to the closest key frame, which is
    /// what a lossless stream copy needs.
    pub fn set_in_point(&mut self, source_id: u64, time: f64, snap: bool) -> Option<f64> {
        let source = self
            .sources
            .iter_mut()
            .find(|source| source.id == source_id)?;
        let snapped = snap_time(&source.keyframes, time, snap);
        source.in_point = Some(snapped);
        Some(snapped)
    }

    /// Move the out marker of a source.
    pub fn set_out_point(&mut self, source_id: u64, time: f64, snap: bool) -> Option<f64> {
        let source = self
            .sources
            .iter_mut()
            .find(|source| source.id == source_id)?;
        let snapped = snap_time(&source.keyframes, time, snap);
        source.out_point = Some(snapped);
        Some(snapped)
    }

    /// The range that is marked for cutting in a source.
    ///
    /// When only one marker is set, the range reaches to the current play head
    /// position, which is what the user expects while marking.
    pub fn marked_range(&self, source_id: u64, position: f64) -> Option<(f64, f64)> {
        let source = self.source(source_id)?;
        match (source.in_point, source.out_point) {
            (Some(start), Some(end)) => Some((start.min(end), start.max(end))),
            (Some(start), None) => Some((start, position.max(start))),
            (None, Some(end)) => Some((position.min(end), end)),
            (None, None) => None,
        }
    }

    /// Cut a range out of a source and append it to the timeline.
    pub fn cut(&mut self, source_id: u64, start: f64, end: f64) -> Option<Clip> {
        let source = self.source(source_id)?;
        let start = start.clamp(0.0, source.duration());
        let end = end.clamp(0.0, source.duration());
        if end - start <= 0.001 {
            return None;
        }
        self.push_clip(Clip {
            id: self.next_id,
            source_id,
            start: start.min(end),
            end: start.max(end),
        })
    }

    /// Add the whole source file to the timeline.
    pub fn add_whole_source(&mut self, source_id: u64) -> Option<Clip> {
        let source = self.source(source_id)?;
        let duration = source.duration();
        if duration <= 0.0 {
            return None;
        }
        self.push_clip(Clip {
            id: self.next_id,
            source_id,
            start: 0.0,
            end: duration,
        })
    }

    fn push_clip(&mut self, clip: Clip) -> Option<Clip> {
        self.next_id += 1;
        self.remember();
        self.select_only(clip.id);
        self.clips.push(clip.clone());
        Some(clip)
    }

    /// Split a clip into two at the given position inside its source.
    ///
    /// Returns the identifier of the new clip that was created behind the
    /// split position.
    pub fn split_clip(&mut self, clip_id: u64, at: f64) -> Option<u64> {
        let index = self.clips.iter().position(|clip| clip.id == clip_id)?;
        let clip = self.clips[index].clone();
        if at <= clip.start + 0.001 || at >= clip.end - 0.001 {
            return None;
        }
        self.remember();
        let mut second = clip.clone();
        second.id = self.next_id;
        self.next_id += 1;
        second.start = at;
        self.clips[index].end = at;
        self.clips.insert(index + 1, second.clone());
        self.select_only(second.id);
        Some(second.id)
    }

    /// Remove a clip from the timeline.
    pub fn remove_clip(&mut self, clip_id: u64) -> bool {
        let Some(index) = self.clips.iter().position(|clip| clip.id == clip_id) else {
            return false;
        };
        self.remember();
        self.clips.remove(index);
        self.selected_clips.retain(|id| *id != clip_id);
        true
    }

    /// Copy a clip and put the copy directly behind it.
    pub fn duplicate_clip(&mut self, clip_id: u64) -> Option<u64> {
        let index = self.clips.iter().position(|clip| clip.id == clip_id)?;
        let mut copy = self.clips[index].clone();
        copy.id = self.next_id;
        self.next_id += 1;
        self.remember();
        self.clips.insert(index + 1, copy.clone());
        self.select_only(copy.id);
        Some(copy.id)
    }

    /// Move a clip to another position in the timeline.
    ///
    /// The move itself is not remembered, because a drag produces a whole
    /// series of moves that belong to a single step of the history. The
    /// caller takes the snapshot with [`Project::remember_change`] when the
    /// drag begins.
    pub fn move_clip(&mut self, from: usize, to: usize) {
        if from >= self.clips.len() || to >= self.clips.len() || from == to {
            return;
        }
        let clip = self.clips.remove(from);
        self.clips.insert(to, clip);
    }

    /// Remember the current clip list so that it can be restored.
    pub fn remember_change(&mut self) {
        self.remember();
    }

    /// Remove every clip from the timeline.
    pub fn clear_clips(&mut self) {
        if self.clips.is_empty() {
            return;
        }
        self.remember();
        self.clips.clear();
        self.clear_selection();
    }

    /// Undo the last change of the timeline.
    pub fn undo(&mut self) -> bool {
        let Some(previous) = self.undo.pop() else {
            return false;
        };
        self.redo.push(self.snapshot());
        self.clips = previous.clips;
        self.restore_hidden(&previous.hidden);
        self.clear_selection();
        true
    }

    /// Redo the change that was undone last.
    pub fn redo(&mut self) -> bool {
        let Some(next) = self.redo.pop() else {
            return false;
        };
        self.undo.push(self.snapshot());
        self.clips = next.clips;
        self.restore_hidden(&next.hidden);
        self.clear_selection();
        true
    }

    /// The current state of the timeline, for the history.
    fn snapshot(&self) -> Snapshot {
        Snapshot {
            clips: self.clips.clone(),
            hidden: self.hidden_sources(),
        }
    }

    /// The identifiers of the sources that are hidden.
    fn hidden_sources(&self) -> Vec<u64> {
        self.sources
            .iter()
            .filter(|source| source.hidden)
            .map(|source| source.id)
            .collect()
    }

    /// Hide exactly the given sources and show the others.
    fn restore_hidden(&mut self, hidden: &[u64]) {
        for source in &mut self.sources {
            source.hidden = hidden.contains(&source.id);
        }
    }

    /// Remember the current clip list so that it can be restored.
    fn remember(&mut self) {
        self.undo.push(self.snapshot());
        if self.undo.len() > 100 {
            self.undo.remove(0);
        }
        self.redo.clear();
    }

    /// The total length of the timeline in seconds.
    pub fn total_duration(&self) -> f64 {
        self.clips.iter().map(Clip::duration).sum()
    }

    /// Where every clip starts on the timeline.
    pub fn layout(&self) -> Vec<Placement> {
        let mut offset = 0.0;
        let mut placements = Vec::with_capacity(self.clips.len());
        for clip in &self.clips {
            let duration = clip.duration();
            placements.push(Placement {
                id: clip.id,
                start: offset,
                duration,
            });
            offset += duration;
        }
        placements
    }

    /// The position of a clip on the timeline.
    pub fn placement_of(&self, clip_id: u64) -> Option<Placement> {
        self.layout().into_iter().find(|item| item.id == clip_id)
    }

    /// Convert a position on the timeline into a position inside a source.
    ///
    /// Returns the clip and the time inside its source. The last clip is used
    /// for positions behind the end of the timeline.
    pub fn locate(&self, position: f64) -> Option<(u64, f64)> {
        if self.clips.is_empty() {
            return None;
        }
        let mut offset = 0.0;
        for clip in &self.clips {
            let duration = clip.duration();
            if position < offset + duration {
                let inner = (position - offset).clamp(0.0, duration);
                return Some((clip.id, clip.start + inner));
            }
            offset += duration;
        }
        let last = self.clips.last()?;
        Some((last.id, last.end))
    }

    /// Convert a position inside a source into a position on the timeline.
    pub fn timeline_position(&self, clip_id: u64, source_time: f64) -> Option<f64> {
        let placement = self.placement_of(clip_id)?;
        let clip = self.clip(clip_id)?;
        Some((placement.start + (source_time - clip.start)).max(placement.start))
    }

    /// `true` when every source that is used by a clip has the same format.
    pub fn is_compatible(&self) -> bool {
        self.incompatibility().is_none()
    }

    /// Describe why the clips cannot be joined with a plain stream copy.
    ///
    /// The details are returned without a sentence around them, because the
    /// user interface builds the message in the language of the user.
    pub fn incompatibility(&self) -> Option<Incompatibility> {
        let mut reference: Option<Format> = None;
        let mut codecs = Vec::new();
        let mut resolutions = Vec::new();
        let mut rates = Vec::new();
        let mut audio = Vec::new();
        let mut used = 0usize;

        for clip in &self.clips {
            let Some(source) = self.source(clip.source_id) else {
                continue;
            };
            let Some(info) = source.info.as_ref() else {
                continue;
            };
            let Some(video) = info.video.as_ref() else {
                continue;
            };
            used += 1;
            let key = Format {
                video_codec: video.codec.clone(),
                width: video.width,
                height: video.height,
                fps: (video.fps * 100.0).round() as u32,
                audio_codec: info
                    .audio
                    .as_ref()
                    .map(|track| track.codec.clone())
                    .unwrap_or_default(),
                channels: info.audio.as_ref().map(|track| track.channels).unwrap_or(0),
                sample_rate: info
                    .audio
                    .as_ref()
                    .map(|track| track.sample_rate)
                    .unwrap_or(0),
            };
            codecs.push(video.codec.clone());
            resolutions.push(format!("{}×{}", video.width, video.height));
            rates.push(format_fps(video.fps));
            audio.push(match info.audio.as_ref() {
                Some(track) => format!(
                    "{} · {} ch · {} Hz",
                    track.codec, track.channels, track.sample_rate
                ),
                None => "no audio track".to_string(),
            });
            match &reference {
                None => reference = Some(key),
                Some(existing) if *existing == key => {}
                Some(_) => {
                    return Some(Incompatibility {
                        codecs: codecs.join(", "),
                        resolutions: resolutions.join(", "),
                        rates: rates.join(", "),
                        audio: audio.join(", "),
                    });
                }
            }
        }

        if used < 2 {
            return None;
        }
        None
    }
}

/// The format of a file, used to compare the clips of the timeline.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Format {
    /// The codec of the video track.
    video_codec: String,
    /// The width of the video track.
    width: u32,
    /// The height of the video track.
    height: u32,
    /// The frame rate in hundredths of a frame per second.
    fps: u32,
    /// The codec of the audio track.
    audio_codec: String,
    /// The number of channels of the audio track.
    channels: u16,
    /// The sample rate of the audio track.
    sample_rate: u32,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::probe::{AudioInfo, VideoInfo};

    fn source(project: &mut Project, duration: f64) -> u64 {
        let id = project.add_source(Path::new("/tmp/clip.mp4"));
        project.set_loaded(
            id,
            MediaInfo {
                path: PathBuf::from("/tmp/clip.mp4"),
                duration,
                ..MediaInfo::default()
            },
            Vec::new(),
            Vec::new(),
        );
        id
    }

    /// A source with a video track and an audio track of the given format.
    fn source_with_audio(project: &mut Project, codec: &str, channels: u16) -> u64 {
        let id = project.add_source(Path::new("/tmp/clip.mp4"));
        project.set_loaded(
            id,
            MediaInfo {
                path: PathBuf::from("/tmp/clip.mp4"),
                duration: 30.0,
                video: Some(VideoInfo {
                    codec: "h264".to_string(),
                    width: 1920,
                    height: 1080,
                    fps: 25.0,
                    ..VideoInfo::default()
                }),
                audio: Some(AudioInfo {
                    codec: codec.to_string(),
                    channels,
                    sample_rate: 48_000,
                    ..AudioInfo::default()
                }),
                ..MediaInfo::default()
            },
            Vec::new(),
            Vec::new(),
        );
        id
    }

    #[test]
    fn a_cut_creates_a_clip() {
        let mut project = Project::new();
        let id = source(&mut project, 30.0);
        let clip = project.cut(id, 5.0, 12.0).expect("the clip is created");
        assert_eq!(clip.start, 5.0);
        assert_eq!(clip.end, 12.0);
        assert_eq!(clip.duration(), 7.0);
        assert_eq!(project.clips.len(), 1);
    }

    #[test]
    fn a_cut_is_clamped_to_the_source() {
        let mut project = Project::new();
        let id = source(&mut project, 10.0);
        let clip = project.cut(id, 5.0, 99.0).expect("the clip is created");
        assert_eq!(clip.end, 10.0);
    }

    #[test]
    fn an_empty_range_is_rejected() {
        let mut project = Project::new();
        let id = source(&mut project, 10.0);
        assert!(project.cut(id, 5.0, 5.0).is_none());
    }

    #[test]
    fn a_clip_can_be_split() {
        let mut project = Project::new();
        let id = source(&mut project, 30.0);
        let clip = project.cut(id, 0.0, 10.0).expect("the clip is created");
        let second = project.split_clip(clip.id, 4.0).expect("the clip is split");
        assert_eq!(project.clips.len(), 2);
        assert_eq!(project.clips[0].end, 4.0);
        assert_eq!(project.clips[1].start, 4.0);
        assert_eq!(project.clips[1].id, second);
    }

    #[test]
    fn splitting_outside_the_clip_does_nothing() {
        let mut project = Project::new();
        let id = source(&mut project, 30.0);
        let clip = project.cut(id, 0.0, 10.0).expect("the clip is created");
        assert!(project.split_clip(clip.id, 20.0).is_none());
        assert_eq!(project.clips.len(), 1);
    }

    #[test]
    fn changes_can_be_undone_and_redone() {
        let mut project = Project::new();
        let id = source(&mut project, 30.0);
        let clip = project.cut(id, 0.0, 10.0).expect("the clip is created");
        project.remove_clip(clip.id);
        assert!(project.clips.is_empty());

        assert!(project.undo());
        assert_eq!(project.clips.len(), 1);
        assert!(project.redo());
        assert!(project.clips.is_empty());
    }

    #[test]
    fn a_whole_drag_is_a_single_step_of_the_history() {
        let mut project = Project::new();
        let id = source(&mut project, 30.0);
        let first = project.cut(id, 0.0, 10.0).expect("the clip is created");
        let second = project.cut(id, 10.0, 20.0).expect("the clip is created");

        project.remember_change();
        project.move_clip(0, 1);
        assert_eq!(project.clips[0].id, second.id);

        assert!(project.undo());
        assert_eq!(project.clips[0].id, first.id);
        assert_eq!(project.clips[1].id, second.id);
    }

    #[test]
    fn the_layout_places_the_clips_one_behind_the_other() {
        let mut project = Project::new();
        let id = source(&mut project, 30.0);
        project.cut(id, 0.0, 10.0).expect("the clip is created");
        project.cut(id, 10.0, 15.0).expect("the clip is created");
        let layout = project.layout();
        assert_eq!(layout.len(), 2);
        assert_eq!(layout[0].start, 0.0);
        assert_eq!(layout[1].start, 10.0);
        assert_eq!(project.total_duration(), 15.0);
    }

    #[test]
    fn a_position_finds_the_clip_it_belongs_to() {
        let mut project = Project::new();
        let id = source(&mut project, 30.0);
        let first = project.cut(id, 0.0, 10.0).expect("the clip is created");
        let second = project.cut(id, 20.0, 30.0).expect("the clip is created");

        let (clip_id, time) = project.locate(5.0).expect("a clip is found");
        assert_eq!(clip_id, first.id);
        assert_eq!(time, 5.0);

        let (clip_id, time) = project.locate(15.0).expect("a clip is found");
        assert_eq!(clip_id, second.id);
        assert_eq!(time, 25.0);

        let position = project
            .timeline_position(second.id, 25.0)
            .expect("the position is known");
        assert_eq!(position, 15.0);
    }

    #[test]
    fn a_whole_source_becomes_one_clip() {
        let mut project = Project::new();
        let id = source(&mut project, 42.0);
        let clip = project.add_whole_source(id).expect("the clip is created");
        assert_eq!(clip.start, 0.0);
        assert_eq!(clip.end, 42.0);
    }

    #[test]
    fn removing_a_source_hides_it_and_takes_its_clips_away() {
        let mut project = Project::new();
        let id = source(&mut project, 30.0);
        project.cut(id, 0.0, 10.0).expect("the clip is created");
        assert!(project.remove_source(id));
        assert!(project.clips.is_empty());
        assert_eq!(project.visible_sources().count(), 0);
        assert_eq!(project.sources.len(), 1);
    }

    #[test]
    fn removing_a_source_can_be_undone() {
        let mut project = Project::new();
        let id = source(&mut project, 30.0);
        project.cut(id, 0.0, 10.0).expect("the clip is created");
        project.remove_source(id);

        assert!(project.undo());
        assert_eq!(project.clips.len(), 1);
        assert_eq!(project.visible_sources().count(), 1);

        assert!(project.redo());
        assert!(project.clips.is_empty());
        assert_eq!(project.visible_sources().count(), 0);
    }

    #[test]
    fn a_different_audio_format_is_reported() {
        let mut project = Project::new();
        let first = source_with_audio(&mut project, "aac", 2);
        let second = source_with_audio(&mut project, "mp3", 2);
        project.cut(first, 0.0, 10.0).expect("the clip is created");
        project.cut(second, 0.0, 10.0).expect("the clip is created");

        let details = project.incompatibility().expect("the formats differ");
        assert!(details.audio.contains("aac"));
        assert!(details.audio.contains("mp3"));
        assert!(!project.is_compatible());
    }

    #[test]
    fn the_same_audio_format_is_accepted() {
        let mut project = Project::new();
        let first = source_with_audio(&mut project, "aac", 2);
        let second = source_with_audio(&mut project, "aac", 2);
        project.cut(first, 0.0, 10.0).expect("the clip is created");
        project.cut(second, 0.0, 10.0).expect("the clip is created");

        assert!(project.incompatibility().is_none());
        assert!(project.is_compatible());
    }
}
