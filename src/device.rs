use std::ffi::OsStr;
use std::os::windows::ffi::OsStrExt;
use std::ptr::null_mut;
use std::thread;
use std::time::Duration;

use windows_sys::Win32::Foundation::{
    CloseHandle, GetLastError, ERROR_IO_PENDING, HANDLE, INVALID_HANDLE_VALUE, WAIT_OBJECT_0,
};
use windows_sys::Win32::Storage::FileSystem::{
    ReadFile, WriteFile, FILE_FLAG_OVERLAPPED, FILE_GENERIC_READ, FILE_GENERIC_WRITE,
    FILE_SHARE_READ, FILE_SHARE_WRITE, OPEN_EXISTING,
};
use windows_sys::Win32::System::IO::{CancelIo, DeviceIoControl, GetOverlappedResult, OVERLAPPED};
use windows_sys::Win32::System::Registry::{
    RegCloseKey, RegEnumKeyExW, RegOpenKeyExW, RegQueryValueExW, RegSetValueExW, HKEY,
    HKEY_LOCAL_MACHINE, KEY_READ, KEY_WRITE, REG_DWORD, REG_OPTION_NON_VOLATILE,
};
use windows_sys::Win32::System::Threading::{ResetEvent, WaitForSingleObject};

#[link(name = "cfgmgr32")]
extern "system" {
    fn CM_Locate_DevNodeW(pdnDevInst: *mut u32, pDeviceID: *const u16, ulFlags: u32) -> u32;
    fn CM_Get_Parent(pdnDevInst: *mut u32, dnDevInst: u32, ulFlags: u32) -> u32;
    fn CM_Get_Device_IDW(dnDevInst: u32, Buffer: *mut u16, BufferLen: u32, ulFlags: u32) -> u32;
}

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

    fn RegCreateKeyExW(
        hKey: HKEY,
        lpSubKey: *const u16,
        Reserved: u32,
        lpClass: *mut u16,
        dwOptions: u32,
        samDesired: u32,
        lpSecurityAttributes: *mut std::ffi::c_void,
        phkResult: *mut HKEY,
        lpdwDisposition: *mut u32,
    ) -> i32;
}

use crate::protocol::*;

const IOCTL_USB_CYCLE_PORT: u32 = 0x00220444;

/// Supported NZXT Lift 2 series hardware IDs (PID 2124 = Lift 2 Ergo, PID 2123 = Lift 2 Symm)
const SUPPORTED_VID_PIDS: [&str; 2] = ["VID_1E71&PID_2124", "VID_1E71&PID_2123"];

#[repr(C)]
struct UsbCyclePortParams {
    connection_index: u32,
    status_returned: u32,
}

pub struct Lift2Device {
    handle: HANDLE,
    read_event: HANDLE,
    write_event: HANDLE,
}

unsafe impl Send for Lift2Device {}

impl Lift2Device {
    pub fn open() -> Result<Self, String> {
        let mi01_path = find_lift2_mi01_path()?
            .ok_or_else(|| "NZXT Lift 2 (MI_01) not found or disconnected. Is it plugged in?".to_string())?;

        let wide: Vec<u16> = OsStr::new(&mi01_path)
            .encode_wide()
            .chain(std::iter::once(0))
            .collect();

        unsafe {
            let handle = CreateFileW(
                wide.as_ptr(),
                FILE_GENERIC_READ | FILE_GENERIC_WRITE,
                FILE_SHARE_READ | FILE_SHARE_WRITE,
                null_mut(),
                OPEN_EXISTING,
                FILE_FLAG_OVERLAPPED,
                null_mut(),
            );

            if handle == INVALID_HANDLE_VALUE {
                return Err(format!(
                    "Failed to open Lift 2 MI_01 handle: Win32 error {}",
                    GetLastError()
                ));
            }

            let read_event = CreateEventW(null_mut(), 1, 0, null_mut());
            let write_event = CreateEventW(null_mut(), 1, 0, null_mut());

            if read_event == 0 as HANDLE || write_event == 0 as HANDLE {
                if read_event != 0 as HANDLE {
                    CloseHandle(read_event);
                }
                if write_event != 0 as HANDLE {
                    CloseHandle(write_event);
                }
                CloseHandle(handle);
                return Err("Failed to create Win32 Overlapped I/O events".to_string());
            }

            Ok(Self {
                handle,
                read_event,
                write_event,
            })
        }
    }

