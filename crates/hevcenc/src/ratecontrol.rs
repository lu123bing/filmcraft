//! Frame-level rate control: constant QP, constant quality and 1-pass ABR with VBV.
//!
//! Phase 1 carries the configuration types and first-pass statistics; the closed-loop
//! controller arrives with the slice encoder.

/// Per-picture statistics recorded by a [`Pass::First`](crate::Pass::First) run.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct FrameStat {
    /// 0 = I/IDR, 1 = P.
    pub slice_type: u8,
    pub qscale: f32,
    pub bits: u64,
}

/// First-pass statistics, serialised between runs for two-pass VBR (follow-up).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct PassStats {
    pub frames: Vec<FrameStat>,
}
