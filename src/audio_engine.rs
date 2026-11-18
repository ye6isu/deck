use crate::dsp::StudioReverb;
use crossbeam_channel::{bounded, unbounded, Receiver, Sender};
use rodio::source::{SeekError, UniformSourceIterator};
use rodio::{ChannelCount, Decoder, DeviceSinkBuilder, Player, SampleRate, Source};
use std::fs::File;
use std::path::{Path, PathBuf};
use std::sync::{
    atomic::{AtomicBool, AtomicU32, AtomicU64, AtomicUsize, Ordering},
    Arc, OnceLock,
};
use std::time::Duration;

pub enum AudioCommand {
    Load {
        path: PathBuf,
        autoplay: bool,
        start_progress: f32,
    },
    Toggle,
    Seek(f32),
    SetSpeed(f32),
    SetReverb(f32),
    SetVolume(f32),
}

#[derive(Clone, Default)]
pub struct AudioStatus {
    pub revision: u64,
    pub is_playing: bool,
    pub position_secs: f64,
    pub duration_secs: f64,
    pub progress: f32,
    pub finished: bool,
    pub error: Option<String>,
}

pub(crate) struct Request {
    pub(crate) revision: u64,
    pub(crate) command: AudioCommand,
}

pub struct AudioEngine {
    cmd_tx: Sender<Request>,
    status_rx: Receiver<AudioStatus>,
    revision: AtomicU64,
}

struct Effects {
    volume: AtomicU32,
    reverb: AtomicU32,
}

impl Default for Effects {
    fn default() -> Self {
        Self {
            volume: AtomicU32::new(0.85f32.to_bits()),
            reverb: AtomicU32::new(0.0f32.to_bits()),
        }
    }
}

const DECODE_BLOCK: usize = 16384; // Interleaved stereo samples, about 186 ms at 44.1 kHz.

struct SampleStore {
    blocks: Vec<OnceLock<Box<[f32]>>>,
    expected: usize,
    written: AtomicUsize,
    finished: AtomicBool,
    canceled: AtomicBool,
}

impl SampleStore {
    fn new(expected: usize) -> Self {
        Self {
            blocks: (0..expected.div_ceil(DECODE_BLOCK))
                .map(|_| OnceLock::new())
                .collect(),
            expected,
            written: AtomicUsize::new(0),
            finished: AtomicBool::new(false),
            canceled: AtomicBool::new(false),
        }
    }
    fn complete(samples: Vec<f32>) -> Self {
        let store = Self::new(samples.len());
        for (i, block) in samples.chunks(DECODE_BLOCK).enumerate() {
            store.publish(i, block.to_vec());
        }
        store.finished.store(true, Ordering::Release);
        store
    }
    fn publish(&self, index: usize, samples: Vec<f32>) {
        let len = samples.len();
        if let Some(block) = self.blocks.get(index) {
            let _ = block.set(samples.into_boxed_slice());
            self.written.fetch_add(len, Ordering::Release);
        }
    }
    fn len(&self) -> usize {
        if self.finished.load(Ordering::Acquire) {
            self.written.load(Ordering::Acquire)
        } else {
            self.expected
        }
    }
    fn frame(&self, frame: usize) -> Option<[f32; 2]> {
        let sample = frame * 2;
        let block = self.blocks.get(sample / DECODE_BLOCK)?.get()?;
        let offset = sample % DECODE_BLOCK;
        Some([*block.get(offset)?, *block.get(offset + 1)?])
    }
}

/// A decoded track: the output thread never reads files or decodes packets.
struct Track {
    samples: Arc<SampleStore>,
    rate: SampleRate,
    position: Arc<AtomicU64>,
}

