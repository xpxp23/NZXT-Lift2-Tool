use std::ffi::OsStr;
use std::os::windows::ffi::OsStrExt;
use std::ptr::null_mut;
use std::sync::atomic::{AtomicBool, Ordering};

use windows_sys::Win32::Foundation::{GetLastError, HANDLE, HWND};
use windows_sys::Win32::Media::{timeBeginPeriod, timeEndPeriod};
use windows_sys::Win32::System::Registry::{
    RegCloseKey, RegDeleteValueW, RegOpenKeyExW, RegQueryValueExW, RegSetValueExW, HKEY,
    HKEY_CURRENT_USER, KEY_READ, KEY_WRITE, REG_SZ,
};
use windows_sys::Win32::UI::Input::KeyboardAndMouse::{
    mouse_event, GetAsyncKeyState, MOUSEEVENTF_LEFTUP, VK_LBUTTON,
};
use windows_sys::Win32::UI::WindowsAndMessaging::PostMessageW;

const ERROR_ALREADY_EXISTS: u32 = 183;

extern "system" {
    fn CreateMutexW(
        lpMutexAttributes: *mut std::ffi::c_void,
        bInitialOwner: i32,
        lpName: *const u16,
    ) -> HANDLE;

    fn FindWindowW(lpClassName: *const u16, lpWindowName: *const u16) -> HWND;
}

static HIGH_RES_TIMER_ACTIVE: AtomicBool = AtomicBool::new(false);

pub fn enable_high_res_timer() {
    if !HIGH_RES_TIMER_ACTIVE.swap(true, Ordering::SeqCst) {
        unsafe {
            timeBeginPeriod(1);
        }
    }
}

pub fn disable_high_res_timer() {
    if HIGH_RES_TIMER_ACTIVE.swap(false, Ordering::SeqCst) {
        unsafe {
            timeEndPeriod(1);
        }
    }
}

/// Ensures only one background/GUI instance of NZXT Lift 2 Driver runs at a time.
/// If an instance is already running and `wake_gui` is true, sends `WM_WAKE_GUI`
/// to the existing tray daemon so it brings the control panel to the front, and returns `false`.
pub fn ensure_single_instance_or_wake(wake_gui: bool) -> bool {
    let mutex_name = to_wide("Local\\NZXTLift2DriverSingleInstanceMutex");
    unsafe {
        let handle = CreateMutexW(null_mut(), 1, mutex_name.as_ptr());
        if handle == 0 as HANDLE {
            return true;
        }
        if GetLastError() == ERROR_ALREADY_EXISTS {
            if wake_gui {
                let class_name = to_wide("NZXTLift2TrayClass");
                let existing_hwnd = FindWindowW(class_name.as_ptr(), null_mut());
                if existing_hwnd != 0 as HWND {
                    PostMessageW(existing_hwnd, crate::tray::WM_WAKE_GUI, 0, 0);
                }
            }
            return false;
        }
        // Keep the mutex handle alive for the lifetime of the process
        true
    }
}

/// Notifies an already-running tray/GUI instance to reload configuration and refresh its UI/tooltip.
/// Returns `true` if a running instance was found and notified.
pub fn notify_running_instance_refresh() -> bool {
    let class_name = to_wide("NZXTLift2TrayClass");
    unsafe {
        let existing_hwnd = FindWindowW(class_name.as_ptr(), null_mut());
        if existing_hwnd != 0 as HWND {
            PostMessageW(existing_hwnd, crate::tray::WM_REFRESH_TRAY, 0, 0);
            true
        } else {
            false
        }
    }
}

/// Dispatches mouse button UP event ONLY if the left button is actually
/// stuck down in the OS, completely eliminating phantom right clicks.
pub fn release_stuck_clicks() {
    unsafe {
        let is_left_down = (GetAsyncKeyState(VK_LBUTTON as i32) as u16) & 0x8000 != 0;
        if is_left_down {
            mouse_event(MOUSEEVENTF_LEFTUP, 0, 0, 0, 0);
        }
    }
}

fn to_wide(s: &str) -> Vec<u16> {
    OsStr::new(s).encode_wide().chain(std::iter::once(0)).collect()
}

const RUN_SUBKEY: &str = "Software\\Microsoft\\Windows\\CurrentVersion\\Run";
const APP_REG_NAME: &str = "NZXTLift2Driver";

/// Checks if Windows auto-start on boot is enabled for NZXT Lift 2 Driver
pub fn is_autostart_enabled() -> bool {
    unsafe {
        let subkey = to_wide(RUN_SUBKEY);
        let mut hkey: HKEY = 0 as HKEY;
        if RegOpenKeyExW(HKEY_CURRENT_USER, subkey.as_ptr(), 0, KEY_READ, &mut hkey) != 0 {
            return false;
        }

        let value_name = to_wide(APP_REG_NAME);
        let mut data_type = 0u32;
        let mut data_size = 0u32;
        let status = RegQueryValueExW(
            hkey,
            value_name.as_ptr(),
            null_mut(),
            &mut data_type,
            null_mut(),
            &mut data_size,
        );
        RegCloseKey(hkey);
        status == 0 && data_size > 0
    }
}

/// Enables or disables Windows auto-start on boot (HKCU\...\Run)
pub fn set_autostart(enable: bool) -> Result<(), String> {
    unsafe {
        let subkey = to_wide(RUN_SUBKEY);
        let mut hkey: HKEY = 0 as HKEY;
        let res = RegOpenKeyExW(
            HKEY_CURRENT_USER,
            subkey.as_ptr(),
            0,
            KEY_WRITE | KEY_READ,
            &mut hkey,
        );
        if res != 0 {
            return Err(format!("无法打开注册表启动项 (错误码 {})", res));
        }

        let value_name = to_wide(APP_REG_NAME);

        if enable {
            let exe_path = std::env::current_exe()
                .map_err(|e| format!("获取当前程序路径失败: {}", e))?;
            let cmd = format!("\"{}\" tray", exe_path.to_string_lossy());
            let wide_cmd = to_wide(&cmd);
            let byte_len = (wide_cmd.len() * 2) as u32;

            let set_res = RegSetValueExW(
                hkey,
                value_name.as_ptr(),
                0,
                REG_SZ,
                wide_cmd.as_ptr() as *const u8,
                byte_len,
            );
            RegCloseKey(hkey);
            if set_res != 0 {
                return Err(format!("写入自启动注册表失败 (错误码 {})", set_res));
            }
        } else {
            let del_res = RegDeleteValueW(hkey, value_name.as_ptr());
            RegCloseKey(hkey);
            // 2 is ERROR_FILE_NOT_FOUND, which means it wasn't registered anyway
            if del_res != 0 && del_res != 2 {
                return Err(format!("删除自启动注册表项失败 (错误码 {})", del_res));
            }
        }

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_autostart_toggle() {
        let initial = is_autostart_enabled();
        // Toggle to true
        let _ = set_autostart(true);
        assert!(is_autostart_enabled(), "Autostart should be enabled");

        // Toggle to false
        let _ = set_autostart(false);
        assert!(!is_autostart_enabled(), "Autostart should be disabled");

        // Restore initial state
        let _ = set_autostart(initial);
    }
}
