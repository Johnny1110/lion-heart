//! Monitor recorder (PRD 014): write the DI (dry input) and wet (processed)
//! stereo tracks to disk while playing, so a take can be re-amped offline
//! later. The engine's two recording taps ([`lh_engine::Chain::set_record_taps`])
//! push interleaved stereo into a pair of rings; a dedicated disk thread here
//! drains them through `hound`. Nothing on this path touches the audio thread —
//! the callback only does an armed check plus a lock-free ring write.

use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread::JoinHandle;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use lh_assets::wav::{WavBits, WavStream};
use lh_engine::RecordTapState;

/// Seconds of audio the tap rings buffer — deep enough to ride a disk hiccup
/// without dropping frames, small enough to stay a couple of MB.
const RING_SECS: u32 = 2;
/// The disk thread drains this often. Well under [`RING_SECS`] so the rings
/// never fill in normal operation.
const DRAIN_INTERVAL: Duration = Duration::from_millis(10);

/// Live status of an in-progress take, for the UI.
#[derive(Debug, Clone, Copy)]
pub struct RecStatus {
    pub elapsed: Duration,
    /// Frames the tap rings could not accept (disk fell behind) — a defect,
    /// shown so a bad take is obvious rather than silently corrupt.
    pub dropped: u64,
}

/// The result of stopping a take.
#[derive(Debug, Clone)]
pub struct RecSummary {
    pub di_path: PathBuf,
    pub wet_path: PathBuf,
    pub seconds: f32,
    pub dropped: u64,
}

impl RecSummary {
    /// A one-line report for the REPL / status line.
    pub fn human(&self) -> String {
        let drops = if self.dropped == 0 {
            String::new()
        } else {
            format!(" — WARNING: {} frames dropped", self.dropped)
        };
        format!(
            "recorded {:.1}s → {} + {}{}",
            self.seconds,
            self.di_path.display(),
            self.wet_path
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_default(),
            drops,
        )
    }
}

/// An active take: the disk thread plus where it is writing.
struct Take {
    stop: Arc<AtomicBool>,
    join: Option<JoinHandle<(rtrb::Consumer<f32>, rtrb::Consumer<f32>)>>,
    started: Instant,
    di_path: PathBuf,
    wet_path: PathBuf,
}

/// Owns the recording tap consumers between takes and spawns the disk thread
/// for each take. Created once per session, alongside the taps.
pub struct Recorder {
    /// Held here while idle; moved into the disk thread while recording.
    di_cons: Option<rtrb::Consumer<f32>>,
    wet_cons: Option<rtrb::Consumer<f32>>,
    /// Shared armed flag + dropped counter (both taps share one state).
    tap: Arc<RecordTapState>,
    sample_rate: u32,
    bits: WavBits,
    dir: PathBuf,
    active: Option<Take>,
}

impl Recorder {
    pub fn new(
        di_cons: rtrb::Consumer<f32>,
        wet_cons: rtrb::Consumer<f32>,
        tap: Arc<RecordTapState>,
        sample_rate: u32,
        dir: PathBuf,
        bits: WavBits,
    ) -> Self {
        Self {
            di_cons: Some(di_cons),
            wet_cons: Some(wet_cons),
            tap,
            sample_rate,
            bits,
            dir,
            active: None,
        }
    }

    /// Frames a `RING_SECS` tap ring should hold (interleaved stereo), given
    /// the stream rate. The session sizes both rings with this.
    pub fn ring_capacity(sample_rate: u32) -> usize {
        (sample_rate * RING_SECS) as usize * 2
    }

    pub fn is_recording(&self) -> bool {
        self.active.is_some()
    }

    /// Live take status, or `None` when idle.
    pub fn status(&self) -> Option<RecStatus> {
        self.active.as_ref().map(|t| RecStatus {
            elapsed: t.started.elapsed(),
            dropped: self.tap.dropped.load(Ordering::Relaxed),
        })
    }

    /// Begin a take: discard any stale ring contents, open the two WAVs, and
    /// start the disk thread. Returns the DI/wet paths.
    pub fn start(&mut self) -> Result<(PathBuf, PathBuf), String> {
        if self.active.is_some() {
            return Err("already recording".into());
        }
        if self.di_cons.is_none() || self.wet_cons.is_none() {
            return Err("recorder is in a bad state (rings unavailable)".into());
        }
        // Do every fallible step *before* taking the rings out, so a transient
        // failure (bad dir, open error) leaves the recorder fully usable.
        std::fs::create_dir_all(&self.dir)
            .map_err(|e| format!("cannot create {}: {e}", self.dir.display()))?;
        let (di_path, wet_path) = self.take_paths();
        let di_stream = WavStream::create(&di_path, 2, self.sample_rate, self.bits)
            .map_err(|e| e.to_string())?;
        let wet_stream = WavStream::create(&wet_path, 2, self.sample_rate, self.bits)
            .map_err(|e| e.to_string())?;

        // Commit: take the rings (checked present above), discard anything
        // buffered from before the take, and zero the counter so `dropped`
        // measures only this take.
        let mut di = self.di_cons.take().expect("checked present");
        let mut wet = self.wet_cons.take().expect("checked present");
        drain_discard(&mut di);
        drain_discard(&mut wet);
        self.tap.dropped.store(0, Ordering::Relaxed);

        let stop = Arc::new(AtomicBool::new(false));
        let join = spawn_disk_thread(di, wet, di_stream, wet_stream, Arc::clone(&stop));

        // Arm only once the writer is ready to drain, so the first captured
        // sample is fresh (the ring was just emptied).
        self.tap.armed.store(true, Ordering::Relaxed);
        self.active = Some(Take {
            stop,
            join: Some(join),
            started: Instant::now(),
            di_path: di_path.clone(),
            wet_path: wet_path.clone(),
        });
        Ok((di_path, wet_path))
    }

