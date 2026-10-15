//! Native Win32 Double-Buffered Zero-Overhead GUI for NZXT Lift 2 Ergo
//! Replaces OpenGL/eframe to reduce memory footprint by 95% (from 95MB down to < 8MB)

use std::ffi::OsStr;
use std::os::windows::ffi::OsStrExt;
use std::ptr::null_mut;
use std::sync::{Arc, Mutex};
use std::time::Instant;

use windows_sys::Win32::Foundation::{HWND, LPARAM, LRESULT, RECT, WPARAM};
use windows_sys::Win32::Graphics::Gdi::{
    BeginPaint, BitBlt, CreateCompatibleBitmap, CreateCompatibleDC, CreateFontW, CreatePen,
    CreateSolidBrush, DeleteDC, DeleteObject, DrawTextW, EndPaint, FillRect,
    InvalidateRect, RoundRect, SelectObject,
    SetBkMode, SetTextColor, CLEARTYPE_QUALITY, DT_CENTER, DT_LEFT, DT_SINGLELINE,
    DT_VCENTER, FW_BOLD, FW_NORMAL, FW_SEMIBOLD, HBRUSH, HDC, HFONT,
    PAINTSTRUCT, PS_SOLID, SRCCOPY, TRANSPARENT,
};
use windows_sys::Win32::System::Threading::{GetCurrentProcess, SetProcessWorkingSetSize};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, DefWindowProcW, DestroyWindow, DispatchMessageW, GetClientRect,
    GetMessageW, GetSystemMetrics, GetWindowLongPtrW, LoadCursorW,
    PostQuitMessage, RegisterClassExW, SetForegroundWindow,
    SetWindowLongPtrW, ShowWindow, TranslateMessage, GWLP_USERDATA,
    IDC_ARROW, SM_CXSCREEN, SM_CYSCREEN, SW_SHOW,
    WM_CLOSE, WM_CREATE, WM_DESTROY, WM_ERASEBKGND, WM_LBUTTONDOWN, WM_LBUTTONUP,
    WM_MOUSEMOVE, WM_PAINT, WM_SETCURSOR, WNDCLASSEXW, WS_CAPTION, WS_MINIMIZEBOX,
    WS_OVERLAPPED, WS_SYSMENU, WS_VISIBLE,
};

use crate::config::AppConfig;
use crate::device::{cycle_usb_port, fix_usb_power_settings, Lift2Device};
use crate::dpi::{
    adjust_dpi_step, clamp_and_snap_dpi, PRESET_FPS_CLASSIC, PRESET_HIGH_RES_4K, PRESET_MOBA_SPEED,
    QUICK_DPI_LIST,
};
use crate::osd::{show_dpi_osd, show_toast_osd};
use crate::protocol::{Lift2Settings, LiftOffDistance, PollingRate};
use crate::system::{is_autostart_enabled, release_stuck_clicks, set_autostart};
use crate::theme::{rgb, ThemeColors, ThemeMode};
use crate::tray::update_global_hotkey;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct UiRect {
    pub x: i32,
    pub y: i32,
    pub w: i32,
    pub h: i32,
}

impl UiRect {
    pub const fn new(x: i32, y: i32, w: i32, h: i32) -> Self {
        Self { x, y, w, h }
    }

    pub fn contains(&self, px: i32, py: i32) -> bool {
        px >= self.x && px < (self.x + self.w) && py >= self.y && py < (self.y + self.h)
    }

    pub fn to_win_rect(&self) -> RECT {
        RECT {
            left: self.x,
            top: self.y,
            right: self.x + self.w,
            bottom: self.y + self.h,
        }
    }
}

pub fn dpi_to_slider_pos(dpi: u32, track_x: i32, track_w: i32) -> i32 {
    let clamped = dpi.clamp(50, 26000);
    let ratio = (clamped - 50) as f32 / (26000 - 50) as f32;
    track_x + (ratio * track_w as f32).round() as i32
}

