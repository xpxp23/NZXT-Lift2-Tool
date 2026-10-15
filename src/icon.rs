//! Aesthetic, high-resolution gaming mouse icon generator for NZXT Lift 2 Ergo


/// Generates an RGBA8 bitmap of an elegant, sleek esports gaming mouse
/// with ergonomic contours, neon purple accents, and glowing cyan scroll wheel.
pub fn generate_mouse_icon_rgba(size: u32) -> Vec<u8> {
    let mut rgba = vec![0u8; (size * size * 4) as usize];
    let s = size as f32;

    for y in 0..size {
        for x in 0..size {
            // Normalized coordinates centered at (0, 0), range [-1.0, 1.0]
            let nx = (x as f32 + 0.5 - s * 0.5) / (s * 0.44);
            let ny = (y as f32 + 0.5 - s * 0.5) / (s * 0.46);

            // Ergonomic asymmetrical mouse body contour equation
            // Lift 2 Ergo has a wider thumb groove on left (negative nx)
            let ergo_shift = if nx < 0.0 { -0.08 * (1.0 - ny.abs()) } else { 0.04 * (1.0 - ny.abs()) };
            let adj_nx = nx - ergo_shift;

            // Base egg/oval shape with wider palm base and tapered clicks
            let taper = 1.0 + ny * 0.22;
            let dist_sq = (adj_nx / taper).powi(2) + ny.powi(2);

            let idx = ((y * size + x) * 4) as usize;

            if dist_sq <= 1.0 {
                let dist = dist_sq.sqrt();
                // Anti-aliased outer edge alpha
                let edge_alpha = ((1.0 - dist) * 12.0).clamp(0.0, 1.0);

                // Default sleek obsidian dark purple body gradient
                let mut r = 26.0 + (1.0 - ny) * 12.0;
                let mut g = 20.0 + (1.0 - ny) * 8.0;
                let mut b = 38.0 + (1.0 - ny) * 20.0;
                let a = edge_alpha * 255.0;

                // Subtle neon purple outer rim glow
                if dist > 0.86 {
                    let glow = (dist - 0.86) / 0.14;
                    r = r * (1.0 - glow) + 168.0 * glow;
                    g = g * (1.0 - glow) + 85.0 * glow;
                    b = b * (1.0 - glow) + 247.0 * glow;
                }

                // Front Click Split seam line (ny < -0.15, |adj_nx| < 0.035)
                if ny < -0.18 && adj_nx.abs() < 0.04 {
                    r = 12.0;
                    g = 10.0;
                    b = 18.0;
                }

                // Scroll Wheel slot & illuminated wheel (ny in [-0.55, -0.22], |adj_nx| < 0.11)
                if ny >= -0.55 && ny <= -0.22 && adj_nx.abs() <= 0.11 {
                    if adj_nx.abs() <= 0.075 && ny >= -0.52 && ny <= -0.25 {
                        // Glowing Cyan Scroll Wheel (NZXT Teal #00F5D4)
                        let wheel_grad = (ny + 0.385).abs() / 0.135;
                        let intensity = 1.0 - wheel_grad * 0.4;
                        r = (0.0 * intensity + 40.0).clamp(0.0, 255.0);
                        g = (245.0 * intensity).clamp(0.0, 255.0);
                        b = (212.0 * intensity + 30.0).clamp(0.0, 255.0);
                    } else {
                        // Dark slot border
                        r = 10.0;
                        g = 8.0;
                        b = 16.0;
                    }
                }

                // DPI Button LED accent (ny in [-0.14, -0.06], |adj_nx| < 0.06)
                if ny >= -0.14 && ny <= -0.06 && adj_nx.abs() <= 0.06 {
                    // Magenta/Purple glowing jewel
                    r = 216.0;
                    g = 120.0;
                    b = 255.0;
                }

                // Subtle illuminated palm badge NZXT arc (ny in [0.35, 0.55])
                let palm_r = (adj_nx.powi(2) + (ny - 0.45).powi(2)).sqrt();
                if palm_r >= 0.14 && palm_r <= 0.17 {
                    let arc_glow = 1.0 - (palm_r - 0.155).abs() / 0.015;
                    r = r + 147.0 * arc_glow * 0.7;
                    g = g + 51.0 * arc_glow * 0.7;
                    b = b + 234.0 * arc_glow * 0.7;
                }

                rgba[idx] = r.clamp(0.0, 255.0) as u8;
                rgba[idx + 1] = g.clamp(0.0, 255.0) as u8;
                rgba[idx + 2] = b.clamp(0.0, 255.0) as u8;
                rgba[idx + 3] = a.clamp(0.0, 255.0) as u8;
            }
        }
    }

    rgba
}

