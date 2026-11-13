use image::ImageFormat;
use std::ffi::c_void;
use std::io::Cursor;
use std::os::windows::process::CommandExt;
use std::process::Command;
use sysinfo::System;
use windows_sys::core::GUID;
use windows_sys::Win32::System::Com::{
    CoCreateInstance, CoInitializeEx, CoUninitialize, CLSCTX_ALL, COINIT_MULTITHREADED,
};
use windows_sys::Win32::System::Power::SetSuspendState;
use windows_sys::Win32::System::Shutdown::LockWorkStation;
use windows_sys::Win32::UI::Input::KeyboardAndMouse::{
    keybd_event, SendInput, INPUT, INPUT_0, INPUT_KEYBOARD, KEYBDINPUT, KEYEVENTF_EXTENDEDKEY,
    KEYEVENTF_KEYUP, VK_MEDIA_PLAY_PAUSE, VK_VOLUME_DOWN, VK_VOLUME_MUTE, VK_VOLUME_UP,
};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    FindWindowW, GetDesktopWindow, SendMessageW, WM_APPCOMMAND,
};
use winreg::enums::HKEY_CURRENT_USER;
use winreg::RegKey;
use xcap::Monitor;

const CREATE_NO_WINDOW: u32 = 0x08000000;
const REG_RUN_PATH: &str = r"Software\Microsoft\Windows\CurrentVersion\Run";
const REG_APP_NAME: &str = "RemotePCAgent";

const CLSID_MM_DEVICE_ENUMERATOR: GUID = GUID {
    data1: 0xBCDE0395,
    data2: 0xE52F,
    data3: 0x467C,
    data4: [0x8E, 0x3D, 0xC4, 0x57, 0x92, 0x91, 0x69, 0x2E],
};

const IID_IMM_DEVICE_ENUMERATOR: GUID = GUID {
    data1: 0xA95664D2,
    data2: 0x9614,
    data3: 0x4F35,
    data4: [0xA7, 0x46, 0xDE, 0x8D, 0xB6, 0x36, 0x17, 0xE6],
};

const IID_IAUDIO_ENDPOINT_VOLUME: GUID = GUID {
    data1: 0x5CDF2C82,
    data2: 0x841E,
    data3: 0x4546,
    data4: [0x97, 0x22, 0x0C, 0xF7, 0x40, 0x78, 0x22, 0x9A],
};

const APPCOMMAND_VOLUME_MUTE: u32 = 8;
const APPCOMMAND_VOLUME_DOWN: u32 = 9;
const APPCOMMAND_VOLUME_UP: u32 = 10;
const APPCOMMAND_MEDIA_PLAY_PAUSE: u32 = 14;

#[repr(C)]
struct IMMDeviceEnumeratorVtbl {
    query_interface: unsafe extern "system" fn(*mut c_void, *const GUID, *mut *mut c_void) -> i32,
    add_ref: unsafe extern "system" fn(*mut c_void) -> u32,
    release: unsafe extern "system" fn(*mut c_void) -> u32,
    enum_audio_endpoints: unsafe extern "system" fn(*mut c_void, i32, u32, *mut *mut c_void) -> i32,
    get_default_audio_endpoint:
        unsafe extern "system" fn(*mut c_void, i32, i32, *mut *mut c_void) -> i32,
}

#[repr(C)]
struct IMMDeviceVtbl {
    query_interface: unsafe extern "system" fn(*mut c_void, *const GUID, *mut *mut c_void) -> i32,
    add_ref: unsafe extern "system" fn(*mut c_void) -> u32,
    release: unsafe extern "system" fn(*mut c_void) -> u32,
    activate: unsafe extern "system" fn(
        *mut c_void,
        *const GUID,
        u32,
        *const c_void,
        *mut *mut c_void,
    ) -> i32,
}