impl Track {
    fn decode(path: &Path) -> Result<Self, String> {
        let file = File::open(path).map_err(|e| e.to_string())?;
        let decoder = Decoder::try_from(file).map_err(|e| e.to_string())?;
        let rate = decoder.sample_rate();
        let duration = decoder.total_duration();
        let mut input = UniformSourceIterator::new(decoder, ChannelCount::new(2).unwrap(), rate);
        let samples = if let Some(duration) = duration.filter(|d| !d.is_zero()) {
            let expected = (duration.as_secs_f64() * rate.get() as f64).ceil() as usize * 2;
            // Allow a little metadata rounding; duration is corrected when decoding ends.
            let mut store = SampleStore::new(expected + rate.get() as usize * 4);
            store.expected = expected;
            let first: Vec<f32> = input.by_ref().take(DECODE_BLOCK).collect();
            if first.is_empty() {
                return Err("The track contains no decodable audio.".into());
            }
            store.publish(0, first);
            let store = Arc::new(store);
            let background = Arc::clone(&store);
            std::thread::Builder::new()
                .name("deck-decode".into())
                .spawn(move || {
                    for index in 1..background.blocks.len() {
                        if background.canceled.load(Ordering::Relaxed) {
                            break;
                        }
                        let block: Vec<f32> = input.by_ref().take(DECODE_BLOCK).collect();
                        if block.is_empty() {
                            break;
                        }
                        background.publish(index, block);
                    }
                    background.finished.store(true, Ordering::Release);
                })
                .map_err(|e| e.to_string())?;
            store
        } else {
            Arc::new(SampleStore::complete(input.collect()))
        };
        if samples.len() == 0 {
            return Err("The track contains no decodable audio.".into());
        }
        Ok(Self {
            samples,
            rate,
            position: Arc::new(AtomicU64::new(0)),
        })
    }

    fn wait_for_position(&self, position: Duration) {
        let target = (position.as_secs_f64() * self.rate.get() as f64) as usize * 2;
        while !self.samples.finished.load(Ordering::Acquire)
            && self.samples.written.load(Ordering::Acquire) <= target
        {
            std::thread::sleep(Duration::from_millis(2));
        }
    }

    fn duration(&self) -> Duration {
        Duration::from_secs_f64(self.samples.len() as f64 / 2.0 / self.rate.get() as f64)
    }

    fn source(&self, effects: Arc<Effects>) -> TrackSource {
        let volume = f32::from_bits(effects.volume.load(Ordering::Relaxed));
        let wet = f32::from_bits(effects.reverb.load(Ordering::Relaxed));
        TrackSource {
            samples: Arc::clone(&self.samples),
            rate: self.rate,
            cursor: 0,
            dsp: StudioReverb::new(self.rate.get() as usize),
            effects,
            position: Arc::clone(&self.position),
            volume,
            wet,
            right: None,
            smoothing: 1.0 - (-1.0 / (self.rate.get() as f32 * 0.01)).exp(),
        }
    }
}

impl Drop for Track {
    fn drop(&mut self) {
        self.samples.canceled.store(true, Ordering::Relaxed);
    }
}

struct TrackSource {
    samples: Arc<SampleStore>,
    rate: SampleRate,
    cursor: usize,
    position: Arc<AtomicU64>,
    dsp: StudioReverb,
    effects: Arc<Effects>,
    volume: f32,
    wet: f32,
    right: Option<f32>,
    smoothing: f32,
}

impl Iterator for TrackSource {
    type Item = f32;
    fn next(&mut self) -> Option<f32> {
        if let Some(right) = self.right.take() {
            return Some(right);
        }
        if self.cursor >= self.samples.len() {
            return None;
        }
        let Some(frame) = self.samples.frame(self.cursor / 2) else {
            // Never block the device callback if a slow disk temporarily falls behind.
            self.right = Some(0.0); // Silence an entire stereo frame, even if decoding finishes mid-frame.
            return Some(0.0);
        };
        self.cursor += 2;
        self.position
            .store((self.cursor / 2) as u64, Ordering::Relaxed);
        let target_volume = f32::from_bits(self.effects.volume.load(Ordering::Relaxed));
        let target_wet = f32::from_bits(self.effects.reverb.load(Ordering::Relaxed));
        self.volume += (target_volume - self.volume) * self.smoothing;
        self.wet += (target_wet - self.wet) * self.smoothing;
        let (left, right) = self.dsp.process(frame[0], frame[1], self.wet);
        // Dry playback remains untouched. Reserve headroom only when adding reverb.
        let gain = self.volume / (1.0 + self.wet * 0.5);
        self.right = Some(right * gain);
        Some(left * gain)
    }
}

