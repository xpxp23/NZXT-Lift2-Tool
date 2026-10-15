use crate::config::MouseConfig;

pub const REPORT_ID: u8 = 0x43;
pub const REPORT_SIZE: usize = 64;

pub const CMD_GET_VERSION: u8 = 0x01;
pub const CMD_GET_UID: u8 = 0x03;
#[allow(dead_code)]
pub const CMD_GET_PROFILE: u8 = 0x44;
#[allow(dead_code)]
pub const CMD_SET_PROFILE: u8 = 0x45;
#[allow(dead_code)]
pub const CMD_GET_MAX_STAGES: u8 = 0x90;
pub const CMD_GET_PROFILE_SETTINGS: u8 = 0x92;
pub const CMD_SET_PROFILE_SETTINGS: u8 = 0x93;
pub const CMD_GET_ACTIVE_CPI: u8 = 0x9A;
pub const CMD_SET_ACTIVE_CPI: u8 = 0x9B;

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum PollingRate {
    Hz8000 = 0,
    Hz4000 = 1,
    Hz2000 = 2,
    Hz1000 = 3,
    Hz500 = 4,
    Hz250 = 5,
    Hz125 = 6,
}

impl PollingRate {
    pub fn to_hz(&self) -> u32 {
        match self {
            Self::Hz8000 => 8000,
            Self::Hz4000 => 4000,
            Self::Hz2000 => 2000,
            Self::Hz1000 => 1000,
            Self::Hz500 => 500,
            Self::Hz250 => 250,
            Self::Hz125 => 125,
        }
    }

    pub fn to_delay_str(&self) -> &'static str {
        match self {
            Self::Hz8000 => "0.125 ms",
            Self::Hz4000 => "0.25 ms",
            Self::Hz2000 => "0.5 ms",
            Self::Hz1000 => "1.0 ms",
            Self::Hz500 => "2.0 ms",
            Self::Hz250 => "4.0 ms",
            Self::Hz125 => "8.0 ms",
        }
    }

    pub fn from_hz(hz: u32) -> Self {
        match hz {
            8000 => Self::Hz8000,
            4000 => Self::Hz4000,
            2000 => Self::Hz2000,
            1000 => Self::Hz1000,
            500 => Self::Hz500,
            250 => Self::Hz250,
            125 => Self::Hz125,
            _ => Self::Hz1000,
        }
    }

    pub fn to_raw(&self) -> u8 {
        *self as u8
    }

    pub fn from_raw(raw: u8) -> Self {
        match raw {
            0 => Self::Hz8000,
            1 => Self::Hz4000,
            2 => Self::Hz2000,
            3 => Self::Hz1000,
            4 => Self::Hz500,
            5 => Self::Hz250,
            6 => Self::Hz125,
            _ => Self::Hz1000,
        }
    }

    /// Ordered strictly from highest to lowest polling rate for consistent UI presentation
    pub const ALL: [PollingRate; 7] = [
        PollingRate::Hz8000,
        PollingRate::Hz4000,
        PollingRate::Hz2000,
        PollingRate::Hz1000,
        PollingRate::Hz500,
        PollingRate::Hz250,
        PollingRate::Hz125,
    ];
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum LiftOffDistance {
    Mm1 = 0, // 1 mm (Low)
    Mm2 = 1, // 2 mm (High)
}

impl LiftOffDistance {
    pub fn to_mm(&self) -> u32 {
        match self {
            Self::Mm1 => 1,
            Self::Mm2 => 2,
        }
    }

