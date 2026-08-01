//! Practice tools: metronome, drum groove, and song player.
//!
//! All three run on an aux player thread (off the audio callback), rendering
//! into a lock-free ring that the engine sums into the output. They share the
//! same global tempo (ADR 014).

use super::*;

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::thread::JoinHandle;
use std::time::Duration;

use lh_dsp::practice::{DrumMachine, Metronome};

/// Stereo frames the aux monitor ring holds (~170 ms @ 48k) — deep enough that
/// the player thread's periodic fill never underruns the audio callback.
pub(super) const AUX_RING_FRAMES: usize = 8_192;
/// Frames the player renders at most per wake.
pub(super) const PLAYER_CHUNK: usize = 2_048;
/// The player keeps roughly this much audio buffered ahead — enough to ride
/// scheduling jitter, short enough that a tempo/enable change is heard promptly
/// (a whole-ring pre-fill would lag control by the ring depth).
pub(super) const PLAYER_TARGET_MS: f32 = 50.0;
/// The player thread sleeps this long between fills.
pub(super) const PLAYER_TICK: Duration = Duration::from_millis(3);

/// Cross-thread metronome control (PRD 019): the session writes these atomics,
/// the aux player thread reads them each fill. All scalars are independent (no
/// multi-field invariant), so `Relaxed` is sufficient; the audio content itself
/// travels the lock-free aux ring. The BPM mirrors the rig's global tempo
/// (ADR 014), pushed here whenever the tempo moves.
pub(super) struct MetronomeShared {
    /// The player thread runs while this is set (cleared on session drop).
    pub(super) running: AtomicBool,
    pub(super) enabled: AtomicBool,
    /// Bumped to force a downbeat restart (enable / count-in).
    pub(super) restart_gen: AtomicU32,
    pub(super) bpm_bits: AtomicU32,
    pub(super) volume_bits: AtomicU32,
    pub(super) beats_per_bar: AtomicU32,
    pub(super) accent: AtomicBool,
}

impl MetronomeShared {
    pub(super) fn new(bpm: f32) -> Self {
        Self {
            running: AtomicBool::new(true),
            enabled: AtomicBool::new(false),
            restart_gen: AtomicU32::new(0),
            bpm_bits: AtomicU32::new(lh_core::tempo::clamp_bpm(bpm).to_bits()),
            volume_bits: AtomicU32::new(0.6f32.to_bits()),
            beats_per_bar: AtomicU32::new(4),
            accent: AtomicBool::new(true),
        }
    }

    pub(super) fn enabled(&self) -> bool {
        self.enabled.load(Ordering::Relaxed)
    }
    pub(super) fn set_enabled(&self, on: bool) {
        self.enabled.store(on, Ordering::Relaxed);
    }
    pub(super) fn bpm(&self) -> f32 {
        f32::from_bits(self.bpm_bits.load(Ordering::Relaxed))
    }
    pub(super) fn set_bpm(&self, bpm: f32) {
        self.bpm_bits
            .store(lh_core::tempo::clamp_bpm(bpm).to_bits(), Ordering::Relaxed);
    }
    pub(super) fn volume(&self) -> f32 {
        f32::from_bits(self.volume_bits.load(Ordering::Relaxed))
    }
    pub(super) fn set_volume(&self, v: f32) {
        self.volume_bits
            .store(v.clamp(0.0, 1.0).to_bits(), Ordering::Relaxed);
    }
    pub(super) fn beats_per_bar(&self) -> u32 {
        self.beats_per_bar.load(Ordering::Relaxed).clamp(1, 16)
    }
    pub(super) fn set_beats_per_bar(&self, n: u32) {
        self.beats_per_bar.store(n.clamp(1, 16), Ordering::Relaxed);
    }
    pub(super) fn accent(&self) -> bool {
        self.accent.load(Ordering::Relaxed)
    }
    pub(super) fn set_accent(&self, on: bool) {
        self.accent.store(on, Ordering::Relaxed);
    }
    pub(super) fn request_restart(&self) {
        self.restart_gen.fetch_add(1, Ordering::Relaxed);
    }
    pub(super) fn restart_gen(&self) -> u32 {
        self.restart_gen.load(Ordering::Relaxed)
    }
}

