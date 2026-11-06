#[cfg(target_os = "windows")]
mod windows {
    use std::ffi::c_void;
    use std::sync::atomic::{AtomicIsize, Ordering};

    const ERROR_ALREADY_EXISTS: u32 = 183;
    const MUTEX_NAME: &str = "Local\\GomoSingleInstance";
    const EVENT_NAME: &str = "Local\\GomoActivate";
    const WINDOW_TITLE: &str = "Gomo";
    const EVENT_MODIFY_STATE: u32 = 0x0002;
    const WAIT_OBJECT_0: u32 = 0;

    static ACTIVATION_EVENT: AtomicIsize = AtomicIsize::new(0);

    #[link(name = "kernel32")]
    extern "system" {
        fn CreateMutexW(
            attributes: *mut c_void,
            initial_owner: i32,
            name: *const u16,
        ) -> *mut c_void;
        fn CreateEventW(
            attributes: *mut c_void,
            manual_reset: i32,
            initial_state: i32,
            name: *const u16,
        ) -> *mut c_void;
        fn OpenEventW(access: u32, inherit: i32, name: *const u16) -> *mut c_void;
        fn SetEvent(event: *mut c_void) -> i32;
        fn WaitForSingleObject(handle: *mut c_void, milliseconds: u32) -> u32;
        fn CloseHandle(handle: *mut c_void) -> i32;
        fn GetLastError() -> u32;
    }

    #[link(name = "user32")]
    extern "system" {
        fn FindWindowW(class_name: *const u16, window_name: *const u16) -> *mut c_void;
        fn ShowWindow(window: *mut c_void, command: i32) -> i32;
        fn SetForegroundWindow(window: *mut c_void) -> i32;
    }

    fn wide(text: &str) -> Vec<u16> {
        text.encode_utf16().chain(std::iter::once(0)).collect()
    }

    pub fn focus_existing() -> bool {
        let title = wide(WINDOW_TITLE);
        let window = unsafe { FindWindowW(std::ptr::null(), title.as_ptr()) };
        if window.is_null() {
            return false;
        }
        unsafe {
            ShowWindow(window, 9); // SW_RESTORE
            SetForegroundWindow(window);
        }
        true
    }

    pub fn acquire() -> bool {
        let name = wide(MUTEX_NAME);
        let mutex = unsafe { CreateMutexW(std::ptr::null_mut(), 0, name.as_ptr()) };
        if mutex.is_null() {
            return true;
        }
        if unsafe { GetLastError() } == ERROR_ALREADY_EXISTS {
            let event_name = wide(EVENT_NAME);
            let event = unsafe { OpenEventW(EVENT_MODIFY_STATE, 0, event_name.as_ptr()) };
            if !event.is_null() {
                unsafe {
                    SetEvent(event);
                    CloseHandle(event);
                }
            }
            return false;
        }

        let event_name = wide(EVENT_NAME);
        let event = unsafe { CreateEventW(std::ptr::null_mut(), 0, 0, event_name.as_ptr()) };
        ACTIVATION_EVENT.store(event as isize, Ordering::SeqCst);
        // handle stays open on purpose, for the process lifetime
        let _ = mutex;
        true
    }

    pub fn activation_requested() -> bool {
        let handle = ACTIVATION_EVENT.load(Ordering::SeqCst);
        if handle == 0 {
            return false;
        }
        unsafe { WaitForSingleObject(handle as *mut c_void, 0) == WAIT_OBJECT_0 }
    }
}

#[cfg(target_os = "windows")]
pub use windows::{acquire, activation_requested, focus_existing};

#[cfg(not(target_os = "windows"))]
pub fn acquire() -> bool {
    true
}

#[cfg(not(target_os = "windows"))]
pub fn activation_requested() -> bool {
    false
}

#[cfg(not(target_os = "windows"))]
pub fn focus_existing() -> bool {
    false
}