    pub fn to_label(&self) -> &'static str {
        match self {
            Self::Mm1 => "1 mm (Low / 竞技默认)",
            Self::Mm2 => "2 mm (High / 厚脚贴)",
        }
    }

    pub fn from_mm(mm: u32) -> Self {
        if mm >= 2 {
            Self::Mm2
        } else {
            Self::Mm1
        }
    }

    pub fn to_raw(&self) -> u8 {
        *self as u8
    }

    pub fn from_raw(raw: u8) -> Self {
        if raw >= 1 {
            Self::Mm2
        } else {
            Self::Mm1
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Lift2Settings {
    pub slot: u8,
    pub polling_rate: PollingRate,
    pub lod: LiftOffDistance,
    pub stage_count: u8,
    pub active_cpi_idx: u8, // 0..4
    pub stages: [u32; 5],   // e.g. [400, 800, 1600, 3200, 6400]
}

impl Default for Lift2Settings {
    fn default() -> Self {
        Self {
            slot: 0,
            polling_rate: PollingRate::Hz1000,
            lod: LiftOffDistance::Mm1,
            stage_count: 5,
            active_cpi_idx: 2, // default 1600 DPI (Stage 3)
            stages: [400, 800, 1600, 3200, 6400],
        }
    }
}

impl Lift2Settings {
    pub fn from_mouse_config(cfg: &MouseConfig) -> Self {
        let active_idx = cfg.active_stage.clamp(1, 5).saturating_sub(1);
        let mut stages = cfg.stages;
        for s in &mut stages {
            *s = crate::dpi::clamp_and_snap_dpi(*s);
        }
        Self {
            slot: 0,
            polling_rate: PollingRate::from_hz(cfg.polling_rate),
            lod: LiftOffDistance::from_mm(cfg.lod),
            stage_count: 5,
            active_cpi_idx: active_idx,
            stages,
        }
    }
}

pub fn dpi_to_raw(dpi: u32) -> u16 {
    // Formula: (dpi - 50) / 50
    let clamped = dpi.clamp(50, 26000);
    ((clamped.saturating_sub(50)) / 50) as u16
}

pub fn raw_to_dpi(raw: u16) -> u32 {
    // Formula: raw * 50 + 50
    ((raw as u32) * 50 + 50).clamp(50, 26000)
}

/// Builds a 64-byte output report buffer for NZXT Lift 2
pub fn build_packet(opcode: u8, payload: &[u8]) -> [u8; REPORT_SIZE] {
    let mut buf = [0u8; REPORT_SIZE];
    buf[0] = REPORT_ID;
    buf[1] = (payload.len() as u8 + 1) | 0x80;
    buf[2] = 0x00; // seq
    buf[3] = opcode;
    for (i, &b) in payload.iter().enumerate() {
        if 4 + i < REPORT_SIZE {
            buf[4 + i] = b;
        }
    }
    buf
}

/// Safely extracts the active CPI stage index (0..=4) from a CMD_GET_ACTIVE_CPI response packet
pub fn parse_active_cpi_idx(packet: &[u8]) -> Option<u8> {
    if packet.len() < 5 || packet[0] != REPORT_ID {
        return None;
    }
    // Standard payload offset is byte 4 when byte 3 echoes CMD_GET_ACTIVE_CPI (0x9A)
    if packet[3] == CMD_GET_ACTIVE_CPI && packet[4] <= 4 {
        return Some(packet[4]);
    }
    // Fallback for firmware variants that place the stage index directly at byte 3
    if packet[3] <= 4 {
        return Some(packet[3]);
    }
    if packet[4] <= 4 {
        return Some(packet[4]);
    }
    None
}

/// Parses the data packet returned by CMD_GET_PROFILE_SETTINGS (0x92)
/// Layout of data packet:
/// Byte 4: slot (0..4)
/// Byte 5: polling_rate (0: 8000Hz, 1: 4000Hz, 2: 2000Hz, 3: 1000Hz, 4: 500Hz, 5: 250Hz, 6: 125Hz)
/// Byte 6: lod (0: 1mm, 1: 2mm)
/// Byte 7: stage_count (5)
/// Byte 8: active_cpi_idx (0..4)
/// Byte 9..10: cpi_0 (big-endian raw)
/// Byte 11..12: cpi_1 (big-endian raw)
/// Byte 13..14: cpi_2 (big-endian raw)
/// Byte 15..16: cpi_3 (big-endian raw)
/// Byte 17..18: cpi_4 (big-endian raw)
/// Byte 19: 0xFF
pub fn parse_profile_settings(packet: &[u8]) -> Result<Lift2Settings, String> {
    if packet.len() < 19 {
        return Err(format!("Settings packet too short: {} bytes", packet.len()));
    }
    if packet[0] != REPORT_ID {
        return Err(format!("Invalid report ID: 0x{:02X}", packet[0]));
    }

    let slot = packet[4].min(4);
    let polling_rate = PollingRate::from_raw(packet[5]);
    let lod = LiftOffDistance::from_raw(packet[6]);
    let stage_count = packet[7].clamp(1, 5);
    let active_cpi_idx = packet[8].min(4);

    let mut stages = [400u32; 5];
    for i in 0..5 {
        let offset = 9 + i * 2;
        if offset + 1 < packet.len() {
            let raw = u16::from_be_bytes([packet[offset], packet[offset + 1]]);
            stages[i] = raw_to_dpi(raw);
        }
    }

    Ok(Lift2Settings {
        slot,
        polling_rate,
        lod,
        stage_count,
        active_cpi_idx,
        stages,
    })
}

/// Encodes payload for CMD_SET_PROFILE_SETTINGS (0x93)
pub fn encode_profile_settings(settings: &Lift2Settings) -> Vec<u8> {
    let mut payload = Vec::with_capacity(16);
    payload.push(settings.slot.min(4));
    payload.push(settings.polling_rate.to_raw());
    payload.push(settings.lod.to_raw());
    payload.push(settings.stage_count.clamp(1, 5));
    payload.push(settings.active_cpi_idx.min(4));
    for &dpi in &settings.stages {
        let raw = dpi_to_raw(dpi);
        payload.extend_from_slice(&raw.to_be_bytes());
    }
    payload.push(0xFF);
    payload
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_polling_rate_all_descending() {
        let hz_list: Vec<u32> = PollingRate::ALL.iter().map(|r| r.to_hz()).collect();
        assert_eq!(hz_list, vec![8000, 4000, 2000, 1000, 500, 250, 125]);
    }

    #[test]
    fn test_parse_active_cpi_idx_variants() {
        let mut pkt_std = [0u8; REPORT_SIZE];
        pkt_std[0] = REPORT_ID;
        pkt_std[3] = CMD_GET_ACTIVE_CPI;
        pkt_std[4] = 3;
        assert_eq!(parse_active_cpi_idx(&pkt_std), Some(3));

        let mut pkt_legacy = [0u8; REPORT_SIZE];
        pkt_legacy[0] = REPORT_ID;
        pkt_legacy[3] = 2;
        pkt_legacy[4] = 0xFF;
        assert_eq!(parse_active_cpi_idx(&pkt_legacy), Some(2));

        let mut pkt_invalid = [0u8; REPORT_SIZE];
        pkt_invalid[0] = REPORT_ID;
        pkt_invalid[3] = CMD_GET_ACTIVE_CPI;
        pkt_invalid[4] = 0xFF;
        assert_eq!(parse_active_cpi_idx(&pkt_invalid), None);
    }

    #[test]
    fn test_parse_profile_settings_clamping() {
        let mut pkt = [0u8; REPORT_SIZE];
        pkt[0] = REPORT_ID;
        pkt[3] = CMD_GET_PROFILE_SETTINGS;
        pkt[4] = 9;  // out-of-range slot
        pkt[5] = 0;  // 8000Hz
        pkt[6] = 1;  // 2mm
        pkt[7] = 99; // out-of-range stage count
        pkt[8] = 42; // out-of-range active_cpi_idx
        let parsed = parse_profile_settings(&pkt).expect("Should parse and clamp");
        assert_eq!(parsed.slot, 4);
        assert_eq!(parsed.stage_count, 5);
        assert_eq!(parsed.active_cpi_idx, 4);
    }
}