/// Generates a Win32 HICON for the tray or window with proper 1-bit transparency mask
#[cfg(windows)]
pub unsafe fn create_custom_hicon(size: u32) -> windows_sys::Win32::UI::WindowsAndMessaging::HICON {
    use windows_sys::Win32::Foundation::HWND;
    use windows_sys::Win32::Graphics::Gdi::{
        CreateBitmap, CreateCompatibleBitmap, CreateCompatibleDC, DeleteDC, DeleteObject,
        GetDC, ReleaseDC, SelectObject, SetPixel,
    };
    use windows_sys::Win32::UI::WindowsAndMessaging::{CreateIconIndirect, ICONINFO};

    let rgba = generate_mouse_icon_rgba(size);

    let dc = GetDC(0 as HWND);
    let mem_dc = CreateCompatibleDC(dc);
    let color_bmp = CreateCompatibleBitmap(dc, size as i32, size as i32);

    let old_bmp = SelectObject(mem_dc, color_bmp);

    // Scan line width for 1-bit monochrome bitmap must be WORD (16-bit) aligned
    let bytes_per_line = ((size + 15) / 16 * 2) as usize;
    let mut mask_bits = vec![0u8; bytes_per_line * (size as usize)];

    for y in 0..size {
        for x in 0..size {
            let idx = ((y * size + x) * 4) as usize;
            let r = rgba[idx] as u32;
            let g = rgba[idx + 1] as u32;
            let b = rgba[idx + 2] as u32;
            let a = rgba[idx + 3];

            if a < 30 {
                // Transparent pixel: bit = 1 in mask
                let line_offset = (y as usize) * bytes_per_line;
                let byte_idx = line_offset + (x as usize) / 8;
                let bit_shift = 7 - (x % 8);
                mask_bits[byte_idx] |= 1 << bit_shift;
                SetPixel(mem_dc, x as i32, y as i32, 0);
            } else {
                // Opaque pixel: bit = 0 in mask, color in RGB
                let color = (r) | (g << 8) | (b << 16);
                SetPixel(mem_dc, x as i32, y as i32, color);
            }
        }
    }

    SelectObject(mem_dc, old_bmp);
    DeleteDC(mem_dc);
    ReleaseDC(0 as HWND, dc);

    let mask_bmp = CreateBitmap(
        size as i32,
        size as i32,
        1,
        1,
        mask_bits.as_ptr() as *const _,
    );

    let mut icon_info = ICONINFO {
        fIcon: 1,
        xHotspot: 0,
        yHotspot: 0,
        hbmMask: mask_bmp,
        hbmColor: color_bmp,
    };

    let hicon = CreateIconIndirect(&mut icon_info);
    DeleteObject(color_bmp);
    DeleteObject(mask_bmp);
    hicon
}

#[cfg(windows)]
pub unsafe fn create_custom_tray_hicon() -> windows_sys::Win32::UI::WindowsAndMessaging::HICON {
    create_custom_hicon(32)
}

#[cfg(windows)]
pub unsafe fn create_custom_window_hicon() -> windows_sys::Win32::UI::WindowsAndMessaging::HICON {
    create_custom_hicon(64)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_generate_mouse_icon_rgba() {
        let size = 32;
        let rgba = generate_mouse_icon_rgba(size);
        assert_eq!(rgba.len(), (size * size * 4) as usize);

        // Verify there are non-transparent pixels (mouse body and glowing accents)
        let visible_pixels = rgba.chunks(4).filter(|p| p[3] > 0).count();
        assert!(visible_pixels > 200, "Icon should have visible pixels");

        // Verify 64x64 generation
        let rgba64 = generate_mouse_icon_rgba(64);
        assert_eq!(rgba64.len(), 64 * 64 * 4);
    }

    #[test]
    #[cfg(windows)]
    fn test_create_custom_hicon_lifecycle() {
        use windows_sys::Win32::UI::WindowsAndMessaging::DestroyIcon;
        unsafe {
            let hicon_tray = create_custom_tray_hicon();
            assert_ne!(hicon_tray, std::ptr::null_mut());
            DestroyIcon(hicon_tray);

            let hicon_win = create_custom_window_hicon();
            assert_ne!(hicon_win, std::ptr::null_mut());
            DestroyIcon(hicon_win);
        }
    }
}
