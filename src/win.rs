//! Тонкі обгортки над Win32: куди насправді йде ввід, яка там розкладка, як її перемкнути, як «натиснути» клавіші.

use std::ptr::{null, null_mut};

use layout_switcher::keymap::Lang;

use windows_sys::Win32::Foundation::{
    CloseHandle, GetLastError, ERROR_ALREADY_EXISTS, GENERIC_READ, GENERIC_WRITE, HWND, INVALID_HANDLE_VALUE,
};
use windows_sys::Win32::Storage::FileSystem::{CreateFileW, FILE_SHARE_READ, FILE_SHARE_WRITE, OPEN_EXISTING};
use windows_sys::Win32::System::Console::{
    AllocConsole, GetConsoleMode, GetStdHandle, SetConsoleMode, SetConsoleTitleW, SetStdHandle, ENABLE_EXTENDED_FLAGS,
    ENABLE_QUICK_EDIT_MODE, STD_ERROR_HANDLE, STD_INPUT_HANDLE, STD_OUTPUT_HANDLE,
};
use windows_sys::Win32::System::Threading::{
    CreateMutexW, OpenProcess, QueryFullProcessImageNameW, PROCESS_QUERY_LIMITED_INFORMATION,
};
use windows_sys::Win32::UI::Input::KeyboardAndMouse::{
    GetAsyncKeyState, GetKeyboardLayout, GetKeyboardLayoutList, MapVirtualKeyW, INPUT, INPUT_0, INPUT_KEYBOARD,
    KEYBDINPUT, KEYEVENTF_KEYUP,
};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    GetForegroundWindow, GetGUIThreadInfo, GetSystemMetrics, GetWindowThreadProcessId, MessageBoxW, PostMessageW,
    GUITHREADINFO, MB_SETFOREGROUND, SM_REMOTESESSION, WM_INPUTLANGCHANGEREQUEST,
};

/// Мітка на подіях, які ввели ми самі, щоб наш же хук їх пропускав.
pub(crate) const MAGIC: usize = 0x4C41_5953;

/// Переднє вікно і вікно, яке в ньому тримає клавіатурний фокус.
#[derive(Clone, Copy)]
pub(crate) struct Target {
    pub fg: HWND,
    pub focus: HWND,
}

/// Куди насправді йде набір.
///
/// Розкладка в Windows — властивість ПОТОКУ, і важить потік вікна з фокусом, а не вікна верхнього рівня.
/// Зазвичай це той самий потік, але в застосунках на WebView2 і UWP (новий Teams, новий Outlook, «Параметри»)
/// поле вводу належить іншому процесу. Сама Windows надсилає запит на зміну мови саме вікну з фокусом —
/// робимо так само. (У лозі з Teams після 13 виправлень розкладка не перемкнулась жодного разу: запит ішов
/// вікну верхнього рівня.)
pub(crate) unsafe fn target() -> Target {
    let fg = GetForegroundWindow();
    let mut gi: GUITHREADINFO = std::mem::zeroed();
    gi.cbSize = std::mem::size_of::<GUITHREADINFO>() as u32;
    let tid = GetWindowThreadProcessId(fg, null_mut());
    let focus = if tid != 0 && GetGUIThreadInfo(tid, &mut gi) != 0 && !gi.hwndFocus.is_null() { gi.hwndFocus } else { fg };
    Target { fg, focus }
}

fn lang_of(hkl: isize) -> Option<Lang> {
    match (hkl as usize) & 0x3FF {
        0x22 => Some(Lang::Uk),
        0x09 => Some(Lang::En),
        _ => None,
    }
}

/// Мова й точний HKL розкладки потоку, якому належить вікно.
pub(crate) unsafe fn layout_of(hwnd: HWND) -> (Option<Lang>, isize) {
    let tid = GetWindowThreadProcessId(hwnd, null_mut());
    let hkl = GetKeyboardLayout(tid) as isize;
    (lang_of(hkl), hkl)
}

/// Розкладка там, куди зараз іде набір.
pub(crate) unsafe fn layout_now() -> (Option<Lang>, isize) {
    let t = target();
    let (lang, hkl) = layout_of(t.focus);
    if hkl != 0 || t.focus == t.fg {
        (lang, hkl)
    } else {
        layout_of(t.fg)
    }
}

/// Попросити вікно з фокусом перемкнути розкладку. Повертає це вікно (щоб згодом перевірити результат).
pub(crate) unsafe fn request_layout(hkl: isize) -> HWND {
    let t = target();
    PostMessageW(t.focus, WM_INPUTLANGCHANGEREQUEST, 0, hkl);
    if t.focus != t.fg {
        // запит «встановити розкладку X» ідемпотентний, тож зайвий адресат не зашкодить
        PostMessageW(t.fg, WM_INPUTLANGCHANGEREQUEST, 0, hkl);
    }
    t.focus
}

/// Ім'я exe процесу, якому належить вікно (без шляху).
pub(crate) unsafe fn exe_of(hwnd: HWND) -> String {
    let mut pid = 0u32;
    GetWindowThreadProcessId(hwnd, &mut pid);
    if pid == 0 {
        return "?".into();
    }
    let h = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid);
    if h.is_null() {
        return "?(no access)".into();
    }
    let mut buf = [0u16; 520];
    let mut len = buf.len() as u32;
    let ok = QueryFullProcessImageNameW(h, 0, buf.as_mut_ptr(), &mut len);
    CloseHandle(h);
    if ok == 0 {
        return "?".into();
    }
    let full = String::from_utf16_lossy(&buf[..len as usize]);
    full.rsplit('\\').next().unwrap_or("?").to_string()
}

