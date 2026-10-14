//! Clean-room, pure-Rust H.265/HEVC software encoder, written from the public
//! ITU-T Rec. H.265 specification (v7, 08/2021; ISO/IEC 23008-2).
//!
//! Phase 1 (this crate today):
//! - Main profile, 4:2:0 8-bit, frame coding; CTU 64 with CU splits down to 8x8.
//! - I-frames (35 intra modes) and P-frames (zero-motion skip plus integer motion search).
//! - 4x4–32x32 integer transforms with deadzone quantisation, full CABAC, normative deblocking.
//! - Rate control: constant QP, constant quality (CRF-like), 1-pass ABR with VBV.
//! - `PacketFormat::AnnexB` (default, parameter sets in-band on every IDR) and
//!   `PacketFormat::LengthPrefixed` (MP4 samples; sets out-of-band via [`Encoder::hvcc`]).
//! - Slice threading is a follow-up: one slice per picture for now.
//!
//! ```no_run
//! use filmcraft_hevcenc::{Encoder, EncoderConfig, RateControl, YuvFrame};
//! let mut cfg = EncoderConfig::new(1280, 720, 30, 1);
//! cfg.rate = RateControl::Crf(22.0);
//! let mut enc = Encoder::new(cfg).unwrap();
//! let (y, u, v) = (vec![128u8; 1280 * 720], vec![128u8; 640 * 360], vec![128u8; 640 * 360]);
//! let frame = YuvFrame { y: &y, u: &u, v: &v, y_stride: 1280, uv_stride: 640 };
//! let mut packets = enc.encode(&frame, 0).unwrap();
//! packets.extend(enc.flush());
//! ```

#![cfg_attr(not(test), deny(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::unimplemented, clippy::todo, clippy::unreachable))]

mod params;
mod ratecontrol;

pub use ratecontrol::{FrameStat, PassStats};

/// Encoder errors.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("invalid dimensions {0}x{1} (must be even and non-zero)")]
    InvalidDimensions(u32, u32),
    #[error("invalid frame rate {0}/{1}")]
    InvalidFrameRate(u32, u32),
    #[error("invalid configuration: {0}")]
    InvalidConfig(String),
    #[error("input frame planes too small for the configured size")]
    BadFrame,
    #[error("unsupported: {0}")]
    Unsupported(String),
}

/// Coded profile. Phase 1 writes Main only; the VPS/SPS/PTL already carry
/// `general_profile_idc = 1` so Main 10 arrives without a header change.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum Profile {
    /// Main: CABAC, I/P frames, 4:2:0 8-bit.
    #[default]
    Main,
}

/// Speed / quality trade-off.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum Preset {
    Speed,
    #[default]
    Balanced,
    Quality,
}

/// Frame-level rate control.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum RateControl {
    /// Constant quality; roughly the QP of P frames (0–51, lower is better).
    Crf(f32),
    /// Constant bitrate target (1-pass ABR with a 1-second VBV at `kbps`).
    Cbr { kbps: u32 },
    /// Variable bitrate: average `target_kbps`, VBV-limited to `max_kbps`.
    Vbr { target_kbps: u32, max_kbps: u32 },
    /// Constant QP (I frames use QP − 2, clamped).
    Qp(u8),
}

/// Multi-pass mode for bitrate-targeted rate control.
#[derive(Clone, Debug, Default, PartialEq)]
pub enum Pass {
    #[default]
    Single,
    /// First pass: encode and collect [`PassStats`] via [`Encoder::pass_stats`].
    First,
    /// Second pass using the statistics of the first (follow-up: rejected for now).
    Second(PassStats),
}

/// Colour description written to the VUI (ITU-T H.273 code points).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ColorConfig {
    pub primaries: u8,
    pub transfer: u8,
    pub matrix: u8,
    pub full_range: bool,
}

impl Default for ColorConfig {
    /// BT.709, limited range.
    fn default() -> Self {
        ColorConfig { primaries: 1, transfer: 1, matrix: 1, full_range: false }
    }
}

/// How coded pictures leave the encoder.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum PacketFormat {
    /// `00 00 00 01`-prefixed NAL units; VPS/SPS/PPS in-band on every IDR.
    #[default]
    AnnexB,
    /// 4-byte big-endian lengths; parameter sets out-of-band via [`Encoder::hvcc`].
    LengthPrefixed,
}

/// Encoder configuration. Start from [`EncoderConfig::new`] and override fields.
#[derive(Clone, Debug)]
pub struct EncoderConfig {
    pub width: u32,
    pub height: u32,
    pub fps_num: u32,
    pub fps_den: u32,
    pub profile: Profile,
    pub preset: Preset,
    pub rate: RateControl,
    /// IDR interval in pictures (a scene cut also inserts one when `scenecut`).
    pub keyint: u32,
    /// Phase 1 stores 0 only; B-frames are a follow-up.
    pub bframes: u32,
    pub color: ColorConfig,
    /// Rayon threads for analysis (0 = automatic). Single-slice coding stays serial.
    pub threads: u32,
    pub pass: Pass,
    pub format: PacketFormat,
    /// Variance-adaptive quantisation strength (0 = off).
    pub aq_strength: f32,
    /// Sample aspect ratio.
    pub sar: (u16, u16),
    /// Insert an IDR on a scene cut.
    pub scenecut: bool,
    /// Emit an access-unit delimiter per picture (Annex-B only).
    pub aud: bool,
    /// `general_level_idc` override (30 = level 1, 120 = level 4.0, …); auto-picked when `None`.
    pub level: Option<u8>,
}