/// The runtime metronome state that must survive an audio-engine restart
/// (device/buffer change) — carried across `Session::carry_over`. BPM is not
/// here: it rides `config.tempo_bpm`, re-read on the fresh session.
#[derive(Clone, Copy)]
pub(super) struct MetroSnapshot {
    pub(super) enabled: bool,
    pub(super) volume: f32,
    pub(super) beats_per_bar: u32,
    pub(super) accent: bool,
}

/// Cross-thread drum-groove control (PRD 019, Phase 2), read by the aux player
/// thread each fill. The groove tracks the same global tempo the metronome does
/// (the player reads BPM from `MetronomeShared` and drives both).
pub(super) struct GrooveShared {
    pub(super) enabled: AtomicBool,
    pub(super) pattern: AtomicU32,
    pub(super) volume_bits: AtomicU32,
    /// Bumped to arm a one-bar fill.
    pub(super) fill_gen: AtomicU32,
    /// Bumped to restart the loop on the downbeat (enable).
    pub(super) restart_gen: AtomicU32,
}

impl GrooveShared {
    pub(super) fn new() -> Self {
        Self {
            enabled: AtomicBool::new(false),
            pattern: AtomicU32::new(0),
            volume_bits: AtomicU32::new(0.7f32.to_bits()),
            fill_gen: AtomicU32::new(0),
            restart_gen: AtomicU32::new(0),
        }
    }

    pub(super) fn enabled(&self) -> bool {
        self.enabled.load(Ordering::Relaxed)
    }
    pub(super) fn set_enabled(&self, on: bool) {
        self.enabled.store(on, Ordering::Relaxed);
    }
    pub(super) fn pattern(&self) -> u32 {
        self.pattern.load(Ordering::Relaxed)
    }
    pub(super) fn set_pattern(&self, index: u32) {
        self.pattern.store(index, Ordering::Relaxed);
    }
    pub(super) fn volume(&self) -> f32 {
        f32::from_bits(self.volume_bits.load(Ordering::Relaxed))
    }
    pub(super) fn set_volume(&self, v: f32) {
        self.volume_bits
            .store(v.clamp(0.0, 1.0).to_bits(), Ordering::Relaxed);
    }
    pub(super) fn request_fill(&self) {
        self.fill_gen.fetch_add(1, Ordering::Relaxed);
    }
    pub(super) fn fill_gen(&self) -> u32 {
        self.fill_gen.load(Ordering::Relaxed)
    }
    pub(super) fn request_restart(&self) {
        self.restart_gen.fetch_add(1, Ordering::Relaxed);
    }
    pub(super) fn restart_gen(&self) -> u32 {
        self.restart_gen.load(Ordering::Relaxed)
    }
}

/// Runtime groove state carried across an audio-engine restart.
#[derive(Clone, Copy)]
pub(super) struct GrooveSnapshot {
    pub(super) enabled: bool,
    pub(super) pattern: u32,
    pub(super) volume: f32,
}

/// Cross-thread song-player control (PRD 019, Phase 3), read by the aux player
/// thread each fill. The decoded buffer travels a separate channel (it is large;
/// this is just the transport controls + the played-back position feedback).
pub(super) struct SongShared {
    pub(super) playing: AtomicBool,
    pub(super) speed_bits: AtomicU32,
    pub(super) semitones_bits: AtomicU32,
    pub(super) mix_bits: AtomicU32,
    /// A-B loop in source frames; `b <= a` means no loop.
    pub(super) loop_a: AtomicU32,
    pub(super) loop_b: AtomicU32,
    /// Bumped to request a seek to `seek_target` frames.
    pub(super) seek_gen: AtomicU32,
    pub(super) seek_target: AtomicU32,
    /// Current play position (frames), published by the player for the GUI.
    pub(super) pos_frames: AtomicU32,
    /// Total frames of the loaded song (0 = none), published by the player.
    pub(super) total_frames: AtomicU32,
}

impl SongShared {
    pub(super) fn new() -> Self {
        Self {
            playing: AtomicBool::new(false),
            speed_bits: AtomicU32::new(1.0f32.to_bits()),
            semitones_bits: AtomicU32::new(0.0f32.to_bits()),
            mix_bits: AtomicU32::new(0.7f32.to_bits()),
            loop_a: AtomicU32::new(0),
            loop_b: AtomicU32::new(0),
            seek_gen: AtomicU32::new(0),
            seek_target: AtomicU32::new(0),
            pos_frames: AtomicU32::new(0),
            total_frames: AtomicU32::new(0),
        }
    }

