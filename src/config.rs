use serde::{Deserialize, Serialize};
use std::fs;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use crate::dpi::clamp_and_snap_dpi;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct MouseConfig {
    #[serde(default = "default_active_stage")]
    pub active_stage: u8, // 1..5

    #[serde(default = "default_stages")]
    pub stages: [u32; 5],

    #[serde(default = "default_polling_rate")]
    pub polling_rate: u32, // 125, 250, 500, 1000, 2000, 4000, 8000

    #[serde(default = "default_lod")]
    pub lod: u32, // 1 (1mm) or 2 (2mm)
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct SystemConfig {
    #[serde(default = "default_true")]
    pub high_res_timer: bool,

    #[serde(default = "default_true")]
    pub release_stuck_click_on_start: bool,

    #[serde(default = "default_true")]
    pub show_osd: bool,

    #[serde(default = "default_hotkey")]
    pub reset_hotkey: String, // Global keyboard shortcut to cycle USB port

    #[serde(default = "default_theme")]
    pub theme: String, // "obsidian" or "fluent"
}

fn default_active_stage() -> u8 {
    3
}

fn default_stages() -> [u32; 5] {
    [400, 800, 1600, 3200, 6400]
}

fn default_polling_rate() -> u32 {
    1000
}

fn default_lod() -> u32 {
    1
}

fn default_true() -> bool {
    true
}

fn default_hotkey() -> String {
    "Ctrl+Shift+F12".to_string()
}

fn default_theme() -> String {
    "obsidian".to_string()
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct AppConfig {
    #[serde(default)]
    pub mouse: MouseConfig,

    #[serde(default)]
    pub system: SystemConfig,
}

impl Default for MouseConfig {
    fn default() -> Self {
        Self {
            active_stage: default_active_stage(),
            stages: default_stages(),
            polling_rate: default_polling_rate(),
            lod: default_lod(),
        }
    }
}

impl Default for SystemConfig {
    fn default() -> Self {
        Self {
            high_res_timer: true,
            release_stuck_click_on_start: true,
            show_osd: true,
            reset_hotkey: default_hotkey(),
            theme: default_theme(),
        }
    }
}

impl Default for AppConfig {
    fn default() -> Self {
        Self {
            mouse: MouseConfig::default(),
            system: SystemConfig::default(),
        }
    }
}

impl AppConfig {
    pub fn sanitize(&mut self) {
        self.mouse.active_stage = self.mouse.active_stage.clamp(1, 5);
        for dpi in &mut self.mouse.stages {
            *dpi = clamp_and_snap_dpi(*dpi);
        }
        if ![125, 250, 500, 1000, 2000, 4000, 8000].contains(&self.mouse.polling_rate) {
            self.mouse.polling_rate = 1000;
        }
        self.mouse.lod = if self.mouse.lod >= 2 { 2 } else { 1 };
        if self.system.reset_hotkey.trim().is_empty() {
            self.system.reset_hotkey = default_hotkey();
        }
    }

    pub fn get_config_path() -> PathBuf {
        // First check current working directory / exe dir
        if let Ok(exe_path) = std::env::current_exe() {
            if let Some(dir) = exe_path.parent() {
                let local_path = dir.join("config.toml");
                if local_path.exists() {
                    return local_path;
                }
            }
        }

        // Also check current working directory if running from cargo/portable folder
        let cwd_path = PathBuf::from("config.toml");
        if cwd_path.exists() {
            return cwd_path;
        }

        // Otherwise use %APPDATA%\lift2\config.toml
        if let Ok(app_data) = std::env::var("APPDATA") {
            let config_dir = Path::new(&app_data).join("lift2");
            let _ = fs::create_dir_all(&config_dir);
            return config_dir.join("config.toml");
        }

        cwd_path
    }

    pub fn get_last_modified() -> Option<SystemTime> {
        let path = Self::get_config_path();
        fs::metadata(path).ok().and_then(|m| m.modified().ok())
    }

    pub fn load() -> Self {
        let path = Self::get_config_path();
        if path.exists() {
            if let Ok(content) = fs::read_to_string(&path) {
                if let Ok(mut cfg) = toml::from_str::<AppConfig>(&content) {
                    cfg.sanitize();
                    return cfg;
                }
            }
        }

        // Return default and save it
        let default_cfg = Self::default();
        let _ = default_cfg.save();
        default_cfg
    }

    pub fn save(&self) -> Result<(), String> {
        let mut clean = self.clone();
        clean.sanitize();
        let path = Self::get_config_path();
        if let Some(parent) = path.parent() {
            if !parent.as_os_str().is_empty() {
                let _ = fs::create_dir_all(parent);
            }
        }
        let content = toml::to_string_pretty(&clean)
            .map_err(|e| format!("Failed to serialize config: {}", e))?;
        fs::write(&path, content)
            .map_err(|e| format!("Failed to write config file to {:?}: {}", path, e))?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_config_serialization_with_custom_hotkey() {
        let mut cfg = AppConfig::default();
        cfg.system.reset_hotkey = "Ctrl+Alt+R".to_string();
        cfg.system.theme = "fluent".to_string();
        cfg.mouse.stages[0] = 600;

        let toml_str = toml::to_string(&cfg).expect("Should serialize config");
        let parsed: AppConfig = toml::from_str(&toml_str).expect("Should deserialize config");

        assert_eq!(parsed.system.reset_hotkey, "Ctrl+Alt+R");
        assert_eq!(parsed.system.theme, "fluent");
        assert_eq!(parsed.mouse.stages[0], 600);
    }

    #[test]
    fn test_config_sanitize_bounds() {
        let mut cfg = AppConfig::default();
        cfg.mouse.active_stage = 99;
        cfg.mouse.stages[0] = 10;
        cfg.mouse.stages[1] = 825;
        cfg.mouse.polling_rate = 9999;
        cfg.mouse.lod = 5;
        cfg.sanitize();

        assert_eq!(cfg.mouse.active_stage, 5);
        assert_eq!(cfg.mouse.stages[0], 50);
        assert_eq!(cfg.mouse.stages[1], 850);
        assert_eq!(cfg.mouse.polling_rate, 1000);
        assert_eq!(cfg.mouse.lod, 2);
    }
}
