use std::ffi::OsStr;
use std::os::windows::ffi::OsStrExt;
use std::ptr::null_mut;
use std::sync::atomic::{AtomicBool, AtomicIsize, AtomicU32, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

use windows_sys::Win32::Foundation::{
    CloseHandle, GetLastError, ERROR_IO_PENDING, HANDLE, HWND, INVALID_HANDLE_VALUE, LPARAM,
    LRESULT, POINT, WAIT_OBJECT_0, WPARAM,
};
use windows_sys::Win32::Storage::FileSystem::{
    ReadFile, FILE_FLAG_OVERLAPPED, FILE_GENERIC_READ, FILE_SHARE_READ, FILE_SHARE_WRITE,
    OPEN_EXISTING,
};
use windows_sys::Win32::System::IO::{CancelIo, GetOverlappedResult, OVERLAPPED};
use windows_sys::Win32::System::Threading::{ResetEvent, WaitForSingleObject};
use windows_sys::Win32::UI::Input::KeyboardAndMouse::{RegisterHotKey, UnregisterHotKey};
use windows_sys::Win32::UI::Shell::{
    Shell_NotifyIconW, NIF_ICON, NIF_MESSAGE, NIF_TIP, NIM_ADD, NIM_DELETE, NIM_MODIFY,
    NOTIFYICONDATAW,
};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    AppendMenuW, CreatePopupMenu, CreateWindowExW, DefWindowProcW, DestroyIcon, DestroyMenu,
    DestroyWindow, DispatchMessageW, GetCursorPos, GetMessageW, PostQuitMessage,
    RegisterClassExW, SetForegroundWindow, TrackPopupMenu, TranslateMessage, HICON, HMENU,
    MF_CHECKED, MF_DISABLED, MF_GRAYED, MF_SEPARATOR, MF_STRING, MF_UNCHECKED, TPM_BOTTOMALIGN,
    TPM_RIGHTALIGN, WM_COMMAND, WM_HOTKEY, WM_LBUTTONDBLCLK, WM_LBUTTONUP, WM_RBUTTONUP, WM_USER,
    WNDCLASSEXW,
};

extern "system" {
    fn CreateFileW(
        lpFileName: *const u16,
        dwDesiredAccess: u32,
        dwShareMode: u32,
        lpSecurityAttributes: *mut std::ffi::c_void,
        dwCreationDisposition: u32,
        dwFlagsAndAttributes: u32,
        hTemplateFile: HANDLE,
    ) -> HANDLE;

    fn CreateEventW(
        lpEventAttributes: *mut std::ffi::c_void,
        bManualReset: i32,
        bInitialState: i32,
        lpName: *const u16,
    ) -> HANDLE;

    fn RegisterWindowMessageW(lpString: *const u16) -> u32;
}

use crate::config::AppConfig;
use crate::device::{
    cycle_usb_port, find_lift2_dpi_button_path, fix_usb_power_settings, Lift2Device,
};
use crate::osd::{is_osd_enabled, set_osd_enabled, show_dpi_osd, show_toast_osd};
use crate::system::release_stuck_clicks;

pub const WM_TRAY_ICON: u32 = WM_USER + 1;
pub const WM_WAKE_GUI: u32 = WM_USER + 2;
pub const WM_REFRESH_TRAY: u32 = WM_USER + 3;

const HOTKEY_RESET_ID: i32 = 9001;

const IDM_OPEN_GUI: usize = 1000;
const IDM_STAGE_1: usize = 1001;
const IDM_CYCLE_USB: usize = 2001;
const IDM_FIX_POWER: usize = 2002;
const IDM_OPEN_CONFIG: usize = 2003;
const IDM_TOGGLE_OSD: usize = 2004;
const IDM_EXIT: usize = 3001;

static mut GLOBAL_CONFIG: Option<Arc<Mutex<AppConfig>>> = None;
static GLOBAL_HWND: AtomicIsize = AtomicIsize::new(0);
static WM_TASKBAR_CREATED: AtomicU32 = AtomicU32::new(0);
static USB_RESETTING: AtomicBool = AtomicBool::new(false);
static RECONNECT_DPI_LISTENER: AtomicBool = AtomicBool::new(false);
static mut GLOBAL_NID: NOTIFYICONDATAW = unsafe { std::mem::zeroed() };