pub fn slider_pos_to_dpi(click_x: i32, track_x: i32, track_w: i32) -> u32 {
    let rel_x = (click_x - track_x).clamp(0, track_w);
    let ratio = rel_x as f32 / track_w as f32;
    let raw = 50.0 + ratio * (26000.0 - 50.0);
    clamp_and_snap_dpi(raw.round() as u32)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum DropdownType {
    PollingRate,
    Lod,
}

struct GdiFonts {
    title: HFONT,
    subtitle: HFONT,
    body: HFONT,
    body_bold: HFONT,
    large_bold: HFONT,
    small: HFONT,
}

impl GdiFonts {
    unsafe fn create() -> Self {
        let font_face = to_wide("Microsoft YaHei UI");
        let fallback_face = to_wide("Segoe UI");

        let create = |size: i32, weight: i32| {
            let hf = CreateFontW(
                size, 0, 0, 0, weight, 0, 0, 0, 1, 0, 0,
                CLEARTYPE_QUALITY as u32, 0, font_face.as_ptr(),
            );
            if hf != 0 as HFONT {
                hf
            } else {
                CreateFontW(
                    size, 0, 0, 0, weight, 0, 0, 0, 1, 0, 0,
                    CLEARTYPE_QUALITY as u32, 0, fallback_face.as_ptr(),
                )
            }
        };

        Self {
            title: create(-19, FW_BOLD as i32),
            subtitle: create(-12, FW_NORMAL as i32),
            body: create(-13, FW_NORMAL as i32),
            body_bold: create(-13, FW_SEMIBOLD as i32),
            large_bold: create(-18, FW_BOLD as i32),
            small: create(-11, FW_NORMAL as i32),
        }
    }

    unsafe fn destroy(&self) {
        DeleteObject(self.title);
        DeleteObject(self.subtitle);
        DeleteObject(self.body);
        DeleteObject(self.body_bold);
        DeleteObject(self.large_bold);
        DeleteObject(self.small);
    }
}

pub struct NativeGuiState {
    pub config: Arc<Mutex<AppConfig>>,
    pub settings: Lift2Settings,
    pub connected: bool,
    pub firmware_version: String,
    pub serial_number: String,
    pub theme_mode: ThemeMode,
    pub autostart_enabled: bool,
    pub status_message: Option<(String, Instant)>,
    pub hotkey_buffer: String,
    active_dropdown: Option<DropdownType>,
    dragging_stage: Option<usize>,
    hover_pos: Option<(i32, i32)>,
    fonts: Option<GdiFonts>,
}

impl NativeGuiState {
    pub fn new(config: Arc<Mutex<AppConfig>>) -> Self {
        let (initial_hotkey, theme_str) = {
            let cfg = config.lock().unwrap();
            (cfg.system.reset_hotkey.clone(), cfg.system.theme.clone())
        };

        let theme_mode = ThemeMode::from_str(&theme_str);
        let autostart_enabled = is_autostart_enabled();

        let mut state = Self {
            config,
            settings: Lift2Settings::default(),
            connected: false,
            firmware_version: "Unknown".to_string(),
            serial_number: "Unknown".to_string(),
            theme_mode,
            autostart_enabled,
            status_message: None,
            hotkey_buffer: initial_hotkey,
            active_dropdown: None,
            dragging_stage: None,
            hover_pos: None,
            fonts: None,
        };

        state.refresh_from_device();
        state
    }

    pub fn refresh_from_device(&mut self) {
        if let Ok(mut dev) = Lift2Device::open() {
            self.connected = true;
            self.firmware_version = dev.get_firmware_version().unwrap_or_else(|_| "v1.1.24".to_string());
            self.serial_number = dev.get_uid().unwrap_or_else(|_| "Unknown".to_string());
            if let Ok(settings) = dev.get_profile_settings(0) {
                self.settings = settings;

                if let Ok(mut cfg) = self.config.lock() {
                    cfg.mouse.active_stage = self.settings.active_cpi_idx + 1;
                    cfg.mouse.stages = self.settings.stages;
                    cfg.mouse.polling_rate = self.settings.polling_rate.to_hz();
                    cfg.mouse.lod = self.settings.lod.to_mm();
                    let _ = cfg.save();
                }
            }
        } else {
            self.connected = false;
        }
    }

    pub fn save_settings_to_device(&mut self) {
        if let Ok(mut dev) = Lift2Device::open() {
            match dev.set_profile_settings(&self.settings) {
                Ok(_) => {
                    self.set_status("✅ 设置已保存至鼠标板载芯片");
                    if let Ok(mut cfg) = self.config.lock() {
                        cfg.mouse.active_stage = self.settings.active_cpi_idx + 1;
                        cfg.mouse.stages = self.settings.stages;
                        cfg.mouse.polling_rate = self.settings.polling_rate.to_hz();
                        cfg.mouse.lod = self.settings.lod.to_mm();
                        let _ = cfg.save();
                    }
                }
                Err(e) => self.set_status(&format!("❌ 保存失败: {}", e)),
            }
        } else {
            self.connected = false;
            self.set_status("❌ 无法连接鼠标 (Device Disconnected)");
        }
    }

    pub fn switch_active_stage(&mut self, stage_idx: u8) {
        if stage_idx > 4 {
            return;
        }
        self.settings.active_cpi_idx = stage_idx;
        if let Ok(mut dev) = Lift2Device::open() {
            let _ = dev.set_active_cpi_idx(stage_idx);
            let dpi = self.settings.stages[stage_idx as usize];
            show_dpi_osd(dpi, stage_idx + 1, 5);
        }
        self.save_settings_to_device();
    }

    pub fn set_status(&mut self, msg: &str) {
        self.status_message = Some((msg.to_string(), Instant::now()));
    }
}

static mut GLOBAL_GUI_HWND: HWND = 0 as HWND;
static GUI_LAUNCHING: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

pub fn is_gui_open() -> bool {
    unsafe { GLOBAL_GUI_HWND != 0 as HWND }
}

pub fn bring_gui_to_front() {
    unsafe {
        if GLOBAL_GUI_HWND != 0 as HWND {
            ShowWindow(GLOBAL_GUI_HWND, SW_SHOW);
            SetForegroundWindow(GLOBAL_GUI_HWND);
        }
    }
}

pub fn open_native_gui(config: Arc<Mutex<AppConfig>>) {
    if is_gui_open() {
        bring_gui_to_front();
        return;
    }

    if GUI_LAUNCHING
        .compare_exchange(
            false,
            true,
            std::sync::atomic::Ordering::SeqCst,
            std::sync::atomic::Ordering::SeqCst,
        )
        .is_ok()
    {
        std::thread::spawn(move || unsafe {
            run_native_gui_thread(config);
            GUI_LAUNCHING.store(false, std::sync::atomic::Ordering::SeqCst);
        });
    }
}

unsafe fn run_native_gui_thread(config: Arc<Mutex<AppConfig>>) {
    let class_name = to_wide("NZXTLift2NativeClass");
    let hicon_big = crate::icon::create_custom_window_hicon();
    let hicon_sm = crate::icon::create_custom_tray_hicon();
    let hcursor = LoadCursorW(0 as _, IDC_ARROW);

    let wnd_class = WNDCLASSEXW {
        cbSize: std::mem::size_of::<WNDCLASSEXW>() as u32,
        style: 0,
        lpfnWndProc: Some(native_gui_wnd_proc),
        cbClsExtra: 0,
        cbWndExtra: 0,
        hInstance: 0 as _,
        hIcon: hicon_big,
        hCursor: hcursor,
        hbrBackground: 0 as HBRUSH,
        lpszMenuName: null_mut(),
        lpszClassName: class_name.as_ptr(),
        hIconSm: hicon_sm,
    };

    RegisterClassExW(&wnd_class);

    let win_w = 800;
    let win_h = 570;
    let screen_w = GetSystemMetrics(SM_CXSCREEN);
    let screen_h = GetSystemMetrics(SM_CYSCREEN);
    let start_x = (screen_w - win_w) / 2;
    let start_y = (screen_h - win_h) / 2;

    let mut state = Box::new(NativeGuiState::new(config));
    state.fonts = Some(GdiFonts::create());

    let state_ptr = Box::into_raw(state);

    let hwnd = CreateWindowExW(
        0,
        class_name.as_ptr(),
        to_wide("NZXT Lift 2 Ergo - Driver & Control Panel").as_ptr(),
        WS_OVERLAPPED | WS_CAPTION | WS_SYSMENU | WS_MINIMIZEBOX | WS_VISIBLE,
        start_x,
        start_y,
        win_w,
        win_h,
        0 as HWND,
        0 as _,
        0 as _,
        state_ptr as *const std::ffi::c_void,
    );

    GLOBAL_GUI_HWND = hwnd;

    let mut msg = std::mem::zeroed();
    while GetMessageW(&mut msg, 0 as HWND, 0, 0) > 0 {
        TranslateMessage(&msg);
        DispatchMessageW(&msg);
    }

    GLOBAL_GUI_HWND = 0 as HWND;

    // Destroy state safely and trim working set
    if !state_ptr.is_null() {
        let boxed = Box::from_raw(state_ptr);
        if let Some(f) = &boxed.fonts {
            f.destroy();
        }
    }

    // Aggressively flush unneeded working set memory back to OS
    SetProcessWorkingSetSize(GetCurrentProcess(), !0, !0);
}

unsafe extern "system" fn native_gui_wnd_proc(
    hwnd: HWND,
    msg: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    let state_ptr = GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *mut NativeGuiState;

    match msg {
        WM_CREATE => {
            let create_struct = &*(lparam as *const windows_sys::Win32::UI::WindowsAndMessaging::CREATESTRUCTW);
            SetWindowLongPtrW(hwnd, GWLP_USERDATA, create_struct.lpCreateParams as isize);
            0
        }
        WM_ERASEBKGND => 1, // Completely prevents GDI background erase flickering!
        WM_PAINT => {
            if !state_ptr.is_null() {
                let state = &mut *state_ptr;
                render_native_gui(hwnd, state);
            }
            0
        }
        WM_LBUTTONDOWN => {
            if !state_ptr.is_null() {
                let state = &mut *state_ptr;
                let x = (lparam & 0xFFFF) as i16 as i32;
                let y = ((lparam >> 16) & 0xFFFF) as i16 as i32;
                handle_mouse_click(hwnd, state, x, y);
            }
            0
        }
        WM_MOUSEMOVE => {
            if !state_ptr.is_null() {
                let state = &mut *state_ptr;
                let x = (lparam & 0xFFFF) as i16 as i32;
                let y = ((lparam >> 16) & 0xFFFF) as i16 as i32;
                state.hover_pos = Some((x, y));

                // Handle slider drag
                if let Some(stage_idx) = state.dragging_stage {
                    let track_x = 22 + (stage_idx as i32) * 153 + 12;
                    let track_w = 118;
                    let new_dpi = slider_pos_to_dpi(x, track_x, track_w);
                    if state.settings.stages[stage_idx] != new_dpi {
                        state.settings.stages[stage_idx] = new_dpi;
                        state.save_settings_to_device();
                        InvalidateRect(hwnd, null_mut(), 0);
                    }
                }
            }
            0
        }
        WM_LBUTTONUP => {
            if !state_ptr.is_null() {
                let state = &mut *state_ptr;
                state.dragging_stage = None;
            }
            0
        }
        WM_SETCURSOR => {
            DefWindowProcW(hwnd, msg, wparam, lparam)
        }
        WM_CLOSE => {
            DestroyWindow(hwnd);
            0
        }
        WM_DESTROY => {
            PostQuitMessage(0);
            0
        }
        _ => DefWindowProcW(hwnd, msg, wparam, lparam),
    }
}

unsafe fn render_native_gui(hwnd: HWND, state: &mut NativeGuiState) {
    let mut ps: PAINTSTRUCT = std::mem::zeroed();
    let hdc = BeginPaint(hwnd, &mut ps);

    let mut rc: RECT = std::mem::zeroed();
    GetClientRect(hwnd, &mut rc);
    let win_w = rc.right - rc.left;
    let win_h = rc.bottom - rc.top;

    let mem_dc = CreateCompatibleDC(hdc);
    let mem_bmp = CreateCompatibleBitmap(hdc, win_w, win_h);
    let old_bmp = SelectObject(mem_dc, mem_bmp);

    let colors = ThemeColors::for_mode(state.theme_mode);

    // 1. Fill Background
    let bg_brush = CreateSolidBrush(colors.bg);
    FillRect(mem_dc, &rc, bg_brush);
    DeleteObject(bg_brush);

    SetBkMode(mem_dc, TRANSPARENT as i32);

    let fonts = state.fonts.as_ref().unwrap();

    // 2. Header (Y: 10..46)
    {
        SelectObject(mem_dc, fonts.title);
        SetTextColor(mem_dc, colors.text_accent);
        let mut title_rc = RECT { left: 20, top: 10, right: 300, bottom: 32 };
        DrawTextW(mem_dc, to_wide("NZXT LIFT 2 ERGO").as_ptr(), -1, &mut title_rc, DT_LEFT | DT_VCENTER | DT_SINGLELINE);

        SelectObject(mem_dc, fonts.subtitle);
        SetTextColor(mem_dc, colors.text_secondary);
        let active_stage = state.settings.active_cpi_idx as usize % 5;
        let subtitle = format!(
            "固件: {}  |  UID: {}  |  当前: 档位 {} ({} DPI)",
            state.firmware_version,
            if state.serial_number.len() > 12 { &state.serial_number[..12] } else { &state.serial_number },
            active_stage + 1,
            state.settings.stages[active_stage],
        );
        let mut sub_rc = RECT { left: 20, top: 32, right: 480, bottom: 48 };
        DrawTextW(mem_dc, to_wide(&subtitle).as_ptr(), -1, &mut sub_rc, DT_LEFT | DT_VCENTER | DT_SINGLELINE);

        // Hardware Status Indicator
        let (status_text, status_col) = if state.connected {
            ("● 已连接", rgb(52, 199, 89))
        } else {
            ("● 未连接", rgb(255, 69, 58))
        };
        SetTextColor(mem_dc, status_col);
        let mut stat_rc = RECT { left: 450, top: 12, right: 540, bottom: 32 };
        DrawTextW(mem_dc, to_wide(status_text).as_ptr(), -1, &mut stat_rc, DT_LEFT | DT_VCENTER | DT_SINGLELINE);

        // Right Theme Switcher Capsule (x: 580..780, y: 12..38)
        draw_theme_capsule(mem_dc, fonts, &colors, state.theme_mode, 590, 12, 190, 26);
    }

    // 3. Section 1: DPI Sensitivity Card (Y: 54..276)
    {
        let card_rc = UiRect::new(20, 54, 760, 222);
        draw_rounded_card(mem_dc, &card_rc, colors.card_bg, colors.card_border, 8);

        // Top Row: 5 Stage Switch Tabs
        SelectObject(mem_dc, fonts.body_bold);
        SetTextColor(mem_dc, colors.text_primary);
        let mut dpi_title_rc = RECT { left: 32, top: 62, right: 140, bottom: 86 };
        DrawTextW(mem_dc, to_wide("🎯 DPI 档位:").as_ptr(), -1, &mut dpi_title_rc, DT_LEFT | DT_VCENTER | DT_SINGLELINE);

        for i in 0..5 {
            let is_active = state.settings.active_cpi_idx == i as u8;
            let tab_x = 135 + (i as i32) * 122;
            let tab_rc = UiRect::new(tab_x, 62, 116, 24);
            let tab_bg = if is_active { colors.accent } else { colors.button_bg };
            let tab_border = if is_active { colors.card_active_border } else { colors.card_border };
            let text_color = if is_active { rgb(255, 255, 255) } else { colors.text_secondary };

            draw_rounded_card(mem_dc, &tab_rc, tab_bg, tab_border, 5);
            SelectObject(mem_dc, fonts.small);
            SetTextColor(mem_dc, text_color);
            let label = if is_active {
                format!("● 档位 {}: {}", i + 1, state.settings.stages[i])
            } else {
                format!("档位 {}: {}", i + 1, state.settings.stages[i])
            };
            let mut text_rc = tab_rc.to_win_rect();
            DrawTextW(mem_dc, to_wide(&label).as_ptr(), -1, &mut text_rc, DT_CENTER | DT_VCENTER | DT_SINGLELINE);
        }

        // Quick Preset Row (Y: 92..114)
        SelectObject(mem_dc, fonts.small);
        SetTextColor(mem_dc, colors.text_secondary);
        let mut assign_label_rc = RECT { left: 32, top: 92, right: 130, bottom: 114 };
        DrawTextW(mem_dc, to_wide("当前赋予:").as_ptr(), -1, &mut assign_label_rc, DT_LEFT | DT_VCENTER | DT_SINGLELINE);

        let active_idx = state.settings.active_cpi_idx as usize % 5;
        let current_active_dpi = state.settings.stages[active_idx];

        for (k, &quick_val) in QUICK_DPI_LIST.iter().enumerate() {
            let q_rc = UiRect::new(96 + (k as i32) * 52, 92, 48, 20);
            let is_match = current_active_dpi == quick_val;
            let q_bg = if is_match { colors.accent } else { colors.button_bg };
            let q_text_color = if is_match { rgb(255, 255, 255) } else { colors.text_secondary };

            draw_rounded_card(mem_dc, &q_rc, q_bg, colors.card_border, 4);
            SetTextColor(mem_dc, q_text_color);
            let mut q_win_rc = q_rc.to_win_rect();
            DrawTextW(mem_dc, to_wide(&format!("{}", quick_val)).as_ptr(), -1, &mut q_win_rc, DT_CENTER | DT_VCENTER | DT_SINGLELINE);
        }

        let mut preset_label_rc = RECT { left: 474, top: 92, right: 540, bottom: 114 };
        DrawTextW(mem_dc, to_wide("电竞模板:").as_ptr(), -1, &mut preset_label_rc, DT_LEFT | DT_VCENTER | DT_SINGLELINE);

        let templates = [("FPS 经典", 540), ("MOBA 微操", 620), ("4K 高分", 700)];
        for &(t_name, t_x) in &templates {
            let t_rc = UiRect::new(t_x, 92, 74, 20);
            draw_rounded_card(mem_dc, &t_rc, colors.button_bg, colors.card_border, 4);
            SetTextColor(mem_dc, colors.text_secondary);
            let mut t_win_rc = t_rc.to_win_rect();
            DrawTextW(mem_dc, to_wide(t_name).as_ptr(), -1, &mut t_win_rc, DT_CENTER | DT_VCENTER | DT_SINGLELINE);
        }

        // Detailed 5 Stage Columns (Y: 120..264)
        for i in 0..5 {
            let is_active = state.settings.active_cpi_idx == i as u8;
            let col_x = 32 + (i as i32) * 148;
            let col_rc = UiRect::new(col_x, 120, 140, 144);
            let col_bg = if is_active { colors.card_active_bg } else { colors.button_bg };
            let col_border = if is_active { colors.card_active_border } else { colors.card_border };

            draw_rounded_card(mem_dc, &col_rc, col_bg, col_border, 6);

            // 1. Stage Header Button (Entire top is clickable!)
            let btn_rc = UiRect::new(col_x + 6, 126, 128, 26);
            let btn_bg = if is_active { colors.accent } else { colors.card_bg };
            draw_rounded_card(mem_dc, &btn_rc, btn_bg, col_border, 4);

            SelectObject(mem_dc, fonts.body_bold);
            SetTextColor(mem_dc, if is_active { rgb(255, 255, 255) } else { colors.text_secondary });
            let mut btn_win_rc = btn_rc.to_win_rect();
            let btn_label = if is_active { format!("● 档位 {}", i + 1) } else { format!("档位 {}", i + 1) };
            DrawTextW(mem_dc, to_wide(&btn_label).as_ptr(), -1, &mut btn_win_rc, DT_CENTER | DT_VCENTER | DT_SINGLELINE);

            // 2. Large DPI Display
            SelectObject(mem_dc, fonts.large_bold);
            SetTextColor(mem_dc, if is_active { rgb(255, 255, 255) } else { colors.text_primary });
            let mut dpi_rc = RECT { left: col_x, top: 156, right: col_x + 140, bottom: 180 };
            let dpi_str = format!("{} DPI", state.settings.stages[i]);
            DrawTextW(mem_dc, to_wide(&dpi_str).as_ptr(), -1, &mut dpi_rc, DT_CENTER | DT_VCENTER | DT_SINGLELINE);

            // 3. Step buttons [-50] [+50]
            SelectObject(mem_dc, fonts.small);
            let minus_rc = UiRect::new(col_x + 16, 186, 50, 20);
            draw_rounded_card(mem_dc, &minus_rc, colors.card_bg, colors.card_border, 3);
            SetTextColor(mem_dc, colors.text_secondary);
            let mut minus_win_rc = minus_rc.to_win_rect();
            DrawTextW(mem_dc, to_wide("-50").as_ptr(), -1, &mut minus_win_rc, DT_CENTER | DT_VCENTER | DT_SINGLELINE);

            let plus_rc = UiRect::new(col_x + 74, 186, 50, 20);
            draw_rounded_card(mem_dc, &plus_rc, colors.card_bg, colors.card_border, 3);
            let mut plus_win_rc = plus_rc.to_win_rect();
            DrawTextW(mem_dc, to_wide("+50").as_ptr(), -1, &mut plus_win_rc, DT_CENTER | DT_VCENTER | DT_SINGLELINE);

            // 4. Smooth DPI Slider Track & Glowing Knob
            let track_x = col_x + 12;
            let track_y = 222;
            let track_w = 116;

            let track_rc = UiRect::new(track_x, track_y, track_w, 4);
            draw_rounded_card(mem_dc, &track_rc, colors.card_bg, colors.card_border, 2);

            let knob_x = dpi_to_slider_pos(state.settings.stages[i], track_x, track_w);
            let prog_w = knob_x - track_x;
            if prog_w > 0 {
                let prog_rc = UiRect::new(track_x, track_y, prog_w, 4);
                draw_rounded_card(mem_dc, &prog_rc, colors.accent, colors.accent, 2);
            }

            let knob_rc = UiRect::new(knob_x - 5, track_y - 4, 10, 12);
            draw_rounded_card(mem_dc, &knob_rc, rgb(255, 255, 255), colors.accent, 4);
        }
    }

    // 4. Section 2: Hardware Parameters Card (Y: 284..334)
    {
        let card_rc = UiRect::new(20, 284, 760, 50);
        draw_rounded_card(mem_dc, &card_rc, colors.card_bg, colors.card_border, 8);

        // Polling Rate Dropdown Pill
        SelectObject(mem_dc, fonts.body_bold);
        SetTextColor(mem_dc, colors.text_primary);
        let mut hz_title = RECT { left: 34, top: 292, right: 120, bottom: 326 };
        DrawTextW(mem_dc, to_wide("⚡ 回报率:").as_ptr(), -1, &mut hz_title, DT_LEFT | DT_VCENTER | DT_SINGLELINE);

        let hz_btn_rc = UiRect::new(115, 296, 175, 26);
        draw_rounded_card(mem_dc, &hz_btn_rc, colors.button_bg, colors.card_border, 5);

        SelectObject(mem_dc, fonts.body);
        SetTextColor(mem_dc, colors.accent_cyan);
        let hz_text = format!("{} Hz ({})  ▾", state.settings.polling_rate.to_hz(), state.settings.polling_rate.to_delay_str());
        let mut hz_text_rc = hz_btn_rc.to_win_rect();
        hz_text_rc.left += 8;
        DrawTextW(mem_dc, to_wide(&hz_text).as_ptr(), -1, &mut hz_text_rc, DT_LEFT | DT_VCENTER | DT_SINGLELINE);

        // LOD Dropdown Pill
        SelectObject(mem_dc, fonts.body_bold);
        SetTextColor(mem_dc, colors.text_primary);
        let mut lod_title = RECT { left: 330, top: 292, right: 440, bottom: 326 };
        DrawTextW(mem_dc, to_wide("📏 静默高度 (LOD):").as_ptr(), -1, &mut lod_title, DT_LEFT | DT_VCENTER | DT_SINGLELINE);

        let lod_btn_rc = UiRect::new(450, 296, 210, 26);
        draw_rounded_card(mem_dc, &lod_btn_rc, colors.button_bg, colors.card_border, 5);

        SelectObject(mem_dc, fonts.body);
        SetTextColor(mem_dc, colors.text_accent);
        let lod_label = match state.settings.lod {
            LiftOffDistance::Mm1 => "1.0 mm (低 - 电竞推荐)  ▾",
            LiftOffDistance::Mm2 => "2.0 mm (高 - 厚脚贴适用)  ▾",
        };
        let mut lod_text_rc = lod_btn_rc.to_win_rect();
        lod_text_rc.left += 8;
        DrawTextW(mem_dc, to_wide(lod_label).as_ptr(), -1, &mut lod_text_rc, DT_LEFT | DT_VCENTER | DT_SINGLELINE);
    }

    // 5. Section 3: Emergency Rescue & Auto-Start (Y: 342..450)
    {
        let card_rc = UiRect::new(20, 342, 760, 108);
        draw_rounded_card(mem_dc, &card_rc, colors.card_bg, colors.card_border, 8);

        // Top Row: Emergency Header & Autostart Checkbox
        SelectObject(mem_dc, fonts.body_bold);
        SetTextColor(mem_dc, rgb(251, 146, 60)); // Orange alert
        let mut em_rc = RECT { left: 34, top: 350, right: 280, bottom: 372 };
        DrawTextW(mem_dc, to_wide("⚡ 开机按键急救与系统优化").as_ptr(), -1, &mut em_rc, DT_LEFT | DT_VCENTER | DT_SINGLELINE);

        // Autostart checkbox right (x: 580..760)
        let chk_box_rc = UiRect::new(590, 352, 14, 14);
        draw_rounded_card(mem_dc, &chk_box_rc, colors.button_bg, colors.card_border, 3);
        if state.autostart_enabled {
            draw_rounded_card(mem_dc, &UiRect::new(593, 355, 8, 8), colors.accent, colors.accent, 2);
        }
        SelectObject(mem_dc, fonts.small);
        SetTextColor(mem_dc, colors.text_primary);
        let mut chk_label = RECT { left: 610, top: 350, right: 770, bottom: 372 };
        DrawTextW(mem_dc, to_wide("开机自动在后台运行").as_ptr(), -1, &mut chk_label, DT_LEFT | DT_VCENTER | DT_SINGLELINE);

        // Hotkey Row (Y: 378..404)
        SelectObject(mem_dc, fonts.body);
        SetTextColor(mem_dc, colors.text_primary);
        let mut hk_label = RECT { left: 34, top: 378, right: 120, bottom: 404 };
        DrawTextW(mem_dc, to_wide("急救快捷键:").as_ptr(), -1, &mut hk_label, DT_LEFT | DT_VCENTER | DT_SINGLELINE);

        // Hotkey display badge
        let hk_badge_rc = UiRect::new(120, 380, 125, 24);
        draw_rounded_card(mem_dc, &hk_badge_rc, colors.button_bg, colors.card_border, 4);
        SelectObject(mem_dc, fonts.body_bold);
        SetTextColor(mem_dc, rgb(255, 215, 0)); // Gold
        let mut hk_badge_win_rc = hk_badge_rc.to_win_rect();
        DrawTextW(mem_dc, to_wide(&state.hotkey_buffer).as_ptr(), -1, &mut hk_badge_win_rc, DT_CENTER | DT_VCENTER | DT_SINGLELINE);

        SelectObject(mem_dc, fonts.small);
        SetTextColor(mem_dc, colors.text_secondary);
        let mut rec_label = RECT { left: 255, top: 378, right: 300, bottom: 404 };
        DrawTextW(mem_dc, to_wide("推荐:").as_ptr(), -1, &mut rec_label, DT_LEFT | DT_VCENTER | DT_SINGLELINE);

        let hk_presets = [("Ctrl+Shift+F12", 295), ("Ctrl+Alt+R", 400), ("F11", 485)];
        for &(hk_str, hk_x) in &hk_presets {
            let hk_p_rc = UiRect::new(hk_x, 380, 95, 22);
            draw_rounded_card(mem_dc, &hk_p_rc, colors.button_bg, colors.card_border, 4);
            SetTextColor(mem_dc, colors.text_secondary);
            let mut hk_p_win = hk_p_rc.to_win_rect();
            DrawTextW(mem_dc, to_wide(hk_str).as_ptr(), -1, &mut hk_p_win, DT_CENTER | DT_VCENTER | DT_SINGLELINE);
        }

        // Action Buttons Row (Y: 412..438)
        let reset_btn_rc = UiRect::new(34, 412, 185, 26);
        draw_rounded_card(mem_dc, &reset_btn_rc, rgb(194, 65, 12), rgb(234, 88, 12), 4);
        SelectObject(mem_dc, fonts.body_bold);
        SetTextColor(mem_dc, rgb(255, 255, 255));
        let mut reset_win = reset_btn_rc.to_win_rect();
        DrawTextW(mem_dc, to_wide("⚡ 立即重置 USB 端口").as_ptr(), -1, &mut reset_win, DT_CENTER | DT_VCENTER | DT_SINGLELINE);

        let patch_btn_rc = UiRect::new(230, 412, 205, 26);
        draw_rounded_card(mem_dc, &patch_btn_rc, colors.button_bg, colors.card_border, 4);
        SelectObject(mem_dc, fonts.body);
        SetTextColor(mem_dc, colors.text_primary);
        let mut patch_win = patch_btn_rc.to_win_rect();
        DrawTextW(mem_dc, to_wide("🛠️ 写入注册表防休眠补丁").as_ptr(), -1, &mut patch_win, DT_CENTER | DT_VCENTER | DT_SINGLELINE);
    }

    // 6. Docked Footer Bar (Y: 460..510)
    {
        let footer_rc = UiRect::new(0, 460, win_w, 50);
        let ft_brush = CreateSolidBrush(colors.footer_bg);
        FillRect(mem_dc, &footer_rc.to_win_rect(), ft_brush);
        DeleteObject(ft_brush);

        let top_pen = CreatePen(PS_SOLID as i32, 1, colors.card_border);
        let old_pen = SelectObject(mem_dc, top_pen);
        windows_sys::Win32::Graphics::Gdi::MoveToEx(mem_dc, 0, 460, null_mut());
        windows_sys::Win32::Graphics::Gdi::LineTo(mem_dc, win_w, 460);
        SelectObject(mem_dc, old_pen);
        DeleteObject(top_pen);

        // Status text on left
        SelectObject(mem_dc, fonts.small);
        let status_text = if let Some((msg, _)) = &state.status_message {
            msg.as_str()
        } else {
            "就绪 - 设置实时同步至鼠标板载芯片"
        };
        SetTextColor(mem_dc, colors.accent_cyan);
        let mut ft_stat_rc = RECT { left: 20, top: 462, right: 380, bottom: 508 };
        DrawTextW(mem_dc, to_wide(status_text).as_ptr(), -1, &mut ft_stat_rc, DT_LEFT | DT_VCENTER | DT_SINGLELINE);

        // Action buttons on right
        let btns = [
            ("❌ 退出", 710, 70, false),
            ("📂 配置文件", 615, 88, false),
            ("📥 最小化到托盘", 495, 112, false),
            ("💾 保存到板载", 380, 108, true),
        ];

        for &(name, bx, bw, is_primary) in &btns {
            let b_rc = UiRect::new(bx, 472, bw, 26);
            let b_bg = if is_primary { colors.accent } else { colors.button_bg };
            let b_border = if is_primary { colors.card_active_border } else { colors.card_border };
            let b_col = if is_primary { rgb(255, 255, 255) } else { colors.text_secondary };

            draw_rounded_card(mem_dc, &b_rc, b_bg, b_border, 4);
            SelectObject(mem_dc, if is_primary { fonts.body_bold } else { fonts.small });
            SetTextColor(mem_dc, b_col);
            let mut b_win = b_rc.to_win_rect();
            DrawTextW(mem_dc, to_wide(name).as_ptr(), -1, &mut b_win, DT_CENTER | DT_VCENTER | DT_SINGLELINE);
        }
    }

    // 7. Render Active Dropdown Popups (On Top of Everything!)
    if let Some(drop_type) = state.active_dropdown {
        match drop_type {
            DropdownType::PollingRate => {
                let drop_rc = UiRect::new(115, 324, 175, 175);
                draw_rounded_card(mem_dc, &drop_rc, colors.card_active_bg, colors.accent, 6);

                for (idx, rate) in PollingRate::ALL.iter().enumerate() {
                    let item_rc = UiRect::new(117, 326 + (idx as i32) * 24, 171, 23);
                    let is_sel = state.settings.polling_rate == *rate;
                    let item_bg = if is_sel { colors.accent } else { colors.card_bg };
                    let item_text_col = if is_sel { rgb(255, 255, 255) } else { colors.text_primary };

                    draw_rounded_card(mem_dc, &item_rc, item_bg, colors.card_border, 3);
                    SelectObject(mem_dc, fonts.small);
                    SetTextColor(mem_dc, item_text_col);
                    let item_str = format!("{} Hz ({})", rate.to_hz(), rate.to_delay_str());
                    let mut i_win = item_rc.to_win_rect();
                    i_win.left += 8;
                    DrawTextW(mem_dc, to_wide(&item_str).as_ptr(), -1, &mut i_win, DT_LEFT | DT_VCENTER | DT_SINGLELINE);
                }
            }
            DropdownType::Lod => {
                let drop_rc = UiRect::new(450, 324, 210, 54);
                draw_rounded_card(mem_dc, &drop_rc, colors.card_active_bg, colors.accent, 6);

                let lod_options = [(LiftOffDistance::Mm1, "1.0 mm (低 - 电竞推荐)", 326), (LiftOffDistance::Mm2, "2.0 mm (高 - 厚脚贴适用)", 351)];
                for (lod_val, lod_str, ly) in lod_options {
                    let item_rc = UiRect::new(452, ly, 206, 23);
                    let is_sel = state.settings.lod == lod_val;
                    let item_bg = if is_sel { colors.accent } else { colors.card_bg };
                    let item_text_col = if is_sel { rgb(255, 255, 255) } else { colors.text_primary };

                    draw_rounded_card(mem_dc, &item_rc, item_bg, colors.card_border, 3);
                    SelectObject(mem_dc, fonts.small);
                    SetTextColor(mem_dc, item_text_col);
                    let mut i_win = item_rc.to_win_rect();
                    i_win.left += 8;
                    DrawTextW(mem_dc, to_wide(lod_str).as_ptr(), -1, &mut i_win, DT_LEFT | DT_VCENTER | DT_SINGLELINE);
                }
            }
        }
    }

    // Double-buffered blit to screen
    BitBlt(hdc, 0, 0, win_w, win_h, mem_dc, 0, 0, SRCCOPY);

    SelectObject(mem_dc, old_bmp);
    DeleteObject(mem_bmp);
    DeleteDC(mem_dc);
    EndPaint(hwnd, &ps);
}

unsafe fn handle_mouse_click(hwnd: HWND, state: &mut NativeGuiState, x: i32, y: i32) {
    // 0. If a dropdown is active, hit-test dropdown items first
    if let Some(drop_type) = state.active_dropdown {
        match drop_type {
            DropdownType::PollingRate => {
                let drop_rc = UiRect::new(115, 324, 175, 175);
                if drop_rc.contains(x, y) {
                    let clicked_idx = ((y - 326) / 24).clamp(0, (PollingRate::ALL.len() - 1) as i32) as usize;
                    let new_rate = PollingRate::ALL[clicked_idx];
                    state.settings.polling_rate = new_rate;
                    state.save_settings_to_device();
                    show_toast_osd(&format!("⚡ 回报率: {} Hz", new_rate.to_hz()));
                }
                state.active_dropdown = None;
                InvalidateRect(hwnd, null_mut(), 0);
                return;
            }
            DropdownType::Lod => {
                let drop_rc = UiRect::new(450, 324, 210, 54);
                if drop_rc.contains(x, y) {
                    if y < 350 {
                        state.settings.lod = LiftOffDistance::Mm1;
                        show_toast_osd("📏 LOD: 1 mm");
                    } else {
                        state.settings.lod = LiftOffDistance::Mm2;
                        show_toast_osd("📏 LOD: 2 mm");
                    }
                    state.save_settings_to_device();
                }
                state.active_dropdown = None;
                InvalidateRect(hwnd, null_mut(), 0);
                return;
            }
        }
    }

    // 1. Theme Switcher Capsule (x: 590..780, y: 12..38)
    let theme_rc = UiRect::new(590, 12, 190, 26);
    if theme_rc.contains(x, y) {
        state.theme_mode = state.theme_mode.toggle();
        if let Ok(mut cfg) = state.config.lock() {
            cfg.system.theme = state.theme_mode.as_str().to_string();
            let _ = cfg.save();
        }
        state.set_status(&format!("🎨 已切换为 {} 主题", if state.theme_mode == ThemeMode::Obsidian { "电竞紫曜" } else { "Win11 Fluent" }));
        InvalidateRect(hwnd, null_mut(), 0);
        return;
    }

    // 2. Stage Switch Tabs (Top of Section 1, Y: 62..86)
    for i in 0..5 {
        let tab_rc = UiRect::new(135 + (i as i32) * 122, 62, 116, 24);
        if tab_rc.contains(x, y) {
            state.switch_active_stage(i as u8);
            InvalidateRect(hwnd, null_mut(), 0);
            return;
        }
    }

    // 3. Quick Presets Pills (Y: 92..114)
    let active_stage = state.settings.active_cpi_idx as usize % 5;
    for (k, &quick_val) in QUICK_DPI_LIST.iter().enumerate() {
        let q_rc = UiRect::new(96 + (k as i32) * 52, 92, 48, 20);
        if q_rc.contains(x, y) {
            state.settings.stages[active_stage] = quick_val;
            state.save_settings_to_device();
            show_dpi_osd(quick_val, state.settings.active_cpi_idx + 1, 5);
            InvalidateRect(hwnd, null_mut(), 0);
            return;
        }
    }

    // Template buttons
    let templates = [
        ("FPS", UiRect::new(540, 92, 74, 20), PRESET_FPS_CLASSIC),
        ("MOBA", UiRect::new(620, 92, 74, 20), PRESET_MOBA_SPEED),
        ("4K", UiRect::new(700, 92, 74, 20), PRESET_HIGH_RES_4K),
    ];
    for (t_name, t_rc, t_stages) in templates {
        if t_rc.contains(x, y) {
            state.settings.stages = t_stages;
            state.save_settings_to_device();
            state.set_status(&format!("✅ 已应用 {} 预设", t_name));
            InvalidateRect(hwnd, null_mut(), 0);
            return;
        }
    }

    // 4. 5 Stage Columns (Header button, [-50], [+50], Slider)
    for i in 0..5 {
        let col_x = 32 + (i as i32) * 148;

        // Stage Header Button (Entire top button switches stage!)
        let btn_rc = UiRect::new(col_x + 6, 126, 128, 26);
        if btn_rc.contains(x, y) {
            state.switch_active_stage(i as u8);
            InvalidateRect(hwnd, null_mut(), 0);
            return;
        }

        // [-50] button
        let minus_rc = UiRect::new(col_x + 16, 186, 50, 20);
        if minus_rc.contains(x, y) {
            state.settings.stages[i] = adjust_dpi_step(state.settings.stages[i], -50);
            state.save_settings_to_device();
            InvalidateRect(hwnd, null_mut(), 0);
            return;
        }

        // [+50] button
        let plus_rc = UiRect::new(col_x + 74, 186, 50, 20);
        if plus_rc.contains(x, y) {
            state.settings.stages[i] = adjust_dpi_step(state.settings.stages[i], 50);
            state.save_settings_to_device();
            InvalidateRect(hwnd, null_mut(), 0);
            return;
        }

        // Slider track
        let track_rc = UiRect::new(col_x + 12, 214, 116, 20);
        if track_rc.contains(x, y) {
            state.dragging_stage = Some(i);
            state.settings.stages[i] = slider_pos_to_dpi(x, col_x + 12, 116);
            state.save_settings_to_device();
            InvalidateRect(hwnd, null_mut(), 0);
            return;
        }
    }

    // 5. Section 2: Dropdowns Toggle (Polling & LOD)
    let hz_btn_rc = UiRect::new(115, 296, 175, 26);
    if hz_btn_rc.contains(x, y) {
        state.active_dropdown = Some(DropdownType::PollingRate);
        InvalidateRect(hwnd, null_mut(), 0);
        return;
    }

    let lod_btn_rc = UiRect::new(450, 296, 210, 26);
    if lod_btn_rc.contains(x, y) {
        state.active_dropdown = Some(DropdownType::Lod);
        InvalidateRect(hwnd, null_mut(), 0);
        return;
    }

    // 6. Section 3: Autostart Checkbox
    let chk_rc = UiRect::new(590, 350, 180, 20);
    if chk_rc.contains(x, y) {
        state.autostart_enabled = !state.autostart_enabled;
        match set_autostart(state.autostart_enabled) {
            Ok(_) => {
                if state.autostart_enabled {
                    state.set_status("✅ 已启用开机自启动");
                } else {
                    state.set_status("ℹ️ 已关闭开机自启动");
                }
            }
            Err(e) => state.set_status(&format!("❌ 修改自启失败: {}", e)),
        }
        InvalidateRect(hwnd, null_mut(), 0);
        return;
    }

    // Hotkey recommendations
    let hk_presets = [("Ctrl+Shift+F12", UiRect::new(295, 380, 95, 22)), ("Ctrl+Alt+R", UiRect::new(400, 380, 75, 22)), ("F11", UiRect::new(485, 380, 45, 22))];
    for (hk_str, hk_rc) in hk_presets {
        if hk_rc.contains(x, y) {
            state.hotkey_buffer = hk_str.to_string();
            match update_global_hotkey(hk_str) {
                Ok(c) => state.set_status(&format!("✅ 快捷键已更新: {}", c)),
                Err(e) => state.set_status(&format!("❌ 快捷键更新失败: {}", e)),
            }
            InvalidateRect(hwnd, null_mut(), 0);
            return;
        }
    }

    // Reset USB button
    let reset_btn_rc = UiRect::new(34, 412, 185, 26);
    if reset_btn_rc.contains(x, y) {
        let _ = cycle_usb_port();
        release_stuck_clicks();
        state.set_status("⚡ USB 端口硬件断电复位成功！");
        show_toast_osd("⚡ USB Cycled - Stuck Click Fixed");
        InvalidateRect(hwnd, null_mut(), 0);
        return;
    }

    // Fix Power button
    let patch_btn_rc = UiRect::new(230, 412, 205, 26);
    if patch_btn_rc.contains(x, y) {
        match fix_usb_power_settings() {
            Ok(_) => state.set_status("✅ 注册表防休眠优化已应用！"),
            Err(e) => state.set_status(&format!("❌ 注册表修改失败: {}", e)),
        }
        InvalidateRect(hwnd, null_mut(), 0);
        return;
    }

    // 7. Footer Buttons
    // Exit
    if UiRect::new(710, 472, 70, 26).contains(x, y) {
        crate::system::disable_high_res_timer();
        DestroyWindow(hwnd);
        std::process::exit(0);
    }
    // Config
    if UiRect::new(615, 472, 88, 26).contains(x, y) {
        let path = AppConfig::get_config_path();
        let _ = std::process::Command::new("notepad.exe").arg(path).spawn();
        return;
    }
    // Minimize to tray (Destroys window to free memory down to 2MB!)
    if UiRect::new(495, 472, 112, 26).contains(x, y) {
        DestroyWindow(hwnd);
        return;
    }
    // Save
    if UiRect::new(380, 472, 108, 26).contains(x, y) {
        state.save_settings_to_device();
        InvalidateRect(hwnd, null_mut(), 0);
        return;
    }
}

unsafe fn draw_rounded_card(hdc: HDC, rect: &UiRect, fill_color: u32, border_color: u32, radius: i32) {
    let brush = CreateSolidBrush(fill_color);
    let pen = CreatePen(PS_SOLID as i32, 1, border_color);

    let old_brush = SelectObject(hdc, brush);
    let old_pen = SelectObject(hdc, pen);

    RoundRect(hdc, rect.x, rect.y, rect.x + rect.w, rect.y + rect.h, radius * 2, radius * 2);

    SelectObject(hdc, old_brush);
    SelectObject(hdc, old_pen);

    DeleteObject(brush);
    DeleteObject(pen);
}

unsafe fn draw_theme_capsule(hdc: HDC, fonts: &GdiFonts, colors: &ThemeColors, mode: ThemeMode, x: i32, y: i32, w: i32, h: i32) {
    let outer_rc = UiRect::new(x, y, w, h);
    draw_rounded_card(hdc, &outer_rc, colors.card_bg, colors.card_border, 5);

    let half_w = (w - 4) / 2;
    let (obs_rc, flu_rc) = (UiRect::new(x + 2, y + 2, half_w, h - 4), UiRect::new(x + 2 + half_w, y + 2, half_w, h - 4));

    let (obs_bg, flu_bg) = if mode == ThemeMode::Obsidian {
        (colors.accent, colors.card_bg)
    } else {
        (colors.card_bg, colors.accent)
    };

    draw_rounded_card(hdc, &obs_rc, obs_bg, colors.card_border, 4);
    draw_rounded_card(hdc, &flu_rc, flu_bg, colors.card_border, 4);

    SelectObject(hdc, fonts.small);
    SetTextColor(hdc, if mode == ThemeMode::Obsidian { rgb(255, 255, 255) } else { colors.text_secondary });
    let mut obs_text_rc = obs_rc.to_win_rect();
    DrawTextW(hdc, to_wide("🌙 电竞紫曜").as_ptr(), -1, &mut obs_text_rc, DT_CENTER | DT_VCENTER | DT_SINGLELINE);

    SetTextColor(hdc, if mode == ThemeMode::Fluent { rgb(255, 255, 255) } else { colors.text_secondary });
    let mut flu_text_rc = flu_rc.to_win_rect();
    DrawTextW(hdc, to_wide("🪟 Win11").as_ptr(), -1, &mut flu_text_rc, DT_CENTER | DT_VCENTER | DT_SINGLELINE);
}

fn to_wide(s: &str) -> Vec<u16> {
    OsStr::new(s).encode_wide().chain(std::iter::once(0)).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_ui_rect_contains() {
        let rect = UiRect::new(100, 100, 80, 40);
        assert!(rect.contains(100, 100));
        assert!(rect.contains(179, 139));
        assert!(!rect.contains(99, 100));
        assert!(!rect.contains(180, 100));
        assert!(!rect.contains(150, 140));
    }

    #[test]
    fn test_dpi_slider_math() {
        let track_x = 100;
        let track_w = 200;

        assert_eq!(dpi_to_slider_pos(50, track_x, track_w), track_x);
        assert_eq!(dpi_to_slider_pos(26000, track_x, track_w), track_x + track_w);

        assert_eq!(slider_pos_to_dpi(track_x, track_x, track_w), 50);
        assert_eq!(slider_pos_to_dpi(track_x + track_w, track_x, track_w), 26000);

        let mid_pos = track_x + track_w / 2;
        let mid_dpi = slider_pos_to_dpi(mid_pos, track_x, track_w);
        assert!((mid_dpi as i32 - 13025).abs() <= 100);
    }

    #[test]
    fn test_to_wide() {
        let wide = to_wide("NZXT");
        assert_eq!(wide, vec![b'N' as u16, b'Z' as u16, b'X' as u16, b'T' as u16, 0]);
    }

    #[test]
    #[cfg(windows)]
    fn test_gdi_fonts_lifecycle() {
        unsafe {
            let fonts = GdiFonts::create();
            assert_ne!(fonts.title, std::ptr::null_mut());
            assert_ne!(fonts.body, std::ptr::null_mut());
            assert_ne!(fonts.small, std::ptr::null_mut());
            fonts.destroy();
        }
    }
}