    pub(super) fn playing(&self) -> bool {
        self.playing.load(Ordering::Relaxed)
    }
    pub(super) fn set_playing(&self, on: bool) {
        self.playing.store(on, Ordering::Relaxed);
    }
    pub(super) fn speed(&self) -> f32 {
        f32::from_bits(self.speed_bits.load(Ordering::Relaxed))
    }
    pub(super) fn set_speed(&self, v: f32) {
        self.speed_bits
            .store(v.clamp(0.25, 2.0).to_bits(), Ordering::Relaxed);
    }
    pub(super) fn semitones(&self) -> f32 {
        f32::from_bits(self.semitones_bits.load(Ordering::Relaxed))
    }
    pub(super) fn set_semitones(&self, v: f32) {
        self.semitones_bits
            .store(v.clamp(-12.0, 12.0).to_bits(), Ordering::Relaxed);
    }
    pub(super) fn mix(&self) -> f32 {
        f32::from_bits(self.mix_bits.load(Ordering::Relaxed))
    }
    pub(super) fn set_mix(&self, v: f32) {
        self.mix_bits
            .store(v.clamp(0.0, 1.0).to_bits(), Ordering::Relaxed);
    }
    pub(super) fn loop_range(&self) -> (u32, u32) {
        (
            self.loop_a.load(Ordering::Relaxed),
            self.loop_b.load(Ordering::Relaxed),
        )
    }
    pub(super) fn set_loop(&self, a: u32, b: u32) {
        self.loop_a.store(a, Ordering::Relaxed);
        self.loop_b.store(b, Ordering::Relaxed);
    }
    pub(super) fn seek(&self, target: u32) {
        self.seek_target.store(target, Ordering::Relaxed);
        self.seek_gen.fetch_add(1, Ordering::Relaxed);
    }
    pub(super) fn seek_gen(&self) -> u32 {
        self.seek_gen.load(Ordering::Relaxed)
    }
    pub(super) fn seek_target(&self) -> u32 {
        self.seek_target.load(Ordering::Relaxed)
    }
    pub(super) fn pos_frames(&self) -> u32 {
        self.pos_frames.load(Ordering::Relaxed)
    }
    pub(super) fn set_pos_frames(&self, p: u32) {
        self.pos_frames.store(p, Ordering::Relaxed);
    }
    pub(super) fn set_total_frames(&self, n: u32) {
        self.total_frames.store(n, Ordering::Relaxed);
    }
}

/// Buckets in the waveform peak envelope handed to the GUI.
pub(super) const SONG_WAVEFORM_BUCKETS: usize = 400;

