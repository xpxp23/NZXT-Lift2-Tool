use std::ffi::OsStr;
use std::os::windows::ffi::OsStrExt;
use std::ptr::null_mut;
use std::sync::atomic::{AtomicBool, AtomicIsize, AtomicU32, AtomicU8, Ordering};
use std::sync::{Condvar, Mutex, Once};
use std::thread;
use std::time::Duration;

use windows_sys::Win32::Foundation::{HWND, LPARAM, LRESULT, RECT, WPARAM};
use windows_sys::Win32::Graphics::Gdi::{
    BeginPaint, CreateFontW, CreatePen, CreateSolidBrush, DeleteObject, DrawTextW, EndPaint,
    InvalidateRect, RoundRect, SelectObject, SetBkMode, SetTextColor, CLEARTYPE_QUALITY,
    DT_CENTER, DT_SINGLELINE, DT_VCENTER, FW_BOLD, FW_SEMIBOLD, PAINTSTRUCT, PS_SOLID,
    TRANSPARENT,
};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, DefWindowProcW, DispatchMessageW, GetMessageW, GetSystemMetrics, KillTimer,
    PostMessageW, RegisterClassExW, SetLayeredWindowAttributes, SetTimer, SetWindowPos,
    ShowWindow, TranslateMessage, CS_HREDRAW, CS_VREDRAW, HICON, HWND_TOPMOST, LWA_ALPHA,
    SM_CXSCREEN, SM_CYSCREEN, SWP_NOACTIVATE, SWP_SHOWWINDOW, SW_HIDE, SW_SHOWNOACTIVATE,
    WM_PAINT, WM_TIMER, WM_USER, WNDCLASSEXW, WS_EX_LAYERED, WS_EX_NOACTIVATE, WS_EX_TOOLWINDOW,
    WS_EX_TOPMOST, WS_EX_TRANSPARENT, WS_POPUP,
};

static INIT_ONCE: Once = Once::new();
static OSD_READY: (Mutex<bool>, Condvar) = (Mutex::new(false), Condvar::new());
static OSD_ENABLED: AtomicBool = AtomicBool::new(true);
static CURRENT_DPI: AtomicU32 = AtomicU32::new(1600);
static CURRENT_STAGE: AtomicU8 = AtomicU8::new(3);
static MAX_STAGES: AtomicU8 = AtomicU8::new(5);
static IS_CUSTOM_MSG: AtomicBool = AtomicBool::new(false);
static CUSTOM_MSG: Mutex<String> = Mutex::new(String::new());
static OSD_HWND: AtomicIsize = AtomicIsize::new(0);

const WM_SHOW_OSD: u32 = WM_USER + 101;
const TIMER_HIDE_ID: usize = 1;
const OSD_WIDTH: i32 = 280;
const OSD_HEIGHT: i32 = 72;

pub fn set_osd_enabled(enabled: bool) {
    OSD_ENABLED.store(enabled, Ordering::Relaxed);
}

pub fn is_osd_enabled() -> bool {
    OSD_ENABLED.load(Ordering::Relaxed)
}

pub fn show_dpi_osd(dpi: u32, stage: u8, total_stages: u8) {
    if !is_osd_enabled() {
        return;
    }

    IS_CUSTOM_MSG.store(false, Ordering::SeqCst);
    CURRENT_DPI.store(dpi, Ordering::SeqCst);
    CURRENT_STAGE.store(stage, Ordering::SeqCst);
    MAX_STAGES.store(total_stages, Ordering::SeqCst);

    ensure_osd_running();

    let hwnd = OSD_HWND.load(Ordering::SeqCst) as HWND;
    if hwnd != 0 as HWND {
        unsafe {
            PostMessageW(hwnd, WM_SHOW_OSD, 0, 0);
        }
    }
}

pub fn show_toast_osd(msg: &str) {
    if !is_osd_enabled() {
        return;
    }

    if let Ok(mut lock) = CUSTOM_MSG.lock() {
        *lock = msg.to_string();
    }
    IS_CUSTOM_MSG.store(true, Ordering::SeqCst);

    ensure_osd_running();

    let hwnd = OSD_HWND.load(Ordering::SeqCst) as HWND;
    if hwnd != 0 as HWND {
        unsafe {
            PostMessageW(hwnd, WM_SHOW_OSD, 0, 0);
        }
    }
}