    /// End the take: disarm the taps, flush the disk thread, and reclaim the
    /// rings for the next take.
    pub fn stop(&mut self) -> Result<RecSummary, String> {
        let mut take = self.active.take().ok_or("not recording")?;
        // Disarm first so the producer stops writing; then the thread's final
        // drain sees a bounded ring and finalizes the WAVs.
        self.tap.armed.store(false, Ordering::Relaxed);
        take.stop.store(true, Ordering::Relaxed);
        let seconds = take.started.elapsed().as_secs_f32();
        let dropped = self.tap.dropped.load(Ordering::Relaxed);
        if let Some(join) = take.join.take() {
            match join.join() {
                Ok((di, wet)) => {
                    self.di_cons = Some(di);
                    self.wet_cons = Some(wet);
                }
                Err(_) => {
                    // The disk thread panicked — the rings are gone. Recording
                    // is disabled until a device restart rebuilds them.
                    eprintln!(
                        "warning: recorder disk thread panicked; recording disabled until restart"
                    );
                }
            }
        }
        Ok(RecSummary {
            di_path: take.di_path,
            wet_path: take.wet_path,
            seconds,
            dropped,
        })
    }

    /// Timestamped, collision-free `<ts>-di.wav` / `<ts>-wet.wav` in the dir.
    fn take_paths(&self) -> (PathBuf, PathBuf) {
        let base = timestamp_utc();
        for n in 0..100 {
            let stem = if n == 0 {
                base.clone()
            } else {
                format!("{base}-{n}")
            };
            let di = self.dir.join(format!("{stem}-di.wav"));
            let wet = self.dir.join(format!("{stem}-wet.wav"));
            if !di.exists() && !wet.exists() {
                return (di, wet);
            }
        }
        // Extremely unlikely; fall back to the base names (overwrite).
        (
            self.dir.join(format!("{base}-di.wav")),
            self.dir.join(format!("{base}-wet.wav")),
        )
    }
}

impl Drop for Recorder {
    /// Finalize any in-progress take so a session teardown never leaves a
    /// truncated WAV.
    fn drop(&mut self) {
        if self.active.is_some() {
            let _ = self.stop();
        }
    }
}

/// Pull everything currently buffered and throw it away.
fn drain_discard(cons: &mut rtrb::Consumer<f32>) {
    let n = cons.slots();
    if n > 0
        && let Ok(chunk) = cons.read_chunk(n)
    {
        chunk.commit_all();
    }
}

/// The disk thread: drain both rings into their WAVs until stopped, do one
/// final drain, finalize, and hand the rings back.
fn spawn_disk_thread(
    mut di: rtrb::Consumer<f32>,
    mut wet: rtrb::Consumer<f32>,
    mut di_stream: WavStream<std::io::BufWriter<std::fs::File>>,
    mut wet_stream: WavStream<std::io::BufWriter<std::fs::File>>,
    stop: Arc<AtomicBool>,
) -> JoinHandle<(rtrb::Consumer<f32>, rtrb::Consumer<f32>)> {
    std::thread::Builder::new()
        .name("lh-recorder".into())
        .spawn(move || {
            let mut scratch = Vec::new();
            loop {
                let stopping = stop.load(Ordering::Relaxed);
                drain_write(&mut di, &mut di_stream, &mut scratch);
                drain_write(&mut wet, &mut wet_stream, &mut scratch);
                if stopping {
                    // The tap disarmed just before `stop` was set, but one audio
                    // block may have been in flight (already past the armed
                    // check). Settle once and drain again so the take's tail is
                    // captured, not truncated.
                    std::thread::sleep(DRAIN_INTERVAL);
                    drain_write(&mut di, &mut di_stream, &mut scratch);
                    drain_write(&mut wet, &mut wet_stream, &mut scratch);
                    break;
                }
                std::thread::sleep(DRAIN_INTERVAL);
            }
            if let Err(e) = di_stream.finalize() {
                eprintln!("warning: DI recording not finalized: {e}");
            }
            if let Err(e) = wet_stream.finalize() {
                eprintln!("warning: wet recording not finalized: {e}");
            }
            (di, wet)
        })
        .expect("spawn recorder disk thread")
}