impl Source for TrackSource {
    fn current_span_len(&self) -> Option<usize> {
        Some(self.samples.len().saturating_sub(self.cursor) + usize::from(self.right.is_some()))
    }
    fn channels(&self) -> ChannelCount {
        ChannelCount::new(2).unwrap()
    }
    fn sample_rate(&self) -> SampleRate {
        self.rate
    }
    fn total_duration(&self) -> Option<Duration> {
        Some(Duration::from_secs_f64(
            self.samples.len() as f64 / 2.0 / self.rate.get() as f64,
        ))
    }
    fn try_seek(&mut self, pos: Duration) -> Result<(), SeekError> {
        let frame = (pos.as_secs_f64() * self.rate.get() as f64).round() as usize;
        self.cursor = frame.min(self.samples.len() / 2) * 2;
        self.right = None;
        self.position
            .store((self.cursor / 2) as u64, Ordering::Relaxed);
        self.dsp.reset();
        Ok(())
    }
}

/// Prime while paused, before attaching to the device mixer. This ensures its
/// initial conversion uses the track's stereo layout and selected speed.
fn queued_player(
    mixer: &rodio::mixer::Mixer,
    track: &Track,
    effects: Arc<Effects>,
    speed: f32,
) -> Player {
    let (player, mut output) = Player::new();
    player.pause();
    player.set_speed(speed);
    player.append(track.source(effects));
    output.next();
    output.next();
    mixer.add(output);
    player
}

impl AudioEngine {
    #[cfg(test)]
    pub(crate) fn test_engine() -> (Self, Receiver<Request>, Sender<AudioStatus>) {
        let (cmd_tx, cmd_rx) = unbounded();
        let (status_tx, status_rx) = unbounded();
        (
            Self {
                cmd_tx,
                status_rx,
                revision: AtomicU64::new(0),
            },
            cmd_rx,
            status_tx,
        )
    }