pub(crate) unsafe fn installed_layouts() -> Vec<isize> {
    let n = GetKeyboardLayoutList(0, null_mut());
    if n <= 0 {
        return Vec::new();
    }
    let mut v = vec![null_mut(); n as usize];
    let got = GetKeyboardLayoutList(n, v.as_mut_ptr());
    v.truncate(got.max(0) as usize);
    v.into_iter().map(|h| h as isize).collect()
}

pub(crate) fn hkl_hex(hkl: isize) -> String {
    format!("{:08X}", hkl as usize & 0xFFFF_FFFF)
}

pub(crate) fn key_input(vk: u16, scan: u16, flags: u32) -> INPUT {
    INPUT {
        r#type: INPUT_KEYBOARD,
        Anonymous: INPUT_0 { ki: KEYBDINPUT { wVk: vk, wScan: scan, dwFlags: flags, time: 0, dwExtraInfo: MAGIC } },
    }
}

/// Натиснути й відпустити клавішу.
pub(crate) unsafe fn tap(v: &mut Vec<INPUT>, vk: u16) {
    let sc = MapVirtualKeyW(vk as u32, 0) as u16;
    v.push(key_input(vk, sc, 0));
    v.push(key_input(vk, sc, KEYEVENTF_KEYUP));
}

pub(crate) unsafe fn down(vk: u16) -> bool {
    (GetAsyncKeyState(vk as i32) as u16 & 0x8000) != 0
}

/// Чи працюємо всередині сеансу віддаленого робочого стола (RDP).
pub(crate) unsafe fn is_remote_session() -> bool {
    GetSystemMetrics(SM_REMOTESESSION) != 0
}

/// Рядок для Win32 (UTF-16 з нулем у кінці).
pub(crate) fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

/// Вікно повідомлення (програма не має консолі, тож помилки й довідка показуються так).
pub(crate) fn message_box(text: &str, flags: u32) -> i32 {
    let (t, c) = (wide(text), wide("layout-switcher"));
    unsafe { MessageBoxW(null_mut(), t.as_ptr(), c.as_ptr(), flags | MB_SETFOREGROUND) }
}

/// Відкрити власне вікно консолі (`--console`). Програма зібрана без консолі, тож стандартних дескрипторів у неї
/// не було — прив'язуємо їх до нового вікна.
pub(crate) unsafe fn alloc_console() -> bool {
    if AllocConsole() == 0 {
        return false;
    }
    SetConsoleTitleW(wide("layout-switcher: консоль (закриття вікна завершує програму)").as_ptr());
    let name = wide("CONOUT$");
    let h = CreateFileW(
        name.as_ptr(),
        GENERIC_READ | GENERIC_WRITE,
        FILE_SHARE_READ | FILE_SHARE_WRITE,
        null(),
        OPEN_EXISTING,
        0,
        null_mut(),
    );
    if h != INVALID_HANDLE_VALUE {
        SetStdHandle(STD_OUTPUT_HANDLE, h);
        SetStdHandle(STD_ERROR_HANDLE, h);
    }
    true
}

/// Вимкнути у своїй консолі «швидке редагування»: інакше клік у вікні зупиняє вивід програми.
pub(crate) unsafe fn disable_quick_edit() {
    let h = GetStdHandle(STD_INPUT_HANDLE);
    let mut mode = 0u32;
    if !h.is_null() && GetConsoleMode(h, &mut mode) != 0 {
        SetConsoleMode(h, (mode & !ENABLE_QUICK_EDIT_MODE) | ENABLE_EXTENDED_FLAGS);
    }
}

/// Один екземпляр на сеанс Windows. Два екземпляри виправляли б кожне слово двічі й псували текст.
/// Простір імен `Local\` свій у кожному сеансі, тож у сеансі RDP можна запустити власний екземпляр.
pub(crate) unsafe fn single_instance() -> bool {
    let name: Vec<u16> = "Local\\layout-switcher-instance".encode_utf16().chain(std::iter::once(0)).collect();
    let h = CreateMutexW(null(), 0, name.as_ptr());
    // дескриптор навмисно не закриваємо: м'ютекс живе, доки живе процес
    !(h.is_null() || GetLastError() == ERROR_ALREADY_EXISTS)
}

/// Дочекатись завершення процесу з цим номером (до `timeout_ms`). Якщо його вже нема — повертається одразу.
pub(crate) fn wait_for_process(pid: u32, timeout_ms: u32) {
    use windows_sys::Win32::Foundation::CloseHandle;
    use windows_sys::Win32::System::Threading::{OpenProcess, WaitForSingleObject, PROCESS_SYNCHRONIZE};
    unsafe {
        let h = OpenProcess(PROCESS_SYNCHRONIZE, 0, pid);
        if !h.is_null() {
            WaitForSingleObject(h, timeout_ms);
            CloseHandle(h);
        }
    }
}