pub unsafe fn restore_gui_window() {
    if let Some(cfg_arc) = &*(&raw const GLOBAL_CONFIG) {
        crate::native_gui::open_native_gui(Arc::clone(cfg_arc));
    }
}

pub unsafe fn register_hotkey_from_str(hwnd: HWND, hotkey_str: &str) -> Result<String, String> {
    let parsed = crate::hotkey::parse_hotkey(hotkey_str)?;
    UnregisterHotKey(hwnd, HOTKEY_RESET_ID);
    let success = RegisterHotKey(
        hwnd,
        HOTKEY_RESET_ID,
        parsed.modifiers | crate::hotkey::MOD_NOREPEAT,
        parsed.vk_code,
    );
    if success != 0 {
        Ok(parsed.display_name)
    } else {
        let err = GetLastError();
        Err(format!(
            "注册全局热键 '{}' 失败 (Win32错误码 {})，可能已被占用",
            parsed.display_name, err
        ))
    }
}

pub fn update_global_hotkey(new_hotkey: &str) -> Result<String, String> {
    unsafe {
        let hwnd = GLOBAL_HWND.load(Ordering::SeqCst) as HWND;
        if hwnd != 0 as HWND {
            let canonical = register_hotkey_from_str(hwnd, new_hotkey)?;
            if let Some(cfg_arc) = &*(&raw const GLOBAL_CONFIG) {
                if let Ok(mut cfg) = cfg_arc.lock() {
                    cfg.system.reset_hotkey = canonical.clone();
                    let _ = cfg.save();
                }
            }
            Ok(canonical)
        } else {
            Err("托盘后台服务未启动".to_string())
        }
    }
}

/// Executes hardware USB port cycle asynchronously on a worker thread so UI/Tray message loops never freeze,
/// reporting accurate success or failure feedback via OSD and GUI status bar.
pub fn trigger_async_usb_reset() {
    if USB_RESETTING
        .compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst)
        .is_err()
    {
        return;
    }

    crate::native_gui::notify_gui_status("⏳ 正在执行 USB 端口硬件断电重置 (约 1.5 秒)...");

    thread::spawn(|| {
        let hk_text = unsafe {
            if let Some(cfg_arc) = &*(&raw const GLOBAL_CONFIG) {
                cfg_arc
                    .lock()
                    .map(|c| c.system.reset_hotkey.clone())
                    .unwrap_or_else(|_| "Ctrl+Shift+F12".to_string())
            } else {
                "Ctrl+Shift+F12".to_string()
            }
        };

        let result = cycle_usb_port();
        release_stuck_clicks();
        // Signal the hardware DPI button listener to re-open its HID handle after USB re-enumeration
        RECONNECT_DPI_LISTENER.store(true, Ordering::SeqCst);

        match result {
            Ok(_) => {
                show_toast_osd(&format!("⚡ USB 硬件复位已执行 [{}]", hk_text));
                crate::native_gui::notify_gui_status("⚡ USB 端口硬件断电复位成功！");
                crate::native_gui::notify_gui_refresh();
            }
            Err(e) => {
                show_toast_osd("⚠️ 仅释放按键 (硬件重置需管理员权限)");
                crate::native_gui::notify_gui_status(&format!("❌ USB 硬件重置失败: {}", e));
            }
        }

        USB_RESETTING.store(false, Ordering::SeqCst);
    });
}

pub struct TrayApp {
    config: Arc<Mutex<AppConfig>>,
}

impl TrayApp {
    pub fn new(config: Arc<Mutex<AppConfig>>) -> Self {
        Self { config }
    }

    pub fn start_dpi_listener(&self) {
        let config_for_listener = Arc::clone(&self.config);
        thread::spawn(move || {
            listen_hardware_dpi_button(config_for_listener);
        });
    }

    pub fn run_event_loop_on_current_thread(self) {
        self.start_dpi_listener();
        unsafe {
            run_tray_event_loop(self.config);
        }
    }

