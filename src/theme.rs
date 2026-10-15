//! Theme system for Native Low-Memory GUI (Obsidian Purple vs Win11 Fluent)

#[inline]
pub const fn rgb(r: u8, g: u8, b: u8) -> u32 {
    (r as u32) | ((g as u32) << 8) | ((b as u32) << 16)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ThemeMode {
    Obsidian,
    Fluent,
}

impl ThemeMode {
    pub fn as_str(&self) -> &'static str {
        match self {
            ThemeMode::Obsidian => "obsidian",
            ThemeMode::Fluent => "fluent",
        }
    }

    pub fn from_str(s: &str) -> Self {
        if s.eq_ignore_ascii_case("fluent") {
            ThemeMode::Fluent
        } else {
            ThemeMode::Obsidian
        }
    }

    pub fn toggle(&self) -> Self {
        match self {
            ThemeMode::Obsidian => ThemeMode::Fluent,
            ThemeMode::Fluent => ThemeMode::Obsidian,
        }
    }
}

#[derive(Debug, Clone)]
#[allow(dead_code)]
pub struct ThemeColors {
    pub bg: u32,             // 0x00BBGGRR (Win32 BGR format)
    pub card_bg: u32,
    pub card_border: u32,
    pub card_active_bg: u32,
    pub card_active_border: u32,
    pub accent: u32,
    pub accent_hover: u32,
    pub accent_cyan: u32,
    pub text_primary: u32,
    pub text_secondary: u32,
    pub text_accent: u32,
    pub button_bg: u32,
    pub button_hover: u32,
    pub button_active: u32,
    pub footer_bg: u32,
}

impl ThemeColors {
    pub fn for_mode(mode: ThemeMode) -> Self {
        match mode {
            ThemeMode::Obsidian => Self {
                bg: rgb(16, 14, 22),
                card_bg: rgb(24, 22, 32),
                card_border: rgb(48, 44, 62),
                card_active_bg: rgb(38, 25, 54),
                card_active_border: rgb(168, 85, 247),
                accent: rgb(147, 51, 234),
                accent_hover: rgb(168, 85, 247),
                accent_cyan: rgb(0, 245, 212),
                text_primary: rgb(245, 245, 250),
                text_secondary: rgb(150, 150, 165),
                text_accent: rgb(216, 180, 254),
                button_bg: rgb(34, 32, 44),
                button_hover: rgb(46, 44, 60),
                button_active: rgb(147, 51, 234),
                footer_bg: rgb(20, 18, 28),
            },
            ThemeMode::Fluent => Self {
                bg: rgb(32, 32, 32),
                card_bg: rgb(43, 43, 43),
                card_border: rgb(58, 58, 58),
                card_active_bg: rgb(48, 54, 64),
                card_active_border: rgb(96, 205, 255),
                accent: rgb(0, 120, 212),
                accent_hover: rgb(96, 205, 255),
                accent_cyan: rgb(96, 205, 255),
                text_primary: rgb(255, 255, 255),
                text_secondary: rgb(160, 160, 160),
                text_accent: rgb(96, 205, 255),
                button_bg: rgb(48, 48, 48),
                button_hover: rgb(60, 60, 60),
                button_active: rgb(0, 120, 212),
                footer_bg: rgb(26, 26, 26),
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_theme_mode_parsing() {
        assert_eq!(ThemeMode::from_str("obsidian"), ThemeMode::Obsidian);
        assert_eq!(ThemeMode::from_str("fluent"), ThemeMode::Fluent);
        assert_eq!(ThemeMode::from_str("unknown"), ThemeMode::Obsidian); // fallback
        assert_eq!(ThemeMode::Obsidian.as_str(), "obsidian");
        assert_eq!(ThemeMode::Fluent.as_str(), "fluent");
    }

    #[test]
    fn test_theme_toggle() {
        let m1 = ThemeMode::Obsidian;
        let m2 = m1.toggle();
        assert_eq!(m2, ThemeMode::Fluent);
        let m3 = m2.toggle();
        assert_eq!(m3, ThemeMode::Obsidian);
    }

    #[test]
    fn test_theme_colors_different_for_modes() {
        let obsidian = ThemeColors::for_mode(ThemeMode::Obsidian);
        let fluent = ThemeColors::for_mode(ThemeMode::Fluent);
        assert_ne!(obsidian.bg, 0);
        assert_ne!(fluent.bg, 0);
        assert_ne!(obsidian.bg, fluent.bg);
        assert_ne!(obsidian.accent, fluent.accent);
    }
}
