#![windows_subsystem = "windows"]

mod config;
mod device;
mod dpi;
mod hotkey;
mod icon;
mod native_gui;
mod osd;
mod protocol;
mod system;
mod theme;
mod tray;

use clap::{Parser, Subcommand};
use std::sync::{Arc, Mutex};

use crate::config::AppConfig;
use crate::device::{cycle_usb_port, fix_usb_power_settings, Lift2Device};
use crate::osd::show_dpi_osd;
use crate::protocol::{LiftOffDistance, PollingRate};
use crate::system::{enable_high_res_timer, release_stuck_clicks};
use crate::tray::TrayApp;

extern "system" {
    fn AttachConsole(dwProcessId: u32) -> i32;
}
const ATTACH_PARENT_PROCESS: u32 = 0xFFFFFFFF;

#[derive(Parser)]
#[command(name = "lift2")]
#[command(author = "Antigravity")]
#[command(version = "1.0.0")]
#[command(about = "Ultra-lightweight native driver & manager for NZXT Lift 2 Ergo")]
struct Cli {
    #[command(subcommand)]
    command: Option<Commands>,
}

#[derive(Subcommand)]
enum Commands {
    /// Launch the native esports GUI Control Panel (Default)
    Gui,

    /// Show current mouse settings, active DPI, firmware version, and hardware serial
    Status,

    /// Switch the active DPI stage (1 to 5)
    Switch {
        #[arg(value_parser = clap::value_parser!(u8).range(1..=5))]
        stage: u8,
    },

    /// Set DPI for a specific stage (1 to 5, DPI 50 to 26000 in 50 DPI steps)
    SetDpi {
        #[arg(value_parser = clap::value_parser!(u8).range(1..=5))]
        stage: u8,
        #[arg(value_parser = clap::value_parser!(u32).range(50..=26000))]
        dpi: u32,
    },

    /// Set mouse USB polling rate (125, 250, 500, 1000, 2000, 4000, 8000 Hz)
    SetPolling {
        #[arg(value_parser = clap::value_parser!(u32))]
        rate: u32,
    },

    /// Set sensor lift-off distance (1 for 1mm Low, 2 for 2mm High)
    SetLod {
        #[arg(value_parser = clap::value_parser!(u32).range(1..=2))]
        lod: u32,
    },

    /// Hardware-cycle the USB hub port to cleanly reboot mouse MCU & optical switches (Fixes boot stuck clicks)
    ResetUsb,

    /// Apply Windows Registry tweaks to permanently disable USB power sleep / selective suspend
    FixPower,

    /// Run background system tray daemon only
    Tray,
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    if args.len() > 1 && !args[1].eq_ignore_ascii_case("gui") {
        unsafe {
            AttachConsole(ATTACH_PARENT_PROCESS);
        }
    }

    let cli = Cli::parse();
    let mut config = AppConfig::load();

    match cli.command {
        Some(Commands::Status) => {
            cmd_status();
        }
        Some(Commands::Switch { stage }) => {
            cmd_switch(stage, &mut config);
        }
        Some(Commands::SetDpi { stage, dpi }) => {
            cmd_set_dpi(stage, dpi, &mut config);
        }
        Some(Commands::SetPolling { rate }) => {
            cmd_set_polling(rate, &mut config);
        }
        Some(Commands::SetLod { lod }) => {
            cmd_set_lod(lod, &mut config);
        }
        Some(Commands::ResetUsb) => {
            cmd_reset_usb();
        }
        Some(Commands::FixPower) => {
            cmd_fix_power();
        }
        Some(Commands::Tray) => {
            cmd_tray(config);
        }
        Some(Commands::Gui) | None => {
            cmd_gui(config);
        }
    }
}

fn cmd_status() {
    println!("Connecting to NZXT Lift 2 Ergo...");
    let mut dev = match Lift2Device::open() {
        Ok(d) => d,
        Err(e) => {
            eprintln!("Error: {}", e);
            eprintln!("\nTip: If mouse buttons are stuck, press Ctrl+Shift+F12 or run `lift2 reset-usb`.");
            std::process::exit(1);
        }
    };

    let fw = dev.get_firmware_version().unwrap_or_else(|_| "Unknown".to_string());
    let uid = dev.get_uid().unwrap_or_else(|_| "Unknown".to_string());
    let settings = match dev.get_profile_settings(0) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("Failed to read profile settings: {}", e);
            std::process::exit(1);
        }
    };

    println!("\n================ NZXT Lift 2 Ergo Status ================");
    println!(" Firmware Version : v{}", fw);
    println!(" Hardware Serial  : {}", uid);
    println!(" Polling Rate     : {} Hz ({})", settings.polling_rate.to_hz(), settings.polling_rate.to_delay_str());
    println!(" Lift-off Height  : {}", settings.lod.to_label());
    println!(" Active Stage     : Stage {} ({} DPI)", settings.active_cpi_idx + 1, settings.stages[settings.active_cpi_idx as usize]);
    println!("---------------------------------------------------------");
    println!(" Configured DPI Stages (Onboard Flash):");
    for (i, &dpi) in settings.stages.iter().enumerate() {
        let marker = if i as u8 == settings.active_cpi_idx { "-->" } else { "   " };
        println!("  {} Stage {}: {:5} DPI", marker, i + 1, dpi);
    }
    println!("=========================================================\n");
}