    #[allow(dead_code)]
    pub fn start_background_threads(self) {
        self.start_dpi_listener();
        let config_clone = Arc::clone(&self.config);
        thread::spawn(move || unsafe {
            run_tray_event_loop(config_clone);
        });
    }
}

pub unsafe fn update_tray_tooltip(stage_idx: u8, dpi: u32, polling_hz: u32, lod_mm: u32) {
    let hwnd = GLOBAL_HWND.load(Ordering::SeqCst) as HWND;
    if hwnd != 0 as HWND {
        let tip = format!(
            "NZXT Lift 2: {} DPI (Stage {}) | {}Hz | {}mm",
            dpi,
            stage_idx + 1,
            polling_hz,
            lod_mm
        );
        let wide_tip = to_wide(&tip);
        let copy_len = wide_tip.len().min(127);
        let nid_ptr = &raw mut GLOBAL_NID;
        for i in 0..copy_len {
            (*nid_ptr).szTip[i] = wide_tip[i];
        }
        (*nid_ptr).szTip[copy_len] = 0;
        (*nid_ptr).uFlags = NIF_TIP;
        Shell_NotifyIconW(NIM_MODIFY, nid_ptr);
    }
}

unsafe fn show_tray_menu(hwnd: HWND) {
    let menu = CreatePopupMenu();
    if menu == 0 as HMENU {
        return;
    }

    let config = if let Some(cfg) = &*(&raw const GLOBAL_CONFIG) {
        cfg.lock().unwrap().clone()
    } else {
        AppConfig::default()
    };

    let active_stage = config.mouse.active_stage.clamp(1, 5).saturating_sub(1);
    let current_dpi = config.mouse.stages[active_stage as usize % 5];

    let header_text = format!(
        "NZXT Lift 2 [{} DPI / {}Hz]",
        current_dpi, config.mouse.polling_rate
    );
    AppendMenuW(
        menu,
        MF_STRING | MF_DISABLED | MF_GRAYED,
        0,
        to_wide(&header_text).as_ptr(),
    );
    AppendMenuW(menu, MF_SEPARATOR, 0, null_mut());

    AppendMenuW(
        menu,
        MF_STRING,
        IDM_OPEN_GUI,
        to_wide("🎮 Open Control Panel (控制面板)").as_ptr(),
    );
    AppendMenuW(menu, MF_SEPARATOR, 0, null_mut());

    for i in 0..5 {
        let stage_dpi = config.mouse.stages[i];
        let item_text = format!("Stage {}: {} DPI", i + 1, stage_dpi);
        let flag = if i == active_stage as usize {
            MF_STRING | MF_CHECKED
        } else {
            MF_STRING | MF_UNCHECKED
        };
        AppendMenuW(menu, flag, IDM_STAGE_1 + i, to_wide(&item_text).as_ptr());
    }

    AppendMenuW(menu, MF_SEPARATOR, 0, null_mut());
    let osd_flag = if is_osd_enabled() {
        MF_STRING | MF_CHECKED
    } else {
        MF_STRING | MF_UNCHECKED
    };
    AppendMenuW(
        menu,
        osd_flag,
        IDM_TOGGLE_OSD,
        to_wide("🔔 Show DPI OSD (显示屏幕提示)").as_ptr(),
    );

    let reset_text = format!("⚡ Reset Mouse USB [{}]", config.system.reset_hotkey);
    AppendMenuW(menu, MF_STRING, IDM_CYCLE_USB, to_wide(&reset_text).as_ptr());
    AppendMenuW(
        menu,
        MF_STRING,
        IDM_FIX_POWER,
        to_wide("🛠️ Disable USB Power Sleep (Fix Boot)").as_ptr(),
    );
    AppendMenuW(
        menu,
        MF_STRING,
        IDM_OPEN_CONFIG,
        to_wide("📂 Open Config File").as_ptr(),
    );
    AppendMenuW(menu, MF_SEPARATOR, 0, null_mut());
    AppendMenuW(menu, MF_STRING, IDM_EXIT, to_wide("Exit").as_ptr());

    let mut cursor: POINT = std::mem::zeroed();
    GetCursorPos(&mut cursor);

    SetForegroundWindow(hwnd);
    TrackPopupMenu(
        menu,
        TPM_BOTTOMALIGN | TPM_RIGHTALIGN,
        cursor.x,
        cursor.y,
        0,
        hwnd,
        null_mut(),
    );
    DestroyMenu(menu);
}