    fn read_packet(&self, timeout_ms: u32) -> Result<[u8; REPORT_SIZE], String> {
        let mut buf = [0u8; REPORT_SIZE];
        let mut overlapped: OVERLAPPED = unsafe { std::mem::zeroed() };
        overlapped.hEvent = self.read_event;

        unsafe {
            ResetEvent(self.read_event);
            let mut bytes_read: u32 = 0;
            let ok = ReadFile(
                self.handle,
                buf.as_mut_ptr(),
                REPORT_SIZE as u32,
                &mut bytes_read,
                &mut overlapped,
            );

            if ok == 0 {
                let err = GetLastError();
                if err != ERROR_IO_PENDING {
                    return Err(format!("ReadFile failed: error {}", err));
                }
            }

            let wait_res = WaitForSingleObject(self.read_event, timeout_ms);
            let mut transferred = 0u32;
            if wait_res == WAIT_OBJECT_0 {
                GetOverlappedResult(self.handle, &overlapped, &mut transferred, 0);
                Ok(buf)
            } else {
                // Safely cancel pending IRP and wait for kernel driver to release stack buffers
                CancelIo(self.handle);
                GetOverlappedResult(self.handle, &overlapped, &mut transferred, 1);
                Err("Read timeout".to_string())
            }
        }
    }

    fn write_packet(&self, packet: &[u8; REPORT_SIZE], timeout_ms: u32) -> Result<(), String> {
        let mut overlapped: OVERLAPPED = unsafe { std::mem::zeroed() };
        overlapped.hEvent = self.write_event;

        unsafe {
            ResetEvent(self.write_event);
            let mut bytes_written: u32 = 0;
            let ok = WriteFile(
                self.handle,
                packet.as_ptr(),
                REPORT_SIZE as u32,
                &mut bytes_written,
                &mut overlapped,
            );

            if ok == 0 {
                let err = GetLastError();
                if err != ERROR_IO_PENDING {
                    return Err(format!("WriteFile failed: error {}", err));
                }
            }

            let wait_res = WaitForSingleObject(self.write_event, timeout_ms);
            let mut transferred = 0u32;
            if wait_res == WAIT_OBJECT_0 {
                GetOverlappedResult(self.handle, &overlapped, &mut transferred, 0);
                Ok(())
            } else {
                CancelIo(self.handle);
                GetOverlappedResult(self.handle, &overlapped, &mut transferred, 1);
                Err("Write timeout".to_string())
            }
        }
    }

    pub fn send_command(
        &mut self,
        opcode: u8,
        payload: &[u8],
        timeout_ms: u32,
    ) -> Result<[u8; REPORT_SIZE], String> {
        let packet = build_packet(opcode, payload);
        self.write_packet(&packet, timeout_ms)?;

        for _ in 0..6 {
            let pkt = self.read_packet(timeout_ms)?;
            if pkt[0] != REPORT_ID || pkt[1] == 0xE5 {
                continue;
            }
            // Accept packet if opcode matches, or if CMD_GET_ACTIVE_CPI returns stage directly
            if pkt[3] == opcode || (opcode == CMD_GET_ACTIVE_CPI && pkt[3] <= 4) {
                return Ok(pkt);
            }
        }

        Err("Received only ACK or mismatched frames, no valid data frame".to_string())
    }

    pub fn get_firmware_version(&mut self) -> Result<String, String> {
        let pkt = self.send_command(CMD_GET_VERSION, &[], 400)?;
        let major = pkt[4];
        let patch = pkt[5];
        Ok(format!("{}.1.{}", major, patch))
    }

    pub fn get_uid(&mut self) -> Result<String, String> {
        let pkt = self.send_command(CMD_GET_UID, &[], 400)?;
        let hex_str: Vec<String> = pkt[4..20].iter().map(|b| format!("{:02X}", b)).collect();
        Ok(hex_str.join(""))
    }

    pub fn get_active_cpi_idx(&mut self) -> Result<u8, String> {
        if let Ok(pkt) = self.send_command(CMD_GET_ACTIVE_CPI, &[], 300) {
            if let Some(idx) = parse_active_cpi_idx(&pkt) {
                return Ok(idx);
            }
        }
        // Fallback to reading full profile settings if CMD_GET_ACTIVE_CPI layout varies
        let settings = self.get_profile_settings(0)?;
        Ok(settings.active_cpi_idx)
    }

    pub fn set_active_cpi_idx(&mut self, idx: u8) -> Result<(), String> {
        if idx > 4 {
            return Err("Stage index must be between 0 and 4".to_string());
        }
        let _ = self.send_command(CMD_SET_ACTIVE_CPI, &[idx], 400)?;
        Ok(())
    }

