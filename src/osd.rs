use std::ffi::OsStr;
use std::os::windows::ffi::OsStrExt;
use std::ptr::null_mut;
use std::sync::atomic::{AtomicU32, AtomicU8, Ordering};
use std::sync::Once;
use std::thread;

use windows_sys::Win32::Foundation::{HWND, LPARAM, LRESULT, RECT, WPARAM};
use windows_sys::Win32::Graphics::Gdi::{
    BeginPaint, CreateFontW, CreateSolidBrush, DeleteObject, DrawTextW, EndPaint,
    FillRect, SelectObject, SetBkMode, SetTextColor, DT_CENTER,
    DT_SINGLELINE, DT_VCENTER, FW_BOLD, FW_SEMIBOLD, PAINTSTRUCT, TRANSPARENT,
};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, DefWindowProcW, DispatchMessageW, GetMessageW, GetSystemMetrics,
    KillTimer, PostMessageW, RegisterClassExW, SetLayeredWindowAttributes, SetTimer,
    SetWindowPos, ShowWindow, TranslateMessage, CS_HREDRAW, CS_VREDRAW, HICON,
    HWND_TOPMOST, LWA_ALPHA, SM_CXSCREEN, SM_CYSCREEN, SWP_NOACTIVATE,
    SWP_SHOWWINDOW, SW_HIDE, SW_SHOWNOACTIVATE, WM_PAINT, WM_TIMER,
    WM_USER, WNDCLASSEXW, WS_EX_LAYERED, WS_EX_NOACTIVATE, WS_EX_TOOLWINDOW,
    WS_EX_TOPMOST, WS_EX_TRANSPARENT, WS_POPUP,
};

static INIT_ONCE: Once = Once::new();
static CURRENT_DPI: AtomicU32 = AtomicU32::new(1600);
static CURRENT_STAGE: AtomicU8 = AtomicU8::new(3);
static MAX_STAGES: AtomicU8 = AtomicU8::new(5);
static IS_CUSTOM_MSG: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
static CUSTOM_MSG: std::sync::Mutex<String> = std::sync::Mutex::new(String::new());
static mut OSD_HWND: HWND = 0 as HWND;

const WM_SHOW_OSD: u32 = WM_USER + 101;
const TIMER_HIDE_ID: usize = 1;

pub fn show_dpi_osd(dpi: u32, stage: u8, total_stages: u8) {
    IS_CUSTOM_MSG.store(false, Ordering::Relaxed);
    CURRENT_DPI.store(dpi, Ordering::Relaxed);
    CURRENT_STAGE.store(stage, Ordering::Relaxed);
    MAX_STAGES.store(total_stages, Ordering::Relaxed);

    ensure_osd_running();

    unsafe {
        if OSD_HWND != 0 as HWND {
            PostMessageW(OSD_HWND, WM_SHOW_OSD, 0, 0);
        }
    }
}

pub fn show_toast_osd(msg: &str) {
    if let Ok(mut lock) = CUSTOM_MSG.lock() {
        *lock = msg.to_string();
    }
    IS_CUSTOM_MSG.store(true, Ordering::Relaxed);

    ensure_osd_running();

    unsafe {
        if OSD_HWND != 0 as HWND {
            PostMessageW(OSD_HWND, WM_SHOW_OSD, 0, 0);
        }
    }
}