fn handle_menu_command(hwnd: HWND, cmd_id: usize) {
    match cmd_id {
        IDM_OPEN_GUI => unsafe {
            restore_gui_window();
        },
        IDM_STAGE_1..=1005 => {
            let stage_idx = (cmd_id - IDM_STAGE_1) as u8;
            if let Ok(mut dev) = Lift2Device::open() {
                if dev.set_active_cpi_idx(stage_idx).is_ok() {
                    unsafe {
                        if let Some(cfg_arc) = &*(&raw const GLOBAL_CONFIG) {
                            let mut cfg = cfg_arc.lock().unwrap();
                            cfg.mouse.active_stage = stage_idx + 1;
                            let _ = cfg.save();
                            let dpi = cfg.mouse.stages[stage_idx as usize];
                            update_tray_tooltip(
                                stage_idx,
                                dpi,
                                cfg.mouse.polling_rate,
                                cfg.mouse.lod,
                            );
                            show_dpi_osd(dpi, stage_idx + 1, 5);
                            drop(cfg);
                            crate::native_gui::notify_gui_refresh();
                        }
                    }
                }
            }
        }
        IDM_TOGGLE_OSD => unsafe {
            let new_state = !is_osd_enabled();
            set_osd_enabled(new_state);
            if let Some(cfg_arc) = &*(&raw const GLOBAL_CONFIG) {
                if let Ok(mut cfg) = cfg_arc.lock() {
                    cfg.system.show_osd = new_state;
                    let _ = cfg.save();
                }
            }
            crate::native_gui::notify_gui_refresh();
        },
        IDM_CYCLE_USB => {
            trigger_async_usb_reset();
        }
        IDM_FIX_POWER => match fix_usb_power_settings() {
            Ok(_) => {
                show_toast_osd("✅ USB 节电休眠已永久禁用");
                crate::native_gui::notify_gui_status("✅ 注册表防休眠优化已应用！");
            }
            Err(e) => {
                show_toast_osd("❌ 写入失败：请以管理员身份运行");
                crate::native_gui::notify_gui_status(&format!("❌ 注册表修改失败: {}", e));
            }
        },
        IDM_OPEN_CONFIG => {
            let path = AppConfig::get_config_path();
            let _ = std::process::Command::new("notepad.exe").arg(path).spawn();
        }
        IDM_EXIT => unsafe {
            crate::system::disable_high_res_timer();
            UnregisterHotKey(hwnd, HOTKEY_RESET_ID);
            Shell_NotifyIconW(NIM_DELETE, &raw mut GLOBAL_NID);
            DestroyWindow(hwnd);
            PostQuitMessage(0);
            std::process::exit(0);
        },
        _ => {}
    }
}

unsafe extern "system" fn tray_wnd_proc(
    hwnd: HWND,
    msg: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    let taskbar_created_msg = WM_TASKBAR_CREATED.load(Ordering::Relaxed);
    if taskbar_created_msg != 0 && msg == taskbar_created_msg {
        // Re-register tray icon when Windows Explorer (explorer.exe) restarts
        let nid_ptr = &raw mut GLOBAL_NID;
        (*nid_ptr).uFlags = NIF_ICON | NIF_MESSAGE | NIF_TIP;
        Shell_NotifyIconW(NIM_ADD, nid_ptr);
        return 0;
    }

    match msg {
        WM_TRAY_ICON => {
            let lp = lparam as u32;
            if lp == WM_RBUTTONUP {
                show_tray_menu(hwnd);
            } else if lp == WM_LBUTTONUP || lp == WM_LBUTTONDBLCLK {
                restore_gui_window();
            }
            0
        }
        WM_WAKE_GUI => {
            restore_gui_window();
            0
        }
        WM_REFRESH_TRAY => {
            let reloaded = AppConfig::load();
            set_osd_enabled(reloaded.system.show_osd);
            let _ = register_hotkey_from_str(hwnd, &reloaded.system.reset_hotkey);
            let stage_idx = reloaded.mouse.active_stage.clamp(1, 5).saturating_sub(1);
            let dpi = reloaded.mouse.stages[stage_idx as usize];
            update_tray_tooltip(
                stage_idx,
                dpi,
                reloaded.mouse.polling_rate,
                reloaded.mouse.lod,
            );
            if let Some(cfg_arc) = &*(&raw const GLOBAL_CONFIG) {
                if let Ok(mut cfg) = cfg_arc.lock() {
                    *cfg = reloaded;
                }
            }
            crate::native_gui::notify_gui_refresh();
            0
        }
        WM_HOTKEY => {
            if (wparam as i32) == HOTKEY_RESET_ID {
                trigger_async_usb_reset();
            }
            0
        }
        WM_COMMAND => {
            let cmd_id = (wparam & 0xFFFF) as usize;
            handle_menu_command(hwnd, cmd_id);
            0
        }
        _ => DefWindowProcW(hwnd, msg, wparam, lparam),
    }
}