    pub fn get_profile_settings(&mut self, slot: u8) -> Result<Lift2Settings, String> {
        let pkt = self.send_command(CMD_GET_PROFILE_SETTINGS, &[slot, 0x00], 400)?;
        parse_profile_settings(&pkt)
    }

    pub fn set_profile_settings(&mut self, settings: &Lift2Settings) -> Result<(), String> {
        let payload = encode_profile_settings(settings);
        let _ = self.send_command(CMD_SET_PROFILE_SETTINGS, &payload, 400)?;
        Ok(())
    }

    pub fn set_polling_rate(&mut self, slot: u8, rate: PollingRate) -> Result<(), String> {
        let mut settings = self.get_profile_settings(slot)?;
        settings.polling_rate = rate;
        self.set_profile_settings(&settings)
    }

    pub fn set_lod(&mut self, slot: u8, lod: LiftOffDistance) -> Result<(), String> {
        let mut settings = self.get_profile_settings(slot)?;
        settings.lod = lod;
        self.set_profile_settings(&settings)
    }

    #[allow(dead_code)]
    pub fn set_dpi_stage(&mut self, slot: u8, stage_1based: u8, dpi: u32) -> Result<(), String> {
        if !(1..=5).contains(&stage_1based) {
            return Err("Stage must be between 1 and 5".to_string());
        }
        let mut settings = self.get_profile_settings(slot)?;
        settings.stages[(stage_1based - 1) as usize] = dpi.clamp(50, 26000);
        self.set_profile_settings(&settings)
    }
}

impl Drop for Lift2Device {
    fn drop(&mut self) {
        unsafe {
            if self.handle != INVALID_HANDLE_VALUE {
                CloseHandle(self.handle);
            }
            if self.read_event != 0 as HANDLE {
                CloseHandle(self.read_event);
            }
            if self.write_event != 0 as HANDLE {
                CloseHandle(self.write_event);
            }
        }
    }
}

/// Probes whether a symbolic device path is currently connected and can be opened
fn is_device_path_active(path: &str, desired_access: u32) -> bool {
    let wide: Vec<u16> = OsStr::new(path)
        .encode_wide()
        .chain(std::iter::once(0))
        .collect();
    unsafe {
        let handle = CreateFileW(
            wide.as_ptr(),
            desired_access,
            FILE_SHARE_READ | FILE_SHARE_WRITE,
            null_mut(),
            OPEN_EXISTING,
            0,
            null_mut(),
        );
        if handle != INVALID_HANDLE_VALUE {
            CloseHandle(handle);
            true
        } else {
            false
        }
    }
}

pub fn find_lift2_mi01_path() -> Result<Option<String>, String> {
    let subkeys = enum_registry_subkeys(
        HKEY_LOCAL_MACHINE,
        "SYSTEM\\CurrentControlSet\\Control\\DeviceClasses\\{4d1e55b2-f16f-11cf-88cb-001111000030}",
    )?;

    let mut fallback_candidate: Option<String> = None;

    for key in subkeys {
        let upper = key.to_uppercase();
        if SUPPORTED_VID_PIDS
            .iter()
            .any(|vp| upper.contains(&format!("{}&MI_01", vp)))
        {
            let path = key.replace("##?#", "\\\\?\\");
            if is_device_path_active(&path, FILE_GENERIC_READ | FILE_GENERIC_WRITE) {
                return Ok(Some(path));
            }
            if fallback_candidate.is_none() {
                fallback_candidate = Some(path);
            }
        }
    }

    Ok(fallback_candidate)
}

pub fn find_lift2_dpi_button_path() -> Result<Option<String>, String> {
    let subkeys = enum_registry_subkeys(
        HKEY_LOCAL_MACHINE,
        "SYSTEM\\CurrentControlSet\\Control\\DeviceClasses\\{4d1e55b2-f16f-11cf-88cb-001111000030}",
    )?;

    for key in subkeys {
        let upper = key.to_uppercase();
        if SUPPORTED_VID_PIDS
            .iter()
            .any(|vp| upper.contains(&format!("{}&MI_00&COL04", vp)))
        {
            let path = key.replace("##?#", "\\\\?\\");
            if is_device_path_active(&path, FILE_GENERIC_READ) {
                return Ok(Some(path));
            }
        }
    }

    Ok(None)
}