#[repr(C)]
struct IAudioEndpointVolumeVtbl {
    query_interface: unsafe extern "system" fn(*mut c_void, *const GUID, *mut *mut c_void) -> i32,
    add_ref: unsafe extern "system" fn(*mut c_void) -> u32,
    release: unsafe extern "system" fn(*mut c_void) -> u32,
    register_control_change_notify: unsafe extern "system" fn(*mut c_void, *mut c_void) -> i32,
    unregister_control_change_notify: unsafe extern "system" fn(*mut c_void, *mut c_void) -> i32,
    get_channel_count: unsafe extern "system" fn(*mut c_void, *mut u32) -> i32,
    set_master_volume_level: unsafe extern "system" fn(*mut c_void, f32, *const GUID) -> i32,
    set_master_volume_level_scalar: unsafe extern "system" fn(*mut c_void, f32, *const GUID) -> i32,
    get_master_volume_level: unsafe extern "system" fn(*mut c_void, *mut f32) -> i32,
    get_master_volume_level_scalar: unsafe extern "system" fn(*mut c_void, *mut f32) -> i32,
    set_channel_volume_level:
        unsafe extern "system" fn(*mut c_void, u32, f32, *const GUID) -> i32,
    set_channel_volume_level_scalar:
        unsafe extern "system" fn(*mut c_void, u32, f32, *const GUID) -> i32,
    get_channel_volume_level: unsafe extern "system" fn(*mut c_void, u32, *mut f32) -> i32,
    get_channel_volume_level_scalar: unsafe extern "system" fn(*mut c_void, u32, *mut f32) -> i32,
    set_mute: unsafe extern "system" fn(*mut c_void, i32, *const GUID) -> i32,
    get_mute: unsafe extern "system" fn(*mut c_void, *mut i32) -> i32,
}

/// Вызов замыкания с указателем на системный интерфейс IAudioEndpointVolume
fn with_endpoint_volume<F, R>(f: F) -> Option<R>
where
    F: FnOnce(*mut c_void, &IAudioEndpointVolumeVtbl) -> Option<R>,
{
    unsafe {
        let _ = CoInitializeEx(std::ptr::null_mut(), COINIT_MULTITHREADED as u32);

        let mut enumerator_ptr: *mut c_void = std::ptr::null_mut();
        if CoCreateInstance(
            &CLSID_MM_DEVICE_ENUMERATOR,
            std::ptr::null_mut(),
            CLSCTX_ALL,
            &IID_IMM_DEVICE_ENUMERATOR,
            &mut enumerator_ptr,
        ) != 0
        {
            return None;
        }

        let enumerator_vtbl = *(enumerator_ptr as *const *const IMMDeviceEnumeratorVtbl);
        let mut device_ptr: *mut c_void = std::ptr::null_mut();
        // eRender = 0, eMultimedia = 1
        if ((*enumerator_vtbl).get_default_audio_endpoint)(enumerator_ptr, 0, 1, &mut device_ptr)
            != 0
        {
            ((*enumerator_vtbl).release)(enumerator_ptr);
            return None;
        }

        let device_vtbl = *(device_ptr as *const *const IMMDeviceVtbl);
        let mut volume_ptr: *mut c_void = std::ptr::null_mut();
        if ((*device_vtbl).activate)(
            device_ptr,
            &IID_IAUDIO_ENDPOINT_VOLUME,
            CLSCTX_ALL,
            std::ptr::null(),
            &mut volume_ptr,
        ) != 0
        {
            ((*device_vtbl).release)(device_ptr);
            ((*enumerator_vtbl).release)(enumerator_ptr);
            return None;
        }

        let volume_vtbl = *(volume_ptr as *const *const IAudioEndpointVolumeVtbl);
        let result = f(volume_ptr, &*volume_vtbl);

        ((*volume_vtbl).release)(volume_ptr);
        ((*device_vtbl).release)(device_ptr);
        ((*enumerator_vtbl).release)(enumerator_ptr);
        CoUninitialize();

        result
    }
}

/// Отправка команды Windows Shell (WM_APPCOMMAND) в проводник Windows
fn send_shell_app_command(cmd: u32) {
    unsafe {
        let mut class_name: Vec<u16> = "Shell_TrayWnd".encode_utf16().collect();
        class_name.push(0);
        let mut hwnd = FindWindowW(class_name.as_ptr(), std::ptr::null());
        if hwnd.is_null() {
            hwnd = GetDesktopWindow();
        }
        SendMessageW(
            hwnd,
            WM_APPCOMMAND,
            hwnd as usize,
            ((cmd as usize) << 16) as isize,
        );
    }
}

