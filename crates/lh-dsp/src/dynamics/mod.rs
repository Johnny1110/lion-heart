//! Dynamics processors — level in, level out: the noise gate, the
//! compressor, and the output safety limiter.

pub mod comp;
pub mod gate;
pub mod limiter;

pub use comp::Compressor;
pub use gate::NoiseGate;
pub use limiter::Limiter;