/// Drain a ring into `scratch` and write it. `scratch` is reused across calls;
/// growth happens off the audio thread, so allocation is fine.
fn drain_write(
    cons: &mut rtrb::Consumer<f32>,
    stream: &mut WavStream<std::io::BufWriter<std::fs::File>>,
    scratch: &mut Vec<f32>,
) {
    let n = cons.slots();
    if n == 0 {
        return;
    }
    scratch.clear();
    if let Ok(chunk) = cons.read_chunk(n) {
        let (a, b) = chunk.as_slices();
        scratch.extend_from_slice(a);
        scratch.extend_from_slice(b);
        chunk.commit_all();
    }
    if let Err(e) = stream.write(scratch) {
        eprintln!("warning: recording write failed: {e}");
    }
}

/// UTC `YYYYMMDD-HHMMSS` for take file names. Control-thread only (reads the
/// wall clock, never the RT path). Dependency-free civil-from-days conversion.
fn timestamp_utc() -> String {
    let secs = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let days = (secs / 86_400) as i64;
    let sod = secs % 86_400;
    let (h, m, s) = (sod / 3_600, (sod % 3_600) / 60, sod % 60);
    let (y, mo, d) = civil_from_days(days);
    format!("{y:04}{mo:02}{d:02}-{h:02}{m:02}{s:02}")
}

/// Days since 1970-01-01 → (year, month, day) in the proleptic Gregorian
/// calendar (Howard Hinnant's algorithm). UTC — the file name's time zone.
fn civil_from_days(z: i64) -> (i64, u32, u32) {
    let z = z + 719_468;
    let era = (if z >= 0 { z } else { z - 146_096 }) / 146_097;
    let doe = z - era * 146_097; // [0, 146096]
    let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365; // [0, 399]
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100); // [0, 365]
    let mp = (5 * doy + 2) / 153; // [0, 11]
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32; // [1, 31]
    let m = (if mp < 10 { mp + 3 } else { mp - 9 }) as u32; // [1, 12]
    (if m <= 2 { y + 1 } else { y }, m, d)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn timestamp_is_well_formed() {
        let ts = timestamp_utc();
        assert_eq!(ts.len(), 15, "YYYYMMDD-HHMMSS");
        assert_eq!(&ts[8..9], "-");
    }

    #[test]
    fn civil_epoch_is_1970() {
        assert_eq!(civil_from_days(0), (1970, 1, 1));
        // 2000-03-01 is 11017 days after the epoch.
        assert_eq!(civil_from_days(11_017), (2000, 3, 1));
        // 2026-07-21.
        let days = 20_655; // days from 1970-01-01 to 2026-07-21
        assert_eq!(civil_from_days(days), (2026, 7, 21));
    }

    #[test]
    fn ring_capacity_is_stereo_seconds() {
        assert_eq!(Recorder::ring_capacity(48_000), 48_000 * 2 * 2);
    }

    /// Full lifecycle: start → push interleaved stereo onto both taps → stop →
    /// the WAVs round-trip the exact samples in the exact interleaving.
    #[test]
    fn records_di_and_wet_and_round_trips() {
        let (mut di_p, di_c) = rtrb::RingBuffer::new(48_000 * 2);
        let (mut wet_p, wet_c) = rtrb::RingBuffer::new(48_000 * 2);
        let state = Arc::new(RecordTapState::default());
        let dir = std::env::temp_dir().join(format!("lion-heart-rectest-{}", std::process::id()));
        let mut rec = Recorder::new(
            di_c,
            wet_c,
            Arc::clone(&state),
            48_000,
            dir.clone(),
            WavBits::Float32,
        );

        let (di_path, wet_path) = rec.start().unwrap();
        assert!(rec.is_recording());
        assert!(state.armed.load(Ordering::Relaxed), "start arms the taps");

        // Simulate the audio thread's StereoTap: DI = a ramp, wet = its negative,
        // both stereo interleaved.
        let frames = 2_000;
        let mut di_inter = Vec::new();
        let mut wet_inter = Vec::new();
        for f in 0..frames {
            let v = f as f32 / frames as f32;
            di_inter.push(v);
            di_inter.push(v);
            wet_inter.push(-v);
            wet_inter.push(-v);
        }
        for &x in &di_inter {
            di_p.push(x).unwrap();
        }
        for &x in &wet_inter {
            wet_p.push(x).unwrap();
        }

        let summary = rec.stop().unwrap();
        assert!(!rec.is_recording());
        assert!(
            !state.armed.load(Ordering::Relaxed),
            "stop disarms the taps"
        );
        assert_eq!(summary.dropped, 0, "the roomy ring dropped nothing");

        let di = lh_assets::wav::read(&di_path).unwrap();
        let wet = lh_assets::wav::read(&wet_path).unwrap();
        assert_eq!(di.channels, 2);
        assert_eq!(di.sample_rate, 48_000);
        assert_eq!(di.samples, di_inter, "DI round-trips exactly, interleaved");
        assert_eq!(
            wet.samples, wet_inter,
            "wet round-trips exactly, interleaved"
        );

        let _ = std::fs::remove_file(&di_path);
        let _ = std::fs::remove_file(&wet_path);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