unsafe fn run_tray_event_loop(config: Arc<Mutex<AppConfig>>) {
    GLOBAL_CONFIG = Some(config.clone());

    let taskbar_msg = RegisterWindowMessageW(to_wide("TaskbarCreated").as_ptr());
    WM_TASKBAR_CREATED.store(taskbar_msg, Ordering::Relaxed);

    let class_name = to_wide("NZXTLift2TrayClass");

    let wnd_class = WNDCLASSEXW {
        cbSize: std::mem::size_of::<WNDCLASSEXW>() as u32,
        style: 0,
        lpfnWndProc: Some(tray_wnd_proc),
        cbClsExtra: 0,
        cbWndExtra: 0,
        hInstance: 0 as _,
        hIcon: 0 as HICON,
        hCursor: 0 as _,
        hbrBackground: 0 as _,
        lpszMenuName: null_mut(),
        lpszClassName: class_name.as_ptr(),
        hIconSm: 0 as HICON,
    };

    RegisterClassExW(&wnd_class);

    let hwnd = CreateWindowExW(
        0,
        class_name.as_ptr(),
        to_wide("NZXT Lift 2 Driver").as_ptr(),
        0,
        0,
        0,
        0,
        0,
        0 as HWND,
        0 as _,
        0 as _,
        null_mut(),
    );

    GLOBAL_HWND.store(hwnd as isize, Ordering::SeqCst);

    // Register Emergency Global Hotkey from configuration
    let (hotkey_str, show_osd) = {
        let cfg = config.lock().unwrap();
        (cfg.system.reset_hotkey.clone(), cfg.system.show_osd)
    };
    set_osd_enabled(show_osd);
    if register_hotkey_from_str(hwnd, &hotkey_str).is_err() {
        let _ = register_hotkey_from_str(hwnd, "Ctrl+Shift+F12");
    }

    let icon = crate::icon::create_custom_tray_hicon();

    let initial_cfg = config.lock().unwrap().clone();
    let initial_stage = initial_cfg.mouse.active_stage.clamp(1, 5).saturating_sub(1);
    let initial_dpi = initial_cfg.mouse.stages[initial_stage as usize % 5];
    let tip = format!(
        "NZXT Lift 2: {} DPI (Stage {}) | {}Hz | {}mm",
        initial_dpi,
        initial_stage + 1,
        initial_cfg.mouse.polling_rate,
        initial_cfg.mouse.lod
    );
    let wide_tip = to_wide(&tip);

    GLOBAL_NID = std::mem::zeroed();
    GLOBAL_NID.cbSize = std::mem::size_of::<NOTIFYICONDATAW>() as u32;
    GLOBAL_NID.hWnd = hwnd;
    GLOBAL_NID.uID = 1;
    GLOBAL_NID.uFlags = NIF_ICON | NIF_MESSAGE | NIF_TIP;
    GLOBAL_NID.uCallbackMessage = WM_TRAY_ICON;
    GLOBAL_NID.hIcon = icon;

    let copy_len = wide_tip.len().min(127);
    for i in 0..copy_len {
        GLOBAL_NID.szTip[i] = wide_tip[i];
    }

    Shell_NotifyIconW(NIM_ADD, &raw mut GLOBAL_NID);

    let mut msg = std::mem::zeroed();
    while GetMessageW(&mut msg, 0 as HWND, 0, 0) > 0 {
        TranslateMessage(&msg);
        DispatchMessageW(&msg);
    }

    UnregisterHotKey(hwnd, HOTKEY_RESET_ID);
    DestroyIcon(icon);
}