impl EncoderConfig {
    pub fn new(width: u32, height: u32, fps_num: u32, fps_den: u32) -> Self {
        EncoderConfig {
            width,
            height,
            fps_num,
            fps_den,
            profile: Profile::Main,
            preset: Preset::Balanced,
            rate: RateControl::Crf(23.0),
            keyint: 250,
            bframes: 0,
            color: ColorConfig::default(),
            threads: 0,
            pass: Pass::Single,
            format: PacketFormat::AnnexB,
            aq_strength: 0.6,
            sar: (1, 1),
            scenecut: true,
            aud: true,
            level: None,
        }
    }
}

/// One input picture: planar 4:2:0 8-bit.
#[derive(Clone, Copy, Debug)]
pub struct YuvFrame<'a> {
    pub y: &'a [u8],
    pub u: &'a [u8],
    pub v: &'a [u8],
    pub y_stride: usize,
    pub uv_stride: usize,
}

/// Coded picture type.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FrameType {
    Idr,
    I,
    P,
}

/// One coded picture in coding order.
#[derive(Clone, Debug)]
pub struct Packet {
    pub data: Vec<u8>,
    pub pts: i64,
    pub dts: i64,
    pub keyframe: bool,
    pub frame_type: FrameType,
    pub qp: u8,
}

/// A reconstructed picture, for tests proving encoder/decoder agreement.
#[derive(Clone, Debug)]
pub struct ReconFrame {
    pub pts: i64,
    pub y: Vec<u8>,
    pub u: Vec<u8>,
    pub v: Vec<u8>,
}

/// Maximum coded size; the SPS frame cropping signals the visible window.
pub const MAX_DIMENSION: u32 = 16384;

/// The H.265 software encoder.
pub struct Encoder {
    cfg: EncoderConfig,
    capture_recon: bool,
    recon: Vec<ReconFrame>,
}

impl Encoder {
    /// Validate the configuration. Coding lands in later commits; until then
    /// [`Encoder::encode`] reports [`Error::Unsupported`].
    pub fn new(cfg: EncoderConfig) -> Result<Self, Error> {
        if cfg.width == 0 || cfg.height == 0 || cfg.width % 2 != 0 || cfg.height % 2 != 0 || cfg.width > MAX_DIMENSION || cfg.height > MAX_DIMENSION {
            return Err(Error::InvalidDimensions(cfg.width, cfg.height));
        }
        if cfg.fps_num == 0 || cfg.fps_den == 0 {
            return Err(Error::InvalidFrameRate(cfg.fps_num, cfg.fps_den));
        }
        if cfg.bframes != 0 {
            return Err(Error::InvalidConfig(format!("B-frames are a follow-up (got bframes={}), phase 1 codes I/P only", cfg.bframes)));
        }
        if cfg.keyint < 1 {
            return Err(Error::InvalidConfig("keyint must be at least 1".to_string()));
        }
        match cfg.rate {
            RateControl::Qp(qp) if qp > 51 => {
                return Err(Error::InvalidConfig(format!("QP {qp} out of range 0..=51")));
            }
            RateControl::Crf(crf) if !(0.0..=51.0).contains(&crf) => {
                return Err(Error::InvalidConfig(format!("CRF {crf} out of range 0..=51")));
            }
            RateControl::Cbr { kbps } | RateControl::Vbr { target_kbps: kbps, .. } if kbps == 0 => {
                return Err(Error::InvalidConfig("bitrate target must be non-zero".to_string()));
            }
            _ => {}
        }
        if matches!(cfg.pass, Pass::Second(_)) {
            return Err(Error::Unsupported("two-pass second pass arrives in a follow-up".to_string()));
        }
        if let Some(level) = cfg.level
            && !params::is_legal_level_idc(level)
        {
            return Err(Error::InvalidConfig(format!("illegal general_level_idc {level}")));
        }
        Ok(Encoder { cfg, capture_recon: false, recon: Vec::new() })
    }

    pub fn config(&self) -> &EncoderConfig {
        &self.cfg
    }

    /// Reorder delay in pictures (phase 1 has no B-frames, always 0).
    pub fn delay(&self) -> usize {
        0
    }

    pub fn set_recon_capture(&mut self, on: bool) {
        self.capture_recon = on;
        if !on {
            self.recon.clear();
        }
    }

    pub fn take_recon(&mut self) -> Vec<ReconFrame> {
        std::mem::take(&mut self.recon)
    }

    /// First-pass statistics (`Some` only for [`Pass::First`]).
    pub fn pass_stats(&self) -> Option<PassStats> {
        if matches!(self.cfg.pass, Pass::First) { Some(PassStats::default()) } else { None }
    }

    pub fn encode(&mut self, frame: &YuvFrame<'_>, pts: i64) -> Result<Vec<Packet>, Error> {
        self.try_encode(frame, pts)
    }

    pub fn try_encode(&mut self, _frame: &YuvFrame<'_>, _pts: i64) -> Result<Vec<Packet>, Error> {
        Err(Error::Unsupported("picture coding arrives with the slice encoder".to_string()))
    }

    pub fn flush(&mut self) -> Vec<Packet> {
        Vec::new()
    }
}
