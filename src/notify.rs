//! Alarm playback and taskbar flash for the end of a session.

#[cfg(target_os = "windows")]
mod windows {
    use std::ffi::c_void;
    use std::fs;
    use std::path::PathBuf;

    const ALARM_BYTES: &[u8] = include_bytes!("../assets/alarm.mp3");
    const ALARM_ALIAS: &str = "gomo_alarm";

    #[link(name = "winmm")]
    extern "system" {
        fn mciSendStringW(
            command: *const u16,
            return_string: *mut u16,
            return_length: u32,
            callback: *mut c_void,
        ) -> u32;
    }

    #[link(name = "user32")]
    extern "system" {
        fn MessageBeep(u_type: u32) -> i32;
        fn FindWindowW(class_name: *const u16, window_name: *const u16) -> *mut c_void;
        fn GetForegroundWindow() -> *mut c_void;
        fn FlashWindowEx(info: *mut FlashInfo) -> i32;
    }

    #[repr(C)]
    struct FlashInfo {
        size: u32,
        window: *mut c_void,
        flags: u32,
        count: u32,
        timeout: u32,
    }

    fn alarm_path() -> Option<PathBuf> {
        let mut dir = crate::paths::data_dir();
        fs::create_dir_all(&dir).ok()?;
        dir.push("alarm.mp3");

        let current_size = fs::metadata(&dir).map(|meta| meta.len()).unwrap_or(0);
        if current_size != ALARM_BYTES.len() as u64 {
            fs::write(&dir, ALARM_BYTES).ok()?;
        }
        Some(dir)
    }

    fn mci(command: &str) -> bool {
        let command: Vec<u16> = command.encode_utf16().chain(std::iter::once(0)).collect();
        let mut output = [0u16; 128];
        unsafe {
            mciSendStringW(
                command.as_ptr(),
                output.as_mut_ptr(),
                output.len() as u32,
                std::ptr::null_mut(),
            ) == 0
        }
    }

    pub fn play_alarm(volume: u8) {
        if volume == 0 {
            return;
        }
        let Some(path) = alarm_path() else {
            fallback_beep();
            return;
        };
        let path = path.display();

        let _ = mci(&format!("close {ALARM_ALIAS}"));
        let opened = mci(&format!(
            "open \"{path}\" type mpegvideo alias {ALARM_ALIAS}"
        ));
        if opened {
            let _ = mci(&format!(
                "setaudio {ALARM_ALIAS} volume to {}",
                u32::from(volume.min(100)) * 10
            ));
            if mci(&format!("play {ALARM_ALIAS}")) {
                return;
            }
        }

        eprintln!("[alarm] MCI playback failed; falling back to system beep");
        crate::logging::error("alarm", "MCI playback failed; used system beep");
        fallback_beep();
    }

    pub fn stop_alarm() {
        let _ = mci(&format!("close {ALARM_ALIAS}"));
    }

    pub fn flash_taskbar() {
        let title: Vec<u16> = "Gomo".encode_utf16().chain(std::iter::once(0)).collect();
        let window = unsafe { FindWindowW(std::ptr::null(), title.as_ptr()) };
        if window.is_null() || unsafe { GetForegroundWindow() } == window {
            return;
        }

        let mut info = FlashInfo {
            size: std::mem::size_of::<FlashInfo>() as u32,
            window,
            flags: 0x0000000F, // FLASHW_ALL | FLASHW_TIMERNOFG
            count: 3,
            timeout: 0,
        };
        unsafe {
            FlashWindowEx(&mut info);
        }
    }

    fn fallback_beep() {
        unsafe {
            // MB_ICONASTERISK
            MessageBeep(0x00000040);
        }
    }
}

#[cfg(target_os = "windows")]
pub use windows::{flash_taskbar, play_alarm, stop_alarm};

#[cfg(not(target_os = "windows"))]
pub fn play_alarm() {
    print!("\x07");
}

#[cfg(not(target_os = "windows"))]
pub fn stop_alarm() {}

#[cfg(not(target_os = "windows"))]
pub fn flash_taskbar() {}

pub fn notify_session_complete(title: &str, message: &str) {
    println!("[NOTIFICATION] {}: {}", title, message);
    #[cfg(target_os = "windows")]
    flash_taskbar();
}