/// Spawn the aux player thread (PRD 019): it renders the metronome, the drum
/// groove, **and** the song player, sums them, and keeps ~50ms of the mix
/// buffered ahead in the aux ring. Off the audio thread, so heap use here is
/// fine.
pub(super) fn spawn_player(
    mut prod: rtrb::Producer<f32>,
    metro_shared: Arc<MetronomeShared>,
    groove_shared: Arc<GrooveShared>,
    song_shared: Arc<SongShared>,
    song_rx: std::sync::mpsc::Receiver<Arc<lh_dsp::practice::SongBuffer>>,
    sample_rate: u32,
) -> JoinHandle<()> {
    std::thread::Builder::new()
        .name("lh-aux-player".into())
        .spawn(move || {
            let target_frames =
                ((PLAYER_TARGET_MS * 1e-3 * sample_rate as f32) as usize).min(AUX_RING_FRAMES);
            let mut metro = Metronome::new();
            metro.prepare(sample_rate);
            let mut drums = DrumMachine::new();
            drums.prepare(sample_rate);
            let mut song = lh_dsp::practice::SongPlayer::new();
            song.prepare(sample_rate);
            let mut metro_buf = vec![0.0f32; PLAYER_CHUNK];
            let mut drum_buf = vec![0.0f32; PLAYER_CHUNK];
            let mut song_l = vec![0.0f32; PLAYER_CHUNK];
            let mut song_r = vec![0.0f32; PLAYER_CHUNK];

            let mut metro_last_gen = metro_shared.restart_gen();
            let mut metro_was_on = false;
            let mut groove_last_restart = groove_shared.restart_gen();
            let mut groove_last_fill = groove_shared.fill_gen();
            let mut groove_was_on = false;
            let mut song_last_seek = song_shared.seek_gen();
            let mut song_want_prev = false;

            while metro_shared.running.load(Ordering::Relaxed) {
                let bpm = metro_shared.bpm();

                // --- metronome control ---
                metro.set_bpm(bpm);
                metro.set_volume(metro_shared.volume());
                metro.set_beats_per_bar(metro_shared.beats_per_bar());
                metro.set_accent(metro_shared.accent());
                let metro_on = metro_shared.enabled();
                let metro_gen = metro_shared.restart_gen();
                if (metro_on && !metro_was_on) || metro_gen != metro_last_gen {
                    metro.restart();
                }
                metro_last_gen = metro_gen;
                metro_was_on = metro_on;

                // --- groove control ---
                drums.set_bpm(bpm);
                drums.set_volume(groove_shared.volume());
                drums.set_pattern(groove_shared.pattern() as usize);
                let groove_on = groove_shared.enabled();
                let groove_restart = groove_shared.restart_gen();
                if (groove_on && !groove_was_on) || groove_restart != groove_last_restart {
                    drums.restart();
                }
                groove_last_restart = groove_restart;
                groove_was_on = groove_on;
                let fill = groove_shared.fill_gen();
                if fill != groove_last_fill {
                    drums.fill();
                    groove_last_fill = fill;
                }

                // --- song control ---
                if let Ok(buf) = song_rx.try_recv() {
                    song_shared.set_total_frames(buf.frames() as u32);
                    song.set_song(buf);
                }
                song.set_speed(song_shared.speed());
                song.set_semitones(song_shared.semitones());
                song.set_mix(song_shared.mix());
                let (la, lb) = song_shared.loop_range();
                song.set_loop(la as usize, lb as usize);
                let seek = song_shared.seek_gen();
                if seek != song_last_seek {
                    song.seek(song_shared.seek_target() as usize);
                    song_last_seek = seek;
                }
                let song_want = song_shared.playing();
                if song_want && !song_want_prev {
                    if song.loop_range().is_none() && song.pos_frames() >= song.song_frames() {
                        song.seek(0);
                    }
                    song.play();
                } else if !song_want && song_want_prev {
                    song.stop();
                }
                song_want_prev = song_want;
                let song_on = song.is_playing();

                if metro_on || groove_on || song_on {
                    let free_frames = prod.slots() / 2;
                    let buffered = AUX_RING_FRAMES - free_frames;
                    let want = target_frames.saturating_sub(buffered);
                    let n = want.min(free_frames).min(PLAYER_CHUNK);
                    if n > 0 {
                        if metro_on {
                            metro.render(&mut metro_buf[..n]);
                        } else {
                            metro_buf[..n].fill(0.0);
                        }
                        if groove_on {
                            drums.render(&mut drum_buf[..n]);
                        } else {
                            drum_buf[..n].fill(0.0);
                        }
                        song.render(&mut song_l[..n], &mut song_r[..n]);
                        if let Ok(mut chunk) = prod.write_chunk(2 * n) {
                            let (a, b) = chunk.as_mut_slices();
                            let mut dst = a.iter_mut().chain(b.iter_mut());
                            for f in 0..n {
                                let m = metro_buf[f] + drum_buf[f];
                                if let (Some(dl), Some(dr)) = (dst.next(), dst.next()) {
                                    *dl = m + song_l[f];
                                    *dr = m + song_r[f];
                                }
                            }
                            drop(dst);
                            chunk.commit_all();
                        }
                    }
                }

                song_shared.set_pos_frames(song.pos_frames() as u32);
                if song_want && !song.is_playing() {
                    song_shared.set_playing(false);
                    song_want_prev = false;
                }
                std::thread::sleep(PLAYER_TICK);
            }
        })
        .expect("spawn aux player thread")
}

impl super::Session {
    /// Whether the metronome click is currently on.
    pub fn metronome_on(&self) -> bool {
        self.metronome.enabled()
    }

    /// Enable or disable the click; enabling restarts the bar on beat 1.
    pub fn set_metronome(&mut self, on: bool) -> String {
        self.metronome.set_enabled(on);
        if on {
            self.metronome.request_restart();
        }
        format!("metronome {}", if on { "on" } else { "off" })
    }