fn ensure_osd_running() {
    INIT_ONCE.call_once(|| {
        thread::spawn(|| {
            unsafe {
                run_osd_message_loop();
            }
        });
        thread::sleep(std::time::Duration::from_millis(50));
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
            let win_w = 260;
            let win_h = 70;
            let x = (screen_w - win_w) / 2;
            let y = screen_h - win_h - 100;

            SetWindowPos(
                hwnd,
                HWND_TOPMOST,
                x,
                y,
                win_w,
                win_h,
                SWP_NOACTIVATE | SWP_SHOWWINDOW,
            );
            ShowWindow(hwnd, SW_SHOWNOACTIVATE);

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

            let mut rect: RECT = std::mem::zeroed();
            rect.right = 260;
            rect.bottom = 70;

            let bg_brush = CreateSolidBrush(0x00181818);
            FillRect(hdc, &rect, bg_brush);
            DeleteObject(bg_brush);

            SetBkMode(hdc, TRANSPARENT as i32);

            if IS_CUSTOM_MSG.load(Ordering::Relaxed) {
                let msg_text = CUSTOM_MSG.lock().map(|s| s.clone()).unwrap_or_else(|_| "Reset".to_string());
                let title_font = CreateFontW(
                    14, 0, 0, 0, FW_SEMIBOLD as i32, 0, 0, 0, 0, 0, 0, 0, 0,
                    to_wide("Segoe UI").as_ptr(),
                );
                let old_font = SelectObject(hdc, title_font);
                SetTextColor(hdc, 0x0050E3C2); // cyan/emerald
                let mut r_title = RECT { left: 0, top: 8, right: 260, bottom: 26 };
                DrawTextW(hdc, to_wide("NZXT LIFT 2").as_ptr(), -1, &mut r_title, DT_CENTER | DT_SINGLELINE | DT_VCENTER);

                let main_font = CreateFontW(
                    18, 0, 0, 0, FW_BOLD as i32, 0, 0, 0, 0, 0, 0, 0, 0,
                    to_wide("Segoe UI").as_ptr(),
                );
                SelectObject(hdc, main_font);
                SetTextColor(hdc, 0x00FFFFFF);
                let mut r_main = RECT { left: 0, top: 28, right: 260, bottom: 62 };
                DrawTextW(hdc, to_wide(&msg_text).as_ptr(), -1, &mut r_main, DT_CENTER | DT_SINGLELINE | DT_VCENTER);

                SelectObject(hdc, old_font);
                DeleteObject(title_font);
                DeleteObject(main_font);
            } else {
                let dpi = CURRENT_DPI.load(Ordering::Relaxed);
                let stage = CURRENT_STAGE.load(Ordering::Relaxed);
                let total = MAX_STAGES.load(Ordering::Relaxed);

                let title_font = CreateFontW(
                    14, 0, 0, 0, FW_SEMIBOLD as i32, 0, 0, 0, 0, 0, 0, 0, 0,
                    to_wide("Segoe UI").as_ptr(),
                );
                let old_font = SelectObject(hdc, title_font);
                SetTextColor(hdc, 0x00C080FF);
                let mut r_title = RECT { left: 0, top: 8, right: 260, bottom: 26 };
                DrawTextW(hdc, to_wide("NZXT LIFT 2").as_ptr(), -1, &mut r_title, DT_CENTER | DT_SINGLELINE | DT_VCENTER);

                let main_font = CreateFontW(
                    22, 0, 0, 0, FW_BOLD as i32, 0, 0, 0, 0, 0, 0, 0, 0,
                    to_wide("Segoe UI").as_ptr(),
                );
                SelectObject(hdc, main_font);
                SetTextColor(hdc, 0x00FFFFFF);
                let mut r_main = RECT { left: 0, top: 28, right: 260, bottom: 62 };
                let text = format!("{} DPI   [Stage {}/{}]", dpi, stage, total);
                DrawTextW(hdc, to_wide(&text).as_ptr(), -1, &mut r_main, DT_CENTER | DT_SINGLELINE | DT_VCENTER);

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
        260,
        70,
        0 as HWND,
        0 as _,
        0 as _,
        null_mut(),
    );

    OSD_HWND = hwnd;

    SetLayeredWindowAttributes(hwnd, 0, 230, LWA_ALPHA);

    let mut msg = std::mem::zeroed();
    while GetMessageW(&mut msg, 0 as HWND, 0, 0) > 0 {
        TranslateMessage(&msg);
        DispatchMessageW(&msg);
    }
}

fn to_wide(s: &str) -> Vec<u16> {
    OsStr::new(s).encode_wide().chain(std::iter::once(0)).collect()
}
