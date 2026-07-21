//! Practice tools (PRD 019) — monitor/aux sources, **not** chain effects.
//!
//! These generate audio that is mixed into the output *after* the amp chain and
//! the safety limiter (the engine's aux lane): a click/backing track must not
//! be crushed by the guitar's tone. They therefore do not implement [`Effect`]
//! — they are stereo (or mono) generators the app's player thread renders into
//! the aux ring, off the audio thread.
//!
//! Phase 1 ships the [`Metronome`]; the drum groove and song player (WSOLA
//! time-stretch, `symphonia` decode) land here in later phases.
//!
//! [`Effect`]: crate::Effect

mod groove;
mod metronome;
mod song;
mod wsola;

pub use groove::{DrumMachine, pattern_count, pattern_index, pattern_name};
pub use metronome::Metronome;
pub use song::{SongBuffer, SongPlayer};
pub use wsola::Wsola;