/// Глобальная эмуляция нажатия мультимедийной клавиши через keybd_event
fn send_keybd_event(vk: u16, scan: u8) {
    unsafe {
        keybd_event(vk as u8, scan, KEYEVENTF_EXTENDEDKEY, 0);
        keybd_event(
            vk as u8,
            scan,
            KEYEVENTF_EXTENDEDKEY | KEYEVENTF_KEYUP,
            0,
        );
    }
}

/// Эмуляция нажатия и отпускания виртуальной клавиши через SendInput
pub fn press_key(vk: u16) -> u32 {
    let mut inputs = [
        INPUT {
            r#type: INPUT_KEYBOARD,
            Anonymous: INPUT_0 {
                ki: KEYBDINPUT {
                    wVk: vk,
                    wScan: 0,
                    dwFlags: KEYEVENTF_EXTENDEDKEY,
                    time: 0,
                    dwExtraInfo: 0,
                },
            },
        },
        INPUT {
            r#type: INPUT_KEYBOARD,
            Anonymous: INPUT_0 {
                ki: KEYBDINPUT {
                    wVk: vk,
                    wScan: 0,
                    dwFlags: KEYEVENTF_KEYUP | KEYEVENTF_EXTENDEDKEY,
                    time: 0,
                    dwExtraInfo: 0,
                },
            },
        },
    ];

    unsafe {
        SendInput(
            inputs.len() as u32,
            inputs.as_mut_ptr(),
            std::mem::size_of::<INPUT>() as i32,
        )
    }
}

/// Увеличение громкости на 10% (возвращает актуальный % громкости)
pub fn volume_up_10() -> u32 {
    let new_percent = with_endpoint_volume(|ptr, vtbl| unsafe {
        let mut current: f32 = 0.0;
        if ((*vtbl).get_master_volume_level_scalar)(ptr, &mut current) == 0 {
            let next = (current + 0.10).min(1.0);
            let _ = ((*vtbl).set_master_volume_level_scalar)(ptr, next, std::ptr::null());
            // Автоматически снимаем mute при повышении громкости
            let _ = ((*vtbl).set_mute)(ptr, 0, std::ptr::null());
            Some((next * 100.0).round() as u32)
        } else {
            None
        }
    });

    send_shell_app_command(APPCOMMAND_VOLUME_UP);
    send_keybd_event(VK_VOLUME_UP, 0x30);
    press_key(VK_VOLUME_UP);

    new_percent.unwrap_or(0)
}

/// Уменьшение громкости на 10% (возвращает актуальный % громкости)
pub fn volume_down_10() -> u32 {
    let new_percent = with_endpoint_volume(|ptr, vtbl| unsafe {
        let mut current: f32 = 0.0;
        if ((*vtbl).get_master_volume_level_scalar)(ptr, &mut current) == 0 {
            let next = (current - 0.10).max(0.0);
            let _ = ((*vtbl).set_master_volume_level_scalar)(ptr, next, std::ptr::null());
            Some((next * 100.0).round() as u32)
        } else {
            None
        }
    });

    send_shell_app_command(APPCOMMAND_VOLUME_DOWN);
    send_keybd_event(VK_VOLUME_DOWN, 0x2E);
    press_key(VK_VOLUME_DOWN);

    new_percent.unwrap_or(0)
}

/// Переключение режима без звука (возвращает true если звук выключен)
pub fn volume_mute() -> bool {
    let is_muted = with_endpoint_volume(|ptr, vtbl| unsafe {
        let mut muted: i32 = 0;
        if ((*vtbl).get_mute)(ptr, &mut muted) == 0 {
            let new_muted = if muted != 0 { 0 } else { 1 };
            let _ = ((*vtbl).set_mute)(ptr, new_muted, std::ptr::null());
            Some(new_muted != 0)
        } else {
            None
        }
    });

    send_shell_app_command(APPCOMMAND_VOLUME_MUTE);
    send_keybd_event(VK_VOLUME_MUTE, 0x20);
    press_key(VK_VOLUME_MUTE);

    is_muted.unwrap_or(false)
}

/// Воспроизведение / Пауза мультимедиа
pub fn media_play_pause() {
    send_shell_app_command(APPCOMMAND_MEDIA_PLAY_PAUSE);
    send_keybd_event(VK_MEDIA_PLAY_PAUSE, 0x22);
    press_key(VK_MEDIA_PLAY_PAUSE);
}

