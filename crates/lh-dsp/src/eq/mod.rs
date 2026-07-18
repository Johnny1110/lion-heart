//! Equalizers: the in-chain 3-band tone-shaping pedal ([`chain`]) and the
//! 8-band parametric EQ that lives on the engine's fixed output stage
//! ([`global`], PRD 003).

pub mod chain;
pub mod global;

pub use chain::Eq;
pub use global::GlobalEq;