/// Continuously monitors the mouse's physical DPI switch button (`MI_00&COL04`).
/// Automatically reconnects if the mouse is unplugged, plugged into a different port, or reset via `cycle_usb_port`.
fn listen_hardware_dpi_button(config: Arc<Mutex<AppConfig>>) {
    unsafe {
        let event = CreateEventW(null_mut(), 1, 0, null_mut());
        if event == 0 as HANDLE {
            return;
        }

        loop {
            RECONNECT_DPI_LISTENER.store(false, Ordering::SeqCst);

            let col04_path = match find_lift2_dpi_button_path() {
                Ok(Some(p)) => p,
                _ => {
                    thread::sleep(Duration::from_millis(1500));
                    continue;
                }
            };

            let wide: Vec<u16> = OsStr::new(&col04_path)
                .encode_wide()
                .chain(std::iter::once(0))
                .collect();

            let handle = CreateFileW(
                wide.as_ptr(),
                FILE_GENERIC_READ,
                FILE_SHARE_READ | FILE_SHARE_WRITE,
                null_mut(),
                OPEN_EXISTING,
                FILE_FLAG_OVERLAPPED,
                null_mut(),
            );

            if handle == INVALID_HANDLE_VALUE {
                thread::sleep(Duration::from_millis(1500));
                continue;
            }

            let mut buf = [0u8; 64];
            let mut device_disconnected = false;

            while !device_disconnected {
                let mut ov: OVERLAPPED = std::mem::zeroed();
                ov.hEvent = event;
                ResetEvent(event);

                let mut bytes_read = 0u32;
                let ok = ReadFile(handle, buf.as_mut_ptr(), 64, &mut bytes_read, &mut ov);
                if ok == 0 && GetLastError() != ERROR_IO_PENDING {
                    // Device disconnected or port cycled; close handle and re-enumerate
                    break;
                }

                // Wait for the pending overlapped ReadFile to complete without issuing duplicate IRPs
                loop {
                    let wait = WaitForSingleObject(event, 1500);
                    if wait == WAIT_OBJECT_0 {
                        let mut transferred = 0u32;
                        let gor_ok = GetOverlappedResult(handle, &ov, &mut transferred, 0);
                        if gor_ok == 0 {
                            device_disconnected = true;
                            break;
                        }

                        if transferred >= 2 && buf[0] == 0x09 {
                            thread::sleep(Duration::from_millis(20));
                            if let Ok(mut dev) = Lift2Device::open() {
                                if let Ok(raw_idx) = dev.get_active_cpi_idx() {
                                    let active_idx = raw_idx.min(4);
                                    let mut cfg = config.lock().unwrap();
                                    cfg.mouse.active_stage = active_idx + 1;
                                    let _ = cfg.save();
                                    let dpi = cfg.mouse.stages[active_idx as usize];
                                    let polling = cfg.mouse.polling_rate;
                                    let lod = cfg.mouse.lod;
                                    drop(cfg);

                                    update_tray_tooltip(active_idx, dpi, polling, lod);
                                    show_dpi_osd(dpi, active_idx + 1, 5);
                                    crate::native_gui::notify_gui_refresh();
                                }
                            }
                        }
                        break;
                    } else if RECONNECT_DPI_LISTENER.swap(false, Ordering::SeqCst) {
                        let mut transferred = 0u32;
                        CancelIo(handle);
                        GetOverlappedResult(handle, &ov, &mut transferred, 1);
                        device_disconnected = true;
                        break;
                    }
                }
            }

            CloseHandle(handle);
            thread::sleep(Duration::from_millis(800));
        }
    }
}

fn to_wide(s: &str) -> Vec<u16> {
    OsStr::new(s).encode_wide().chain(std::iter::once(0)).collect()
}