/// Dynamically locates the currently connected parent USB Hub and port number for the NZXT Lift 2,
/// and executes IOCTL_USB_CYCLE_PORT to perform a hardware-level bus power cycle.
pub fn cycle_usb_port() -> Result<(), String> {
    let mut active_devinst = 0u32;
    let mut active_port_number = 0u32;
    let mut found_online_instance = false;

    for vid_pid in SUPPORTED_VID_PIDS {
        let base_path = format!("SYSTEM\\CurrentControlSet\\Enum\\USB\\{}", vid_pid);
        let instances = match enum_registry_subkeys(HKEY_LOCAL_MACHINE, &base_path) {
            Ok(list) => list,
            Err(_) => continue,
        };

        for inst_id in instances {
            let device_id_str = format!("USB\\{}\\{}", vid_pid, inst_id);
            let wide_device_id: Vec<u16> = OsStr::new(&device_id_str)
                .encode_wide()
                .chain(std::iter::once(0))
                .collect();

            let mut devinst = 0u32;
            // ulFlags = 0 (CM_LOCATE_DEVNODE_NORMAL) verifies that the device is currently present in the system
            let cr = unsafe { CM_Locate_DevNodeW(&mut devinst, wide_device_id.as_ptr(), 0) };
            if cr == 0 {
                let param_key_path = format!("{}\\{}", base_path, inst_id);
                active_port_number = get_registry_dword(&param_key_path, "Address").unwrap_or(1);
                active_devinst = devinst;
                found_online_instance = true;
                break;
            }
        }

        if found_online_instance {
            break;
        }
    }

    if !found_online_instance {
        return Err("未找到当前在线的 NZXT Lift 2 USB 设备节点".to_string());
    }

    // Resolve parent USB hub using CfgMgr32
    let mut parent_devinst = 0u32;
    let mut parent_id_buf = [0u16; 512];

    unsafe {
        let cr_parent = CM_Get_Parent(&mut parent_devinst, active_devinst, 0);
        if cr_parent != 0 {
            return Err(format!("CM_Get_Parent failed with error {}", cr_parent));
        }

        let cr_id = CM_Get_Device_IDW(parent_devinst, parent_id_buf.as_mut_ptr(), 512, 0);
        if cr_id != 0 {
            return Err(format!("CM_Get_Device_IDW failed with error {}", cr_id));
        }
    }

    let nul_pos = parent_id_buf
        .iter()
        .position(|&c| c == 0)
        .unwrap_or(parent_id_buf.len());
    let parent_id = String::from_utf16_lossy(&parent_id_buf[..nul_pos]);

    // Format parent hub device path: \\?\ + parent_id with \ -> # + #{f18a0e88-c30c-11d0-8815-00a0c906bed8}
    let hub_path = format!(
        "\\\\?\\{}#{{f18a0e88-c30c-11d0-8815-00a0c906bed8}}",
        parent_id.replace('\\', "#")
    );

    let wide_hub: Vec<u16> = OsStr::new(&hub_path)
        .encode_wide()
        .chain(std::iter::once(0))
        .collect();

    unsafe {
        let handle = CreateFileW(
            wide_hub.as_ptr(),
            FILE_GENERIC_READ | FILE_GENERIC_WRITE,
            FILE_SHARE_READ | FILE_SHARE_WRITE,
            null_mut(),
            OPEN_EXISTING,
            0,
            null_mut(),
        );

        if handle == INVALID_HANDLE_VALUE {
            return Err(format!(
                "无法打开上级 USB Hub (错误码 {}，建议以管理员权限运行)",
                GetLastError()
            ));
        }

        let mut params = UsbCyclePortParams {
            connection_index: active_port_number,
            status_returned: 0,
        };
        let mut bytes_returned = 0u32;

        let ok = DeviceIoControl(
            handle,
            IOCTL_USB_CYCLE_PORT,
            &mut params as *mut _ as *mut _,
            std::mem::size_of::<UsbCyclePortParams>() as u32,
            &mut params as *mut _ as *mut _,
            std::mem::size_of::<UsbCyclePortParams>() as u32,
            &mut bytes_returned,
            null_mut(),
        );

        let err_code = GetLastError();
        CloseHandle(handle);

        if ok == 0 {
            return Err(format!(
                "IOCTL_USB_CYCLE_PORT 端口 {} 复位失败 (Win32错误码 {})",
                active_port_number, err_code
            ));
        }
    }

    // Give the USB bus 1.5 seconds to complete re-enumeration
    thread::sleep(Duration::from_millis(1500));
    Ok(())
}

