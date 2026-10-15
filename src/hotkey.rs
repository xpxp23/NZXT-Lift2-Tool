//! Global Hotkey Parser, Formatter, and Validator for Windows Win32 API

pub const MOD_ALT: u32 = 0x0001;
pub const MOD_CONTROL: u32 = 0x0002;
pub const MOD_SHIFT: u32 = 0x0004;
pub const MOD_WIN: u32 = 0x0008;
pub const MOD_NOREPEAT: u32 = 0x4000;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParsedHotkey {
    pub modifiers: u32,
    pub vk_code: u32,
    pub display_name: String,
}

pub fn parse_hotkey(s: &str) -> Result<ParsedHotkey, String> {
    let trimmed = s.trim();
    if trimmed.is_empty() {
        return Err("快捷键不能为空 (Hotkey cannot be empty)".to_string());
    }

    let parts: Vec<&str> = trimmed
        .split('+')
        .map(|p| p.trim())
        .filter(|p| !p.is_empty())
        .collect();
    if parts.is_empty() {
        return Err("无效快捷键格式 (Invalid hotkey format)".to_string());
    }

    let mut modifiers = 0u32;
    let mut key_part: Option<&str> = None;
    let mut modifier_names = Vec::new();

    for part in parts {
        let upper = part.to_ascii_uppercase();
        match upper.as_str() {
            "CTRL" | "CONTROL" => {
                if (modifiers & MOD_CONTROL) == 0 {
                    modifiers |= MOD_CONTROL;
                    modifier_names.push("Ctrl");
                }
            }
            "SHIFT" => {
                if (modifiers & MOD_SHIFT) == 0 {
                    modifiers |= MOD_SHIFT;
                    modifier_names.push("Shift");
                }
            }
            "ALT" => {
                if (modifiers & MOD_ALT) == 0 {
                    modifiers |= MOD_ALT;
                    modifier_names.push("Alt");
                }
            }
            "WIN" | "WINDOWS" | "SUPER" => {
                if (modifiers & MOD_WIN) == 0 {
                    modifiers |= MOD_WIN;
                    modifier_names.push("Win");
                }
            }
            _ => {
                if key_part.is_some() {
                    return Err(format!("只能指定一个主按键，发现了多余按键: '{}'", part));
                }
                key_part = Some(part);
            }
        }
    }

    let key_str = match key_part {
        Some(k) => k,
        None => return Err("缺少主按键，例如 F12 或 R (Missing main key)".to_string()),
    };

    let (vk, canonical_key_name) = parse_key(key_str)?;

    let mut display_parts = modifier_names;
    display_parts.push(&canonical_key_name);
    let display_name = display_parts.join("+");

    Ok(ParsedHotkey {
        modifiers,
        vk_code: vk,
        display_name,
    })
}

/// Converts a Win32 Virtual-Key code and active modifier states into a canonical hotkey string.
/// Returns `None` if `vk` is a standalone modifier key or unsupported key.
pub fn format_hotkey_from_vk(
    ctrl: bool,
    shift: bool,
    alt: bool,
    win: bool,
    vk: u32,
) -> Option<String> {
    // Ignore standalone modifier key presses
    if matches!(vk, 0x10..=0x12 | 0x5B..=0x5C | 0xA0..=0xA5) {
        return None;
    }

    let key_name = match vk {
        0x70..=0x87 => format!("F{}", vk - 0x70 + 1),
        0x41..=0x5A => ((vk as u8) as char).to_string(),
        0x30..=0x39 => ((vk as u8) as char).to_string(),
        0xC0 => "`".to_string(),
        0x20 => "Space".to_string(),
        0x09 => "Tab".to_string(),
        0x0D => "Enter".to_string(),
        0x08 => "Backspace".to_string(),
        0x2D => "Insert".to_string(),
        0x2E => "Delete".to_string(),
        0x24 => "Home".to_string(),
        0x23 => "End".to_string(),
        0x21 => "PageUp".to_string(),
        0x22 => "PageDown".to_string(),
        0x13 => "Pause".to_string(),
        0x2C => "PrintScreen".to_string(),
        0x91 => "ScrollLock".to_string(),
        _ => return None,
    };

    let mut parts = Vec::new();
    if ctrl {
        parts.push("Ctrl".to_string());
    }
    if shift {
        parts.push("Shift".to_string());
    }
    if alt {
        parts.push("Alt".to_string());
    }
    if win {
        parts.push("Win".to_string());
    }
    parts.push(key_name);
    Some(parts.join("+"))
}