    pub fn new() -> Result<Self, String> {
        let (cmd_tx, cmd_rx) = unbounded::<Request>();
        let (status_tx, status_rx) = unbounded::<AudioStatus>();
        let (ready_tx, ready_rx) = bounded(1);
        std::thread::Builder::new()
            .name("deck-playback".into())
            .spawn(move || {
                let (error_tx, error_rx) = unbounded::<String>();
                let stream = DeviceSinkBuilder::from_default_device()
                    .map(|builder| {
                        builder.with_error_callback(move |error| {
                            let _ = error_tx.send(format!("Audio output: {error}"));
                        })
                    })
                    .and_then(|builder| builder.open_stream());
                let stream = match stream {
                    Ok(stream) => {
                        let _ = ready_tx.send(Ok(()));
                        stream
                    }
                    Err(error) => {
                        let _ = ready_tx.send(Err(error.to_string()));
                        return;
                    }
                };
                let mut player = Player::connect_new(stream.mixer());
                player.pause();
                let effects = Arc::new(Effects::default());
                let mut track: Option<Track> = None;
                let mut revision = 0;
                let mut active = false;
                let mut speed = 1.0;
                let mut error = None;
                loop {
                    match cmd_rx.recv_timeout(Duration::from_millis(20)) {
                        Ok(request) => match request.command {
                            AudioCommand::Load {
                                path,
                                autoplay,
                                start_progress,
                            } => {
                                revision = request.revision;
                                player.stop();
                                active = false;
                                match Track::decode(&path) {
                                    Ok(decoded) => {
                                        let start = decoded
                                            .duration()
                                            .mul_f64(start_progress.clamp(0.0, 1.0) as f64);
                                        player = queued_player(
                                            stream.mixer(),
                                            &decoded,
                                            Arc::clone(&effects),
                                            speed,
                                        );
                                        if !start.is_zero() {
                                            decoded.wait_for_position(start);
                                            if let Err(e) = player.try_seek(start.div_f32(speed)) {
                                                error = Some(e.to_string());
                                            }
                                        }
                                        if autoplay {
                                            player.play();
                                        }
                                        track = Some(decoded);
                                        active = true;
                                    }
                                    Err(e) => {
                                        track = None;
                                        error =
                                            Some(format!("Cannot play {}: {e}", path.display()));
                                    }
                                }
                            }
                            AudioCommand::Seek(progress) => {
                                revision = request.revision;
                                if let Some(track) = &track {
                                    let position =
                                        track.duration().mul_f64(progress.clamp(0.0, 1.0) as f64);
                                    track.wait_for_position(position);
                                    if player.empty() {
                                        let paused = player.is_paused();
                                        player = queued_player(
                                            stream.mixer(),
                                            track,
                                            Arc::clone(&effects),
                                            speed,
                                        );
                                        if let Err(e) = player.try_seek(position.div_f32(speed)) {
                                            error = Some(e.to_string());
                                        }
                                        if !paused {
                                            player.play();
                                        }
                                    } else if let Err(e) = player.try_seek(position.div_f32(speed))
                                    {
                                        error = Some(e.to_string());
                                    }
                                    active = true;
                                }
                            }
                            AudioCommand::Toggle => {
                                if player.is_paused() {
                                    player.play();
                                } else {
                                    player.pause();
                                }
                            }
                            AudioCommand::SetSpeed(value) => {
                                speed = value.clamp(0.5, 1.5);
                                player.set_speed(speed);
                            }
                            AudioCommand::SetVolume(value) => {
                                effects
                                    .volume
                                    .store(value.clamp(0.0, 1.0).to_bits(), Ordering::Relaxed);
                            }
                            AudioCommand::SetReverb(value) => {
                                effects
                                    .reverb
                                    .store(value.clamp(0.0, 1.0).to_bits(), Ordering::Relaxed);
                            }
                        },
                        Err(crossbeam_channel::RecvTimeoutError::Disconnected) => break,
                        Err(crossbeam_channel::RecvTimeoutError::Timeout) => {}
                    }
                    if let Ok(message) = error_rx.try_recv() {
                        error = Some(message);
                        player.pause();
                    }
                    let duration_secs = track.as_ref().map_or(0.0, |t| t.duration().as_secs_f64());
                    let finished = active && player.empty();
                    if finished {
                        active = false;
                    }
                    let position_secs = if finished {
                        duration_secs
                    } else {
                        track
                            .as_ref()
                            .map_or(0.0, |t| {
                                t.position.load(Ordering::Relaxed) as f64 / t.rate.get() as f64
                            })
                            .min(duration_secs)
                    };
                    let _ = status_tx.send(AudioStatus {
                        revision,
                        is_playing: active && !player.is_paused(),
                        position_secs,
                        duration_secs,
                        progress: if duration_secs > 0.0 {
                            (position_secs / duration_secs) as f32
                        } else {
                            0.0
                        },
                        finished,
                        error: error.take(),
                    });
                }
            })
            .map_err(|e| e.to_string())?;
        ready_rx.recv().map_err(|e| e.to_string())??;
        Ok(Self {
            cmd_tx,
            status_rx,
            revision: AtomicU64::new(0),
        })
    }

    /// Load/seek revisions let the UI reject telemetry from before the user's action.
    pub fn send(&self, command: AudioCommand) -> u64 {
        let revision = if matches!(command, AudioCommand::Load { .. } | AudioCommand::Seek(_)) {
            self.revision.fetch_add(1, Ordering::Relaxed) + 1
        } else {
            self.revision.load(Ordering::Relaxed)
        };
        let _ = self.cmd_tx.send(Request { revision, command });
        revision
    }