pub fn fix_usb_power_settings() -> Result<(), String> {
    let mut patched_count = 0usize;

    for vid_pid in SUPPORTED_VID_PIDS {
        let base_path = format!("SYSTEM\\CurrentControlSet\\Enum\\USB\\{}", vid_pid);
        let instances = enum_registry_subkeys(HKEY_LOCAL_MACHINE, &base_path)?;

        for inst in instances {
            let param_key_path = format!("{}\\{}\\Device Parameters", base_path, inst);
            set_registry_dword(&param_key_path, "EnhancedPowerManagementEnabled", 0)?;
            set_registry_dword(&param_key_path, "DeviceSelectiveSuspended", 0)?;
            set_registry_dword(&param_key_path, "SelectiveSuspendEnabled", 0)?;
            patched_count += 1;
        }
    }

    if patched_count == 0 {
        return Err("未找到 NZXT Lift 2 注册表项".to_string());
    }

    Ok(())
}

fn get_registry_dword(sub_key: &str, value_name: &str) -> Option<u32> {
    let wide_key: Vec<u16> = OsStr::new(sub_key)
        .encode_wide()
        .chain(std::iter::once(0))
        .collect();
    let wide_val: Vec<u16> = OsStr::new(value_name)
        .encode_wide()
        .chain(std::iter::once(0))
        .collect();

    unsafe {
        let mut h_key: HKEY = 0 as HKEY;
        if RegOpenKeyExW(
            HKEY_LOCAL_MACHINE,
            wide_key.as_ptr(),
            0,
            KEY_READ,
            &mut h_key,
        ) != 0
        {
            return None;
        }

        let mut val = 0u32;
        let mut val_len = 4u32;
        let mut val_type = 0u32;
        let res = RegQueryValueExW(
            h_key,
            wide_val.as_ptr(),
            null_mut(),
            &mut val_type,
            &mut val as *mut u32 as *mut u8,
            &mut val_len,
        );
        RegCloseKey(h_key);

        if res == 0 && val_type == REG_DWORD {
            Some(val)
        } else {
            None
        }
    }
}

fn set_registry_dword(sub_key: &str, value_name: &str, value: u32) -> Result<(), String> {
    let wide_key: Vec<u16> = OsStr::new(sub_key)
        .encode_wide()
        .chain(std::iter::once(0))
        .collect();
    let wide_val: Vec<u16> = OsStr::new(value_name)
        .encode_wide()
        .chain(std::iter::once(0))
        .collect();

    unsafe {
        let mut h_key: HKEY = 0 as HKEY;
        let mut disposition = 0u32;
        let res = RegCreateKeyExW(
            HKEY_LOCAL_MACHINE,
            wide_key.as_ptr(),
            0,
            null_mut(),
            REG_OPTION_NON_VOLATILE,
            KEY_WRITE,
            null_mut(),
            &mut h_key,
            &mut disposition,
        );

        if res != 0 {
            return Err(format!("需要管理员权限 (错误码 {})", res));
        }

        let set_res = RegSetValueExW(
            h_key,
            wide_val.as_ptr(),
            0,
            REG_DWORD,
            &value as *const u32 as *const u8,
            4,
        );
        RegCloseKey(h_key);

        if set_res != 0 {
            return Err(format!(
                "写入注册表 {} 失败 (错误码 {})",
                value_name, set_res
            ));
        }
    }
    Ok(())
}

fn enum_registry_subkeys(root: HKEY, sub_key: &str) -> Result<Vec<String>, String> {
    let wide_key: Vec<u16> = OsStr::new(sub_key)
        .encode_wide()
        .chain(std::iter::once(0))
        .collect();
    let mut h_key: HKEY = 0 as HKEY;

    unsafe {
        let res = RegOpenKeyExW(root, wide_key.as_ptr(), 0, KEY_READ, &mut h_key);
        if res != 0 {
            return Ok(Vec::new());
        }

        let mut results = Vec::new();
        let mut index = 0u32;
        let mut name_buf = [0u16; 512];

        loop {
            let mut name_len = name_buf.len() as u32;
            let enum_res = RegEnumKeyExW(
                h_key,
                index,
                name_buf.as_mut_ptr(),
                &mut name_len,
                null_mut(),
                null_mut(),
                null_mut(),
                null_mut(),
            );

            if enum_res != 0 {
                break;
            }

            let s = String::from_utf16_lossy(&name_buf[..name_len as usize]);
            results.push(s);
            index += 1;
        }

        RegCloseKey(h_key);
        Ok(results)
    }
}