    /// Toggle the click (GUI button / footswitch).
    pub fn toggle_metronome(&mut self) -> String {
        self.set_metronome(!self.metronome.enabled())
    }

    /// Click level, `0.0..=1.0`.
    pub fn click_volume(&self) -> f32 {
        self.metronome.volume()
    }

    pub fn set_click_volume(&mut self, volume: f32) -> String {
        self.metronome.set_volume(volume);
        format!("click volume {:.0}%", self.metronome.volume() * 100.0)
    }

    /// Beats per bar (the accent recurs every `n`).
    pub fn beats_per_bar(&self) -> u32 {
        self.metronome.beats_per_bar()
    }

    pub fn set_beats_per_bar(&mut self, n: u32) -> String {
        self.metronome.set_beats_per_bar(n);
        format!("time signature {}/4", self.metronome.beats_per_bar())
    }

    /// Turn the click on and restart the bar — a count-in lead from beat 1.
    pub fn count_in(&mut self) -> String {
        self.metronome.set_enabled(true);
        self.metronome.request_restart();
        "count-in — click from beat 1".to_string()
    }

    // --- practice tools: drum groove (PRD 019, Phase 2) ---

    /// Whether the drum groove is currently playing.
    pub fn groove_on(&self) -> bool {
        self.groove.enabled()
    }

    /// Start/stop the groove; starting restarts the loop on the downbeat.
    pub fn set_groove(&mut self, on: bool) -> String {
        self.groove.set_enabled(on);
        if on {
            self.groove.request_restart();
        }
        format!(
            "drums {} ({})",
            if on { "on" } else { "off" },
            lh_dsp::practice::pattern_name(self.groove.pattern() as usize),
        )
    }

    pub fn toggle_groove(&mut self) -> String {
        self.set_groove(!self.groove.enabled())
    }

