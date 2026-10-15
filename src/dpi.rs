//! DPI helper functions, validation, snapping, and esports presets

pub const MIN_DPI: u32 = 50;
pub const MAX_DPI: u32 = 26000;
pub const DPI_STEP: u32 = 50;

pub const QUICK_DPI_LIST: [u32; 7] = [400, 800, 1200, 1600, 2400, 3200, 6400];

pub const PRESET_FPS_CLASSIC: [u32; 5] = [400, 800, 1600, 3200, 6400];
pub const PRESET_MOBA_SPEED: [u32; 5] = [800, 1200, 1600, 2400, 3200];
pub const PRESET_HIGH_RES_4K: [u32; 5] = [1200, 1600, 2400, 3200, 6400];

/// Clamps value to 50..=26000 and snaps to the nearest multiple of 50
pub fn clamp_and_snap_dpi(val: u32) -> u32 {
    let clamped = val.clamp(MIN_DPI, MAX_DPI);
    let snapped = ((clamped + (DPI_STEP / 2)) / DPI_STEP) * DPI_STEP;
    snapped.clamp(MIN_DPI, MAX_DPI)
}

/// Increases or decreases DPI by delta, then clamps to bounds
pub fn adjust_dpi_step(val: u32, delta: i32) -> u32 {
    let current = clamp_and_snap_dpi(val) as i32;
    let new_val = current + delta;
    if new_val < MIN_DPI as i32 {
        MIN_DPI
    } else if new_val > MAX_DPI as i32 {
        MAX_DPI
    } else {
        clamp_and_snap_dpi(new_val as u32)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_clamp_and_snap_dpi_bounds() {
        assert_eq!(clamp_and_snap_dpi(10), MIN_DPI);
        assert_eq!(clamp_and_snap_dpi(30000), MAX_DPI);
    }

    #[test]
    fn test_clamp_and_snap_dpi_rounding() {
        assert_eq!(clamp_and_snap_dpi(800), 800);
        assert_eq!(clamp_and_snap_dpi(823), 800);
        assert_eq!(clamp_and_snap_dpi(825), 850);
        assert_eq!(clamp_and_snap_dpi(849), 850);
    }

    #[test]
    fn test_adjust_dpi_step() {
        assert_eq!(adjust_dpi_step(800, 50), 850);
        assert_eq!(adjust_dpi_step(800, -50), 750);
        assert_eq!(adjust_dpi_step(50, -50), 50); // Cannot go below 50
        assert_eq!(adjust_dpi_step(26000, 50), 26000); // Cannot exceed 26000
    }
}