fn ensure_osd_running() {
    INIT_ONCE.call_once(|| {
        thread::spawn(|| unsafe {
            run_osd_message_loop();
        });
        let (lock, cvar) = &OSD_READY;
        if let Ok(mut ready) = lock.lock() {
            while !*ready {
                let res = cvar.wait_timeout(ready, Duration::from_millis(500));
                match res {
                    Ok((guard, timeout)) => {
                        ready = guard;
                        if timeout.timed_out() {
                            break;
                        }
                    }
                    Err(_) => break,
                }
            }
        }
    });
}

unsafe extern "system" fn osd_wnd_proc(
    hwnd: HWND,
    msg: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    match msg {
        WM_SHOW_OSD => {
            let screen_w = GetSystemMetrics(SM_CXSCREEN);
            let screen_h = GetSystemMetrics(SM_CYSCREEN);
            let x = (screen_w - OSD_WIDTH) / 2;
            let y = screen_h - OSD_HEIGHT - 100;

            SetWindowPos(
                hwnd,
                HWND_TOPMOST,
                x,
                y,
                OSD_WIDTH,
                OSD_HEIGHT,
                SWP_NOACTIVATE | SWP_SHOWWINDOW,
            );
            ShowWindow(hwnd, SW_SHOWNOACTIVATE);
            InvalidateRect(hwnd, null_mut(), 0);

            KillTimer(hwnd, TIMER_HIDE_ID);
            SetTimer(hwnd, TIMER_HIDE_ID, 1200, None);
            0
        }
        WM_TIMER => {
            if wparam == TIMER_HIDE_ID {
                KillTimer(hwnd, TIMER_HIDE_ID);
                ShowWindow(hwnd, SW_HIDE);
            }
            0
        }
        WM_PAINT => {
            let mut ps: PAINTSTRUCT = std::mem::zeroed();
            let hdc = BeginPaint(hwnd, &mut ps);

            let is_custom = IS_CUSTOM_MSG.load(Ordering::SeqCst);
            let border_col = if is_custom { 0x0050E3C2 } else { 0x00F755A8 };
            let bg_brush = CreateSolidBrush(0x001E161A);
            let border_pen = CreatePen(PS_SOLID as i32, 1, border_col);

            let old_brush = SelectObject(hdc, bg_brush);
            let old_pen = SelectObject(hdc, border_pen);
            RoundRect(hdc, 0, 0, OSD_WIDTH, OSD_HEIGHT, 10, 10);
            SelectObject(hdc, old_brush);
            SelectObject(hdc, old_pen);
            DeleteObject(bg_brush);
            DeleteObject(border_pen);

            SetBkMode(hdc, TRANSPARENT as i32);

            let font_face = to_wide("Microsoft YaHei UI");
            let title_font = CreateFontW(
                -13,
                0,
                0,
                0,
                FW_SEMIBOLD as i32,
                0,
                0,
                0,
                1,
                0,
                0,
                CLEARTYPE_QUALITY as u32,
                0,
                font_face.as_ptr(),
            );
            let old_font = SelectObject(hdc, title_font);

            if is_custom {
                let msg_text = CUSTOM_MSG
                    .lock()
                    .map(|s| s.clone())
                    .unwrap_or_else(|_| "NZXT Lift 2".to_string());
                SetTextColor(hdc, 0x0050E3C2); // cyan/emerald
                let mut r_title = RECT {
                    left: 8,
                    top: 8,
                    right: OSD_WIDTH - 8,
                    bottom: 26,
                };
                DrawTextW(
                    hdc,
                    to_wide("NZXT LIFT 2").as_ptr(),
                    -1,
                    &mut r_title,
                    DT_CENTER | DT_SINGLELINE | DT_VCENTER,
                );

                let main_font = CreateFontW(
                    -16,
                    0,
                    0,
                    0,
                    FW_BOLD as i32,
                    0,
                    0,
                    0,
                    1,
                    0,
                    0,
                    CLEARTYPE_QUALITY as u32,
                    0,
                    font_face.as_ptr(),
                );
                SelectObject(hdc, main_font);
                SetTextColor(hdc, 0x00FFFFFF);
                let mut r_main = RECT {
                    left: 8,
                    top: 28,
                    right: OSD_WIDTH - 8,
                    bottom: 64,
                };
                DrawTextW(
                    hdc,
                    to_wide(&msg_text).as_ptr(),
                    -1,
                    &mut r_main,
                    DT_CENTER | DT_SINGLELINE | DT_VCENTER,
                );

                SelectObject(hdc, old_font);
                DeleteObject(title_font);
                DeleteObject(main_font);
            } else {
                let dpi = CURRENT_DPI.load(Ordering::SeqCst);
                let stage = CURRENT_STAGE.load(Ordering::SeqCst);
                let total = MAX_STAGES.load(Ordering::SeqCst);

                SetTextColor(hdc, 0x00C080FF);
                let mut r_title = RECT {
                    left: 8,
                    top: 8,
                    right: OSD_WIDTH - 8,
                    bottom: 26,
                };
                DrawTextW(
                    hdc,
                    to_wide("NZXT LIFT 2").as_ptr(),
                    -1,
                    &mut r_title,
                    DT_CENTER | DT_SINGLELINE | DT_VCENTER,
                );

                let main_font = CreateFontW(
                    -20,
                    0,
                    0,
                    0,
                    FW_BOLD as i32,
                    0,
                    0,
                    0,
                    1,
                    0,
                    0,
                    CLEARTYPE_QUALITY as u32,
                    0,
                    font_face.as_ptr(),
                );
                SelectObject(hdc, main_font);
                SetTextColor(hdc, 0x00FFFFFF);
                let mut r_main = RECT {
                    left: 8,
                    top: 28,
                    right: OSD_WIDTH - 8,
                    bottom: 64,
                };
                let text = format!("{} DPI   [档位 {}/{}]", dpi, stage, total);
                DrawTextW(
                    hdc,
                    to_wide(&text).as_ptr(),
                    -1,
                    &mut r_main,
                    DT_CENTER | DT_SINGLELINE | DT_VCENTER,
                );

                SelectObject(hdc, old_font);
                DeleteObject(title_font);
                DeleteObject(main_font);
            }

            EndPaint(hwnd, &ps);
            0
        }
        _ => DefWindowProcW(hwnd, msg, wparam, lparam),
    }
}