/// Блокировка Windows (Win + L)
pub fn lock_workstation() -> bool {
    unsafe { LockWorkStation() != 0 }
}

/// Переход в спящий режим
pub fn sleep_pc() -> bool {
    unsafe { SetSuspendState(false, false, false) }
}

/// Немедленное выключение ПК
pub fn shutdown_now() {
    let _ = Command::new("shutdown")
        .args(["/s", "/t", "0", "/f"])
        .creation_flags(CREATE_NO_WINDOW)
        .spawn();
}

/// Немедленная перезагрузка ПК
pub fn reboot_now() {
    let _ = Command::new("shutdown")
        .args(["/r", "/t", "0", "/f"])
        .creation_flags(CREATE_NO_WINDOW)
        .spawn();
}

/// Установка таймера выключения ПК
pub fn schedule_shutdown(seconds: u32) {
    let _ = Command::new("shutdown")
        .args(["/s", "/t", &seconds.to_string()])
        .creation_flags(CREATE_NO_WINDOW)
        .spawn();
}

/// Отмена запланированного выключения ПК
pub fn cancel_shutdown() {
    let _ = Command::new("shutdown")
        .args(["/a"])
        .creation_flags(CREATE_NO_WINDOW)
        .spawn();
}

/// Проверка наличия записи в автозагрузке Windows через реестр
pub fn is_autostart_enabled() -> bool {
    let hkcu = RegKey::predef(HKEY_CURRENT_USER);
    if let Ok(key) = hkcu.open_subkey(REG_RUN_PATH) {
        key.get_value::<String, _>(REG_APP_NAME).is_ok()
    } else {
        false
    }
}

/// Включение/выключение автозагрузки Windows через HKCU\Software\Microsoft\Windows\CurrentVersion\Run
pub fn set_autostart(enable: bool) -> Result<(), std::io::Error> {
    let hkcu = RegKey::predef(HKEY_CURRENT_USER);
    let (key, _) = hkcu.create_subkey(REG_RUN_PATH)?;

    if enable {
        let exe_path = std::env::current_exe()?;
        let exe_str = format!("\"{}\"", exe_path.to_string_lossy());
        key.set_value(REG_APP_NAME, &exe_str)?;
    } else {
        let _ = key.delete_value(REG_APP_NAME);
    }

    Ok(())
}

/// Захват главного экрана и кодирование в PNG прямо в памяти через Cursor (без сохранения на диск)
pub fn capture_primary_screenshot() -> Result<Vec<u8>, Box<dyn std::error::Error + Send + Sync>> {
    let monitors = Monitor::all()?;
    let monitor = monitors
        .iter()
        .find(|m| m.is_primary().unwrap_or(false))
        .or_else(|| monitors.first())
        .ok_or("Основной монитор не найден")?;

    let rgba_image = monitor.capture_image()?;
    let mut cursor = Cursor::new(Vec::new());
    rgba_image.write_to(&mut cursor, ImageFormat::Png)?;
    Ok(cursor.into_inner())
}

/// Системные показатели нагрузки
pub struct SysMetrics {
    pub cpu_usage: f32,
    pub used_ram_gb: f64,
    pub total_ram_gb: f64,
    pub ram_percent: u32,
}

/// Получение системных метрик CPU и RAM
pub fn fetch_system_metrics(sys: &mut System) -> SysMetrics {
    sys.refresh_cpu_usage();
    sys.refresh_memory();

    let cpu = sys.global_cpu_usage();
    let used_bytes = sys.used_memory();
    let total_bytes = sys.total_memory();

    let used_ram_gb = used_bytes as f64 / (1024.0 * 1024.0 * 1024.0);
    let total_ram_gb = total_bytes as f64 / (1024.0 * 1024.0 * 1024.0);
    let ram_percent = if total_bytes > 0 {
        ((used_bytes as f64 / total_bytes as f64) * 100.0) as u32
    } else {
        0
    };

    SysMetrics {
        cpu_usage: cpu,
        used_ram_gb,
        total_ram_gb,
        ram_percent,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_core_audio_volume() {
        let vol = volume_up_10();
        println!("Volume after up: {}%", vol);
        assert!(vol <= 100);

        let vol_down = volume_down_10();
        println!("Volume after down: {}%", vol_down);
        assert!(vol_down <= 100);
    }
}