    /// Current groove pattern's menu name.
    pub fn groove_pattern_name(&self) -> &'static str {
        lh_dsp::practice::pattern_name(self.groove.pattern() as usize)
    }

    /// Select a groove by name (e.g. `"funk"`) or numeric index.
    pub fn set_groove_pattern(&mut self, selector: &str) -> Result<String, String> {
        let count = lh_dsp::practice::pattern_count();
        let index = lh_dsp::practice::pattern_index(selector)
            .or_else(|| selector.parse::<usize>().ok().filter(|i| *i < count))
            .ok_or_else(|| format!("unknown groove {selector:?}"))?;
        self.groove.set_pattern(index as u32);
        Ok(format!("groove {}", lh_dsp::practice::pattern_name(index)))
    }

    /// Step to the next groove pattern (GUI chip), wrapping.
    pub fn cycle_groove_pattern(&mut self) -> String {
        let count = lh_dsp::practice::pattern_count().max(1) as u32;
        let next = (self.groove.pattern() + 1) % count;
        self.groove.set_pattern(next);
        format!("groove {}", lh_dsp::practice::pattern_name(next as usize))
    }

    pub fn groove_volume(&self) -> f32 {
        self.groove.volume()
    }

    pub fn set_groove_volume(&mut self, volume: f32) -> String {
        self.groove.set_volume(volume);
        format!("drum volume {:.0}%", self.groove.volume() * 100.0)
    }

    /// Arm a one-bar drum fill (plays from the next downbeat).
    pub fn groove_fill(&mut self) -> String {
        self.groove.request_fill();
        "drum fill armed".to_string()
    }

    // --- practice tools: song player (PRD 019, Phase 3) ---

    /// Decode a backing track on a background loader thread (WAV/MP3). The
    /// result arrives on [`Self::poll_song`]. Not carried across a device
    /// restart — the buffer is large and the player thread is rebuilt.
    pub fn load_song(&mut self, path: &Path) -> String {
        let sr = self.sample_rate;
        let tx = self.song_load_tx.clone();
        self.config.song_dir = parent_dir(path);
        save_config(&self.config);
        let path = path.to_path_buf();
        let name = file_name(&path);
        let report = name.clone();
        std::thread::spawn(move || {
            let msg = match crate::song_loader::load_song(&path, sr) {
                Ok(song) => SongLoad::Loaded {
                    peaks: song.peaks(SONG_WAVEFORM_BUCKETS),
                    song: Arc::new(song),
                    name,
                },
                Err(e) => SongLoad::Failed(format!("song load failed: {e}")),
            };
            let _ = tx.send(msg);
        });
        format!("loading song {report:?}…")
    }

    /// Drain a completed decode (call on the control tick). Hands the buffer to
    /// the player and returns a status line, or `None` if nothing finished.
    pub fn poll_song(&mut self) -> Option<String> {
        match self.song_load_rx.try_recv().ok()? {
            SongLoad::Loaded { song, name, peaks } => {
                let secs = song.seconds();
                self.song.set_total_frames(song.frames() as u32);
                self.song.seek(0);
                self.song.set_playing(false);
                self.song_current = Some(Arc::clone(&song));
                self.song_name = Some(name.clone());
                self.song_peaks = peaks;
                let _ = self.song_tx.send(song);
                Some(format!("song {name:?} loaded ({secs:.0}s)"))
            }
            SongLoad::Failed(e) => Some(e),
        }
    }

    pub fn song_play(&mut self) -> String {
        if self.song_current.is_none() {
            return "no song loaded".into();
        }
        self.song.set_playing(true);
        "song playing".into()
    }
    pub fn song_stop(&mut self) -> String {
        self.song.set_playing(false);
        "song stopped".into()
    }
    pub fn song_toggle(&mut self) -> String {
        if self.song.playing() {
            self.song_stop()
        } else {
            self.song_play()
        }
    }
    pub fn song_is_playing(&self) -> bool {
        self.song.playing()
    }
    pub fn has_song(&self) -> bool {
        self.song_current.is_some()
    }
    pub fn song_name(&self) -> Option<&str> {
        self.song_name.as_deref()
    }
    pub fn song_peaks(&self) -> &[f32] {
        &self.song_peaks
    }
    pub fn song_frames(&self) -> usize {
        self.song_current.as_ref().map_or(0, |s| s.frames())
    }
    pub fn song_seconds(&self) -> f32 {
        self.song_current.as_ref().map_or(0.0, |s| s.seconds())
    }
    /// Current play position as a fraction `0..1` (for the GUI progress bar).
    pub fn song_fraction(&self) -> f32 {
        let total = self.song_frames();
        if total == 0 {
            0.0
        } else {
            (self.song.pos_frames() as f32 / total as f32).clamp(0.0, 1.0)
        }
    }

    pub fn set_song_speed(&mut self, speed: f32) -> String {
        self.song.set_speed(speed);
        format!("song speed {:.0}%", self.song.speed() * 100.0)
    }
    pub fn song_speed(&self) -> f32 {
        self.song.speed()
    }
    pub fn set_song_semitones(&mut self, semitones: f32) -> String {
        self.song.set_semitones(semitones);
        format!("song transpose {:+.0} st", self.song.semitones())
    }
    pub fn song_semitones(&self) -> f32 {
        self.song.semitones()
    }
    pub fn set_song_mix(&mut self, mix: f32) -> String {
        self.song.set_mix(mix);
        format!("song mix {:.0}%", self.song.mix() * 100.0)
    }
    pub fn song_mix(&self) -> f32 {
        self.song.mix()
    }

    /// Seek to a fraction `0..1` of the song.
    pub fn song_seek_fraction(&mut self, frac: f32) {
        let frame = (frac.clamp(0.0, 1.0) * self.song_frames() as f32) as u32;
        self.song.seek(frame);
    }

    /// Set the A-B loop from fractions of the song (`a >= b` clears it).
    pub fn set_song_loop_fraction(&mut self, a: f32, b: f32) -> String {
        let total = self.song_frames() as f32;
        let fa = (a.clamp(0.0, 1.0) * total) as u32;
        let fb = (b.clamp(0.0, 1.0) * total) as u32;
        self.song.set_loop(fa, fb);
        if fb > fa {
            format!(
                "song loop {:.0}s–{:.0}s",
                a * self.song_seconds(),
                b * self.song_seconds()
            )
        } else {
            "song loop cleared".into()
        }
    }
    pub fn clear_song_loop(&mut self) -> String {
        self.song.set_loop(0, 0);
        "song loop cleared".into()
    }
    /// The A-B loop as fractions `0..1`, if set.
    pub fn song_loop_fraction(&self) -> Option<(f32, f32)> {
        let (a, b) = self.song.loop_range();
        let total = self.song_frames() as f32;
        (b > a && total > 0.0).then(|| (a as f32 / total, b as f32 / total))
    }
}