unsafe fn run_osd_message_loop() {
    let class_name = to_wide("NZXTLift2OSD");

    let wnd_class = WNDCLASSEXW {
        cbSize: std::mem::size_of::<WNDCLASSEXW>() as u32,
        style: CS_HREDRAW | CS_VREDRAW,
        lpfnWndProc: Some(osd_wnd_proc),
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
        WS_EX_LAYERED | WS_EX_TRANSPARENT | WS_EX_TOPMOST | WS_EX_TOOLWINDOW | WS_EX_NOACTIVATE,
        class_name.as_ptr(),
        to_wide("Lift2 OSD").as_ptr(),
        WS_POPUP,
        0,
        0,
        OSD_WIDTH,
        OSD_HEIGHT,
        0 as HWND,
        0 as _,
        0 as _,
        null_mut(),
    );

    SetLayeredWindowAttributes(hwnd, 0, 235, LWA_ALPHA);
    OSD_HWND.store(hwnd as isize, Ordering::SeqCst);

    let (lock, cvar) = &OSD_READY;
    if let Ok(mut ready) = lock.lock() {
        *ready = true;
        cvar.notify_all();
    }

    let mut msg = std::mem::zeroed();
    while GetMessageW(&mut msg, 0 as HWND, 0, 0) > 0 {
        TranslateMessage(&msg);
        DispatchMessageW(&msg);
    }
}

fn to_wide(s: &str) -> Vec<u16> {
    OsStr::new(s).encode_wide().chain(std::iter::once(0)).collect()
}