fn parse_key(s: &str) -> Result<(u32, String), String> {
    let upper = s.to_ascii_uppercase();

    // Check F1..F24
    if upper.starts_with('F') {
        if let Ok(num) = upper[1..].parse::<u32>() {
            if (1..=24).contains(&num) {
                let vk = 0x70 + (num - 1);
                return Ok((vk, format!("F{}", num)));
            }
        }
    }

    // Single character A-Z or 0-9
    if upper.len() == 1 {
        let ch = upper.chars().next().unwrap();
        if ch.is_ascii_alphabetic() {
            return Ok((ch as u32, ch.to_string()));
        }
        if ch.is_ascii_digit() {
            return Ok((ch as u32, ch.to_string()));
        }
        if ch == '`' || ch == '~' {
            return Ok((0xC0, "`".to_string()));
        }
    }

    // Named special keys
    match upper.as_str() {
        "SPACE" => Ok((0x20, "Space".to_string())),
        "TAB" => Ok((0x09, "Tab".to_string())),
        "ESC" | "ESCAPE" => Ok((0x1B, "Esc".to_string())),
        "ENTER" | "RETURN" => Ok((0x0D, "Enter".to_string())),
        "BACKSPACE" => Ok((0x08, "Backspace".to_string())),
        "INSERT" | "INS" => Ok((0x2D, "Insert".to_string())),
        "DELETE" | "DEL" => Ok((0x2E, "Delete".to_string())),
        "HOME" => Ok((0x24, "Home".to_string())),
        "END" => Ok((0x23, "End".to_string())),
        "PAGEUP" | "PGUP" => Ok((0x21, "PageUp".to_string())),
        "PAGEDOWN" | "PGDN" => Ok((0x22, "PageDown".to_string())),
        "PAUSE" => Ok((0x13, "Pause".to_string())),
        "PRINTSCREEN" | "PRTSCN" => Ok((0x2C, "PrintScreen".to_string())),
        "SCROLLLOCK" => Ok((0x91, "ScrollLock".to_string())),
        _ => Err(format!("不支持或未知的按键: '{}'", s)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_default_hotkey_ctrl_shift_f12() {
        let parsed = parse_hotkey("Ctrl+Shift+F12").expect("Should parse default hotkey");
        assert_eq!(parsed.modifiers, MOD_CONTROL | MOD_SHIFT);
        assert_eq!(parsed.vk_code, 0x7B); // VK_F12 is 0x7B
        assert_eq!(parsed.display_name, "Ctrl+Shift+F12");
    }

    #[test]
    fn test_parse_hotkey_with_spaces_and_lowercase() {
        let parsed =
            parse_hotkey("ctrl + alt + r").expect("Should parse with spaces and lowercase");
        assert_eq!(parsed.modifiers, MOD_CONTROL | MOD_ALT);
        assert_eq!(parsed.vk_code, 'R' as u32);
        assert_eq!(parsed.display_name, "Ctrl+Alt+R");
    }

    #[test]
    fn test_parse_single_key_no_modifiers() {
        let parsed = parse_hotkey("F11").expect("Should parse F11");
        assert_eq!(parsed.modifiers, 0);
        assert_eq!(parsed.vk_code, 0x7A); // VK_F11 is 0x7A
    }

    #[test]
    fn test_parse_win_key() {
        let parsed = parse_hotkey("Win+Shift+F9").expect("Should parse Win key");
        assert_eq!(parsed.modifiers, MOD_WIN | MOD_SHIFT);
        assert_eq!(parsed.vk_code, 0x78); // VK_F9
    }

    #[test]
    fn test_format_hotkey_from_vk() {
        assert_eq!(format_hotkey_from_vk(true, false, false, false, 0x11), None);
        assert_eq!(
            format_hotkey_from_vk(true, true, false, false, 0x7B),
            Some("Ctrl+Shift+F12".to_string())
        );
        assert_eq!(
            format_hotkey_from_vk(true, false, true, false, 'R' as u32),
            Some("Ctrl+Alt+R".to_string())
        );
    }

    #[test]
    fn test_parse_invalid_hotkey() {
        assert!(parse_hotkey("").is_err());
        assert!(parse_hotkey("Ctrl+").is_err());
        assert!(parse_hotkey("Ctrl+Shift+UnknownKey999").is_err());
    }
}