fn cmd_switch(stage: u8, config: &mut AppConfig) {
    let mut dev = match Lift2Device::open() {
        Ok(d) => d,
        Err(e) => {
            eprintln!("Error connecting to mouse: {}", e);
            std::process::exit(1);
        }
    };

    let idx = stage - 1;
    if let Err(e) = dev.set_active_cpi_idx(idx) {
        eprintln!("Failed to switch stage: {}", e);
        std::process::exit(1);
    }

    config.mouse.active_stage = stage;
    let _ = config.save();

    let dpi = config.mouse.stages[idx as usize];
    println!("Switched to Stage {} ({} DPI)", stage, dpi);
    show_dpi_osd(dpi, stage, 5);
}

fn cmd_set_dpi(stage: u8, dpi: u32, config: &mut AppConfig) {
    let mut dev = match Lift2Device::open() {
        Ok(d) => d,
        Err(e) => {
            eprintln!("Error connecting to mouse: {}", e);
            std::process::exit(1);
        }
    };

    let idx = (stage - 1) as usize;
    let mut settings = match dev.get_profile_settings(0) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("Failed to read settings: {}", e);
            std::process::exit(1);
        }
    };

    settings.stages[idx] = dpi;
    if let Err(e) = dev.set_profile_settings(&settings) {
        eprintln!("Failed to save DPI: {}", e);
        std::process::exit(1);
    }

    config.mouse.stages[idx] = dpi;
    let _ = config.save();

    println!("Stage {} set to {} DPI (Saved to mouse flash memory)", stage, dpi);
    if config.mouse.active_stage == stage {
        show_dpi_osd(dpi, stage, 5);
    }
}

fn cmd_set_polling(rate_hz: u32, config: &mut AppConfig) {
    if ![125, 250, 500, 1000, 2000, 4000, 8000].contains(&rate_hz) {
        eprintln!("Invalid polling rate: {} Hz. Allowed rates: 125, 250, 500, 1000, 2000, 4000, 8000 Hz.", rate_hz);
        std::process::exit(1);
    }

    let mut dev = match Lift2Device::open() {
        Ok(d) => d,
        Err(e) => {
            eprintln!("Error connecting to mouse: {}", e);
            std::process::exit(1);
        }
    };

    let rate = PollingRate::from_hz(rate_hz);
    if let Err(e) = dev.set_polling_rate(0, rate) {
        eprintln!("Failed to set polling rate: {}", e);
        std::process::exit(1);
    }

    config.mouse.polling_rate = rate_hz;
    let _ = config.save();
    println!("Polling rate updated to {} Hz ({}) (Saved to onboard flash)", rate_hz, rate.to_delay_str());
}

fn cmd_set_lod(lod_mm: u32, config: &mut AppConfig) {
    let mut dev = match Lift2Device::open() {
        Ok(d) => d,
        Err(e) => {
            eprintln!("Error connecting to mouse: {}", e);
            std::process::exit(1);
        }
    };

    let lod = LiftOffDistance::from_mm(lod_mm);
    if let Err(e) = dev.set_lod(0, lod) {
        eprintln!("Failed to set LOD: {}", e);
        std::process::exit(1);
    }

    config.mouse.lod = lod_mm;
    let _ = config.save();
    println!("Sensor LOD updated to {} (Saved to onboard flash)", lod.to_label());
}

fn cmd_reset_usb() {
    println!("Initiating hardware-level USB hub port cycle...");
    match cycle_usb_port() {
        Ok(_) => {
            println!("Port cycle signal successfully sent to parent USB Hub.");
            println!("Resetting OS mouse button latch states...");
            release_stuck_clicks();
            println!("Success! Mouse optical switches and MCU cleanly rebooted.");
        }
        Err(e) => {
            eprintln!("Error resetting USB port: {}", e);
            std::process::exit(1);
        }
    }
}

fn cmd_fix_power() {
    println!("Applying Windows Registry power management tweaks for NZXT Lift 2...");
    match fix_usb_power_settings() {
        Ok(_) => {
            println!("Successfully applied Windows Registry power management tweaks.");
            println!("- EnhancedPowerManagementEnabled = 0");
            println!("- SelectiveSuspendEnabled = 0");
            println!("- DeviceSelectiveSuspended = 0");
            println!("USB port sleep is now permanently disabled on boot.");
        }
        Err(e) => {
            eprintln!("Registry patch failed: {}", e);
            eprintln!("Tip: Run PowerShell/CMD as Administrator to write to HKEY_LOCAL_MACHINE.");
            std::process::exit(1);
        }
    }
}

fn cmd_tray(config: AppConfig) {
    if config.system.release_stuck_click_on_start {
        release_stuck_clicks();
    }
    if config.system.high_res_timer {
        enable_high_res_timer();
    }

    let shared_cfg = Arc::new(Mutex::new(config));
    let tray_app = TrayApp::new(shared_cfg);
    tray_app.start_background_threads();

    // Keep thread alive if running in tray-only mode
    loop {
        std::thread::sleep(std::time::Duration::from_secs(3600));
    }
}

fn cmd_gui(config: AppConfig) {
    if config.system.release_stuck_click_on_start {
        release_stuck_clicks();
    }
    if config.system.high_res_timer {
        enable_high_res_timer();
    }

    let shared_cfg = Arc::new(Mutex::new(config));

    // Start background tray daemon, DPI button listener, and Global Hotkey
    let tray_app = TrayApp::new(Arc::clone(&shared_cfg));
    tray_app.start_background_threads();

    // Open zero-overhead Win32 Native Double-Buffered GUI (< 8MB RAM, 0ms latency)
    crate::native_gui::open_native_gui(Arc::clone(&shared_cfg));

    // Keep process alive so background tray daemon, DPI listener, and hotkeys stay responsive
    loop {
        std::thread::sleep(std::time::Duration::from_secs(3600));
    }
}
