//! VPS/SPS/PPS writers and the `hvcC` box payload (ITU-T H.265 §7.3, ISO/IEC 14496-15).
//!
//! Phase 1 writes Main-profile, 8-bit, 4:2:0 parameter sets for frame coding with
//! a single reference picture and no tiles, WPP, SAO or scaling lists.

/// Legal `general_level_idc` values (H.265 Table A.1: levels 1–6.2, `30 * level`).
pub fn is_legal_level_idc(idc: u8) -> bool {
    matches!(idc, 30 | 60 | 63 | 90 | 93 | 120 | 123 | 150 | 153 | 156 | 180 | 183 | 186 | 210)
}