    pub fn try_recv_status(&self) -> Option<AudioStatus> {
        let mut latest: Option<AudioStatus> = None;
        while let Ok(mut status) = self.status_rx.try_recv() {
            if let Some(previous) = latest {
                if previous.revision == status.revision {
                    status.finished |= previous.finished;
                    if status.error.is_none() {
                        status.error = previous.error;
                    }
                }
            }
            latest = Some(status);
        }
        latest
    }
}
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn delayed_decode_preserves_stereo_frame_alignment() {
        let (mut track, effects) = fixture();
        track.samples = Arc::new(SampleStore::new(4));
        let mut source = track.source(effects);
        assert_eq!(source.next(), Some(0.0));
        track.samples.publish(0, vec![0.25, -0.25, 0.5, -0.5]);
        assert_eq!(source.next(), Some(0.0));
        assert_eq!(source.next(), Some(0.25));
        assert_eq!(source.next(), Some(-0.25));
        assert_eq!(source.next(), Some(0.5));
        assert_eq!(source.next(), Some(-0.5));
        assert_eq!(source.next(), None);
    }

    fn fixture() -> (Track, Arc<Effects>) {
        let samples: Vec<f32> = (0..44100)
            .flat_map(|i| {
                let v = (i as f32 * std::f32::consts::TAU * 440.0 / 44100.0).sin() * 0.8;
                [v, -v]
            })
            .collect();
        let effects = Arc::new(Effects {
            volume: AtomicU32::new(1.0f32.to_bits()),
            reverb: AtomicU32::new(0.0f32.to_bits()),
        });
        (
            Track {
                samples: Arc::new(SampleStore::complete(samples)),
                rate: SampleRate::new(44100).unwrap(),
                position: Arc::new(AtomicU64::new(0)),
            },
            effects,
        )
    }

    #[test]
    fn dry_playback_preserves_every_sample_without_clipping_or_gaps() {
        let (track, effects) = fixture();
        assert_eq!(
            track.source(effects).collect::<Vec<_>>(),
            (0..track.samples.len() / 2)
                .flat_map(|i| track.samples.frame(i).unwrap())
                .collect::<Vec<_>>()
        );
    }

    #[test]
    fn repeated_seeks_land_on_exact_frames_and_discard_cached_channel() {
        let (track, effects) = fixture();
        let mut source = track.source(effects);
        for frame in [30000, 100, 44000, 500, 0, 22050] {
            source.next(); // Deliberately leave the right channel cached.
            source
                .try_seek(Duration::from_secs_f64(frame as f64 / 44100.0))
                .unwrap();
            assert_eq!(source.next(), Some(track.samples.frame(frame).unwrap()[0]));
            assert_eq!(source.next(), Some(track.samples.frame(frame).unwrap()[1]));
        }
        source.try_seek(Duration::from_secs(10)).unwrap();
        assert_eq!(source.next(), None);
        source.try_seek(Duration::ZERO).unwrap();
        assert_eq!(source.next(), Some(track.samples.frame(0).unwrap()[0]));
    }

    #[test]
    fn device_mixer_keeps_signal_continuous_at_common_output_rates_and_speeds() {
        let (track, effects) = fixture();
        for rate in [44100, 48000, 192000] {
            for speed in [0.5, 1.0, 1.5] {
                let (mixer, mut output) = rodio::mixer::mixer(
                    ChannelCount::new(2).unwrap(),
                    SampleRate::new(rate).unwrap(),
                );
                let player = queued_player(&mixer, &track, Arc::clone(&effects), speed);
                player.play();
                let samples: Vec<f32> = output
                    .by_ref()
                    .take((rate as f32 / speed * 2.0) as usize)
                    .collect();
                for (index, window) in samples
                    [rate as usize / 10..samples.len() - rate as usize / 10]
                    .chunks(1024)
                    .enumerate()
                {
                    let power = window.iter().map(|v| v * v).sum::<f32>() / window.len() as f32;
                    assert!(
                        power > 0.02,
                        "silent hole at {rate} Hz / {speed}x block {index}, power {power}"
                    );
                    assert!(window.iter().all(|v| v.is_finite() && v.abs() <= 0.801));
                }
                for (index, frame) in samples.chunks_exact(2).enumerate() {
                    assert!(
                        (frame[0] + frame[1]).abs() < 1e-5,
                        "channel mismatch at {rate}/{speed}, frame {index}: {frame:?}"
                    );
                }
            }
        }
    }

    #[test]
    fn volume_changes_are_ramped_instead_of_discontinuous() {
        let (mut track, effects) = fixture();
        track.samples = Arc::new(SampleStore::complete(vec![0.8; 88200]));
        let mut source = track.source(Arc::clone(&effects));
        assert_eq!(source.next(), Some(0.8));
        source.next();
        effects.volume.store(0.0f32.to_bits(), Ordering::Relaxed);
        assert!(source.next().unwrap() > 0.79);
        let tail = source.skip(8000).next().unwrap();
        assert!(tail < 0.001);
    }

    #[test]
    fn stale_completion_does_not_cross_seek_revision() {
        let (cmd_tx, _) = unbounded();
        let (tx, status_rx) = unbounded();
        let engine = AudioEngine {
            cmd_tx,
            status_rx,
            revision: AtomicU64::new(0),
        };
        tx.send(AudioStatus {
            revision: 1,
            finished: true,
            ..Default::default()
        })
        .unwrap();
        tx.send(AudioStatus {
            revision: 2,
            progress: 0.4,
            ..Default::default()
        })
        .unwrap();
        let status = engine.try_recv_status().unwrap();
        assert_eq!(status.revision, 2);
        assert!(!status.finished);
        assert_eq!(status.progress, 0.4);
    }

    #[test]
    #[ignore = "requires the local output device and DECK_TEST_TRACK; muted"]
    fn real_device_seek_and_playback_smoke() {
        let path = PathBuf::from(std::env::var("DECK_TEST_TRACK").expect("DECK_TEST_TRACK"));
        let engine = AudioEngine::new().unwrap();
        engine.send(AudioCommand::SetVolume(0.0));
        let load_started = std::time::Instant::now();
        let load_revision = engine.send(AudioCommand::Load {
            path,
            autoplay: false,
            start_progress: 0.0,
        });
        fn await_revision(engine: &AudioEngine, revision: u64) -> AudioStatus {
            let deadline = std::time::Instant::now() + Duration::from_secs(30);
            while std::time::Instant::now() < deadline {
                if let Some(status) = engine.try_recv_status() {
                    assert!(status.error.is_none(), "{:?}", status.error);
                    if status.revision == revision && status.duration_secs > 0.0 {
                        return status;
                    }
                }
                std::thread::sleep(Duration::from_millis(20));
            }
            panic!("No acknowledged playback state");
        }
        let loaded = await_revision(&engine, load_revision);
        eprintln!("First audio buffer ready in {:?}", load_started.elapsed());
        assert!(!loaded.is_playing);
        for (pct, speed) in [(0.7, 0.5), (0.1, 1.5), (0.8, 0.75), (0.2, 1.0)] {
            engine.send(AudioCommand::SetSpeed(speed));
            let revision = engine.send(AudioCommand::Seek(pct));
            let status = await_revision(&engine, revision);
            assert!((status.progress - pct).abs() < 0.001);
            std::thread::sleep(Duration::from_millis(100));
            let held = await_revision(&engine, revision);
            assert!(
                (held.position_secs - status.position_secs).abs() < 0.01,
                "paused seek drifted"
            );
        }
        engine.send(AudioCommand::Toggle);
        std::thread::sleep(Duration::from_secs(2));
        let status = engine.try_recv_status().unwrap();
        assert!(status.error.is_none(), "{:?}", status.error);
        assert!(status.is_playing);
        let advanced = status.position_secs - loaded.duration_secs * 0.2;
        assert!(
            (1.7..2.3).contains(&advanced),
            "playback clock advanced {advanced}"
        );
        eprintln!(
            "Native device playback and repeated paused seeks passed; duration {:.2}s",
            loaded.duration_secs
        );
    }
}
