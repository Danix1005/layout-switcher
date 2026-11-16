//! Тестовий інструмент (лише Windows): надіслати запущеній програмі команду меню значка за номером.
//! Так без миші перевіряються пункти меню під Wine. Номери — у `src/tray.rs` (CMD_*):
//! 1001 пауза/відновити, 1010 лог вимкнути, 1011 лог без слів, 1013 показати лог, 1020 автозапуск,
//! 1030 перевірити оновлення, 1031 перевірка раз на добу (вмик./вимк.), 1032 оновити, 1099 вийти.
//!
//!     wm_command.exe 1011

#[cfg(windows)]
fn main() {
    use std::ptr::null;
    use windows_sys::Win32::UI::WindowsAndMessaging::{FindWindowW, PostMessageW, WM_COMMAND};

    let id: usize = match std::env::args().nth(1).and_then(|a| a.parse().ok()) {
        Some(v) => v,
        None => {
            eprintln!("використання: wm_command <номер команди>");
            std::process::exit(2);
        }
    };
    let class: Vec<u16> = "LayoutSwitcherTray".encode_utf16().chain(std::iter::once(0)).collect();
    unsafe {
        let hwnd = FindWindowW(class.as_ptr(), null());
        if hwnd.is_null() {
            eprintln!("layout-switcher не запущено");
            std::process::exit(1);
        }
        PostMessageW(hwnd, WM_COMMAND, id, 0);
    }
}

#[cfg(not(windows))]
fn main() {
    eprintln!("інструмент працює лише в Windows");
}
