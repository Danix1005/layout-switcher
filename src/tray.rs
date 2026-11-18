//! Значок біля годинника: стан («працює» / «призупинено»), меню, повідомлення, керування ззовні.
//!
//! Програма не має вікна консолі. Усе, що раніше було в консолі, тепер у меню значка: пауза, запис логу,
//! автозапуск з Windows, вихід. Вікно значка створюється в тому самому потоці, що й хуки клавіатури, тож
//! головний цикл повідомлень обслуговує і хуки, і значок; уся робота з інтерфейсом — лише в цьому потоці.
//! З інших потоків (хук, фоновий потік виділеного) можна тільки `notify`: він кладе повідомлення в чергу
//! й будить вікно, а показує його вже головний потік.

use std::cell::RefCell;
use std::ffi::c_void;
use std::mem::{size_of, zeroed};
use std::ptr::{null, null_mut};
use std::sync::atomic::{AtomicBool, AtomicPtr, Ordering};
use std::sync::Mutex;

use layout_switcher::cli::Control;

use windows_sys::Win32::Foundation::{HWND, LPARAM, LRESULT, POINT, WPARAM};
use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
use windows_sys::Win32::UI::Shell::{
    ShellExecuteW, Shell_NotifyIconW, NIF_ICON, NIF_INFO, NIF_MESSAGE, NIF_TIP, NIIF_INFO, NIM_ADD, NIM_DELETE,
    NIM_MODIFY, NIN_BALLOONHIDE, NIN_BALLOONTIMEOUT, NIN_BALLOONUSERCLICK, NOTIFYICONDATAW,
};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    AppendMenuW, CreatePopupMenu, CreateWindowExW, DefWindowProcW, DestroyMenu, DestroyWindow, FindWindowW,
    GetCursorPos, GetSystemMetrics, LoadIconW, LoadImageW, PostMessageW, PostQuitMessage, RegisterClassExW,
    RegisterWindowMessageW, SetForegroundWindow, TrackPopupMenu, HICON, IDI_APPLICATION, IMAGE_ICON, LR_SHARED,
    MB_ICONERROR, MB_ICONWARNING, MB_OKCANCEL, MF_CHECKED, MF_DISABLED, MF_GRAYED, MF_POPUP, MF_SEPARATOR,
    MF_STRING, SM_CXSMICON, TPM_BOTTOMALIGN, TPM_RETURNCMD, TPM_RIGHTBUTTON, WM_APP, WM_COMMAND, WM_DESTROY,
    WM_ENDSESSION, WM_LBUTTONUP, WM_NULL, WM_RBUTTONUP, WNDCLASSEXW, WS_EX_TOOLWINDOW, WS_POPUP,
};

use crate::logger::log_event;
use crate::{appsoff, autostart, engine, logger, settings, updater, win};
use layout_switcher::apps::{AppMode, AppRules, OffReason};
use layout_switcher::cli::quote_command;

/// Ім'я класу вікна значка: за ним `--pause` / `--resume` / `--exit` знаходять запущену програму.
const CLASS: &str = "LayoutSwitcherTray";
const VERSION: &str = env!("CARGO_PKG_VERSION");

const WM_TRAY: u32 = WM_APP + 1; // подія від значка
const WM_NOTICE: u32 = WM_APP + 2; // «є повідомлення в черзі»

const CMD_TOGGLE: usize = 1001;
const CMD_PAUSE: usize = 1002;
const CMD_RESUME: usize = 1003;
const CMD_LOG_OFF: usize = 1010;
const CMD_LOG_PLAIN: usize = 1011;
const CMD_LOG_WORDS: usize = 1012;
const CMD_OPEN_LOG: usize = 1013;
const CMD_OPEN_LEARNED: usize = 1014;
const CMD_APP_OFF: usize = 1015;
const CMD_AUTOSTART: usize = 1020;
const CMD_UPDATE_CHECK: usize = 1030;
const CMD_UPDATE_AUTO: usize = 1031;
const CMD_UPDATE_INSTALL: usize = 1032;
const CMD_EXIT: usize = 1099;

const ICON_ON: usize = 1; // номери значків у ресурсах (assets/app.rc)
const ICON_OFF: usize = 2;

struct Tray {
    hwnd: HWND,
    icon_on: HICON,
    icon_off: HICON,
    /// Номер повідомлення «TaskbarCreated»: Провідник перезапустився, значок треба повернути.
    taskbar_created: u32,
    /// Рядок запуску для запису автозапуску.
    autostart_cmd: String,
}

thread_local! {
    static TRAY: RefCell<Option<Tray>> = const { RefCell::new(None) };
}

/// Вікно значка для `notify` з інших потоків.
static HWND_PTR: AtomicPtr<c_void> = AtomicPtr::new(null_mut());
/// Повідомлення в черзі: (заголовок, текст, чи це «вийшла нова версія» — клік по такому запускає оновлення).
static NOTICES: Mutex<Vec<(String, String, bool)>> = Mutex::new(Vec::new());
/// Повідомлення, що зараз на екрані, — про оновлення (тоді клік по ньому запускає встановлення).
static BALLOON_IS_UPDATE: AtomicBool = AtomicBool::new(false);

fn with_tray<R>(f: impl FnOnce(&Tray) -> R) -> Option<R> {
    TRAY.with(|c| c.try_borrow().ok().and_then(|g| g.as_ref().map(f)))
}

// ───────────────────────────── створення ─────────────────────────────

/// Створити вікно й значок. `announce` — показати підказку про значок (при ручному запуску).
pub(crate) unsafe fn create(autostart_cmd: String, announce: bool) -> bool {
    let hinst = GetModuleHandleW(null());
    let class = win::wide(CLASS);
    let mut wc: WNDCLASSEXW = zeroed();
    wc.cbSize = size_of::<WNDCLASSEXW>() as u32;
    wc.lpfnWndProc = Some(wnd_proc);
    wc.hInstance = hinst;
    wc.lpszClassName = class.as_ptr();
    if RegisterClassExW(&wc) == 0 {
        return false;
    }
    let title = win::wide("layout-switcher");
    let hwnd =
        CreateWindowExW(WS_EX_TOOLWINDOW, class.as_ptr(), title.as_ptr(), WS_POPUP, 0, 0, 0, 0, null_mut(), null_mut(), hinst, null());
    if hwnd.is_null() {
        return false;
    }

    // Значки з ресурсів exe. Розмір — той, що Windows просить для області сповіщень (16 за 100 % масштабу).
    let px = GetSystemMetrics(SM_CXSMICON).max(16);
    let load = |id: usize| {
        let h = LoadImageW(hinst, id as *const u16, IMAGE_ICON, px, px, LR_SHARED) as HICON;
        if h.is_null() {
            (LoadIconW(null_mut(), IDI_APPLICATION), false)
        } else {
            (h, true)
        }
    };
    let ((icon_on, on_ok), (icon_off, off_ok)) = (load(ICON_ON), load(ICON_OFF));
    log_event(
        "tray_icons",
        vec![
            ("icon_px", px.to_string()),
            ("icon_on", (on_ok as u8).to_string()),
            ("icon_off", (off_ok as u8).to_string()),
            ("announce", (announce as u8).to_string()),
        ],
    );
    let taskbar_created = RegisterWindowMessageW(win::wide("TaskbarCreated").as_ptr());

    // Автозапуск, якщо він увімкнений, але вказує на exe, якого вже нема (теку перенесли): виправляємо.
    // Якщо старий exe на місці — чіпати не можна: це може бути інша, «основна» копія.
    if let Some(cur) = autostart::current() {
        let target = cur.trim_start().strip_prefix('"').and_then(|r| r.split('"').next()).unwrap_or("");
        if !target.is_empty() && !std::path::Path::new(target).exists() {
            autostart::enable(&autostart_cmd);
        }
    }

    HWND_PTR.store(hwnd, Ordering::SeqCst);
    TRAY.with(|c| *c.borrow_mut() = Some(Tray { hwnd, icon_on, icon_off, taskbar_created, autostart_cmd }));
    with_tray(|t| add_icon(t));
    if announce {
        with_tray(|t| {
            balloon(
                t,
                "layout-switcher працює",
                "Значок — біля годинника (якщо його не видно, натисни ^ і перетягни значок на панель). \
                 Права кнопка — меню: пауза, запис логу, автозапуск, оновлення, вихід.",
            )
        });
    }
    true
}

// ───────────────────────────── значок ─────────────────────────────

unsafe fn data(t: &Tray, flags: u32) -> NOTIFYICONDATAW {
    let mut d: NOTIFYICONDATAW = zeroed();
    d.cbSize = size_of::<NOTIFYICONDATAW>() as u32;
    d.hWnd = t.hwnd;
    d.uID = 1;
    d.uFlags = flags;
    d
}

fn copy_wide(dst: &mut [u16], text: &str) {
    let w: Vec<u16> = text.encode_utf16().collect();
    let n = w.len().min(dst.len() - 1);
    dst[..n].copy_from_slice(&w[..n]);
    dst[n] = 0;
}

fn tip() -> String {
    let state = if engine::enabled() { "працює" } else { "призупинено" };
    let log = if logger::log_enabled() { " · запис логу" } else { "" };
    format!("layout-switcher: {state}{log}")
}

fn current_icon(t: &Tray) -> HICON {
    if engine::enabled() {
        t.icon_on
    } else {
        t.icon_off
    }
}

unsafe fn add_icon(t: &Tray) {
    let mut d = data(t, NIF_MESSAGE | NIF_ICON | NIF_TIP);
    d.uCallbackMessage = WM_TRAY;
    d.hIcon = current_icon(t);
    copy_wide(&mut d.szTip, &tip());
    Shell_NotifyIconW(NIM_ADD, &d);
}

/// Оновити значок і підказку після зміни стану.
unsafe fn refresh(t: &Tray) {
    let mut d = data(t, NIF_ICON | NIF_TIP);
    d.hIcon = current_icon(t);
    copy_wide(&mut d.szTip, &tip());
    Shell_NotifyIconW(NIM_MODIFY, &d);
}

unsafe fn balloon(t: &Tray, title: &str, text: &str) {
    let mut d = data(t, NIF_INFO);
    copy_wide(&mut d.szInfoTitle, title);
    copy_wide(&mut d.szInfo, text);
    d.dwInfoFlags = NIIF_INFO;
    Shell_NotifyIconW(NIM_MODIFY, &d);
}

unsafe fn remove_icon(t: &Tray) {
    Shell_NotifyIconW(NIM_DELETE, &data(t, 0));
}

/// Показати повідомлення над значком. Можна викликати з будь-якого потоку.
pub(crate) fn notify(title: &str, text: &str) {
    push_notice(title, text, false);
}

/// Повідомлення про нову версію: клік по ньому запускає оновлення.
pub(crate) fn notify_update(title: &str, text: &str) {
    push_notice(title, text, true);
}

fn push_notice(title: &str, text: &str, update: bool) {
    NOTICES.lock().unwrap_or_else(|e| e.into_inner()).push((title.to_string(), text.to_string(), update));
    let h = HWND_PTR.load(Ordering::SeqCst);
    if !h.is_null() {
        unsafe { PostMessageW(h, WM_NOTICE, 0, 0) };
    }
}

/// Попросити програму завершитись (з будь-якого потоку): так нова версія після оновлення лишається сама.
pub(crate) fn request_exit() {
    let h = HWND_PTR.load(Ordering::SeqCst);
    if !h.is_null() {
        unsafe { PostMessageW(h, WM_COMMAND, CMD_EXIT, 0) };
    }
}

// ───────────────────────────── вікно ─────────────────────────────

unsafe extern "system" fn wnd_proc(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    let r = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| handle(hwnd, msg, wp, lp)));
    match r {
        Ok(Some(v)) => v,
        _ => DefWindowProcW(hwnd, msg, wp, lp),
    }
}

unsafe fn handle(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> Option<LRESULT> {
    match msg {
        WM_TRAY => {
            let ev = lp as u32;
            if ev == WM_RBUTTONUP || ev == WM_LBUTTONUP {
                show_menu(hwnd);
            } else if ev == NIN_BALLOONUSERCLICK {
                // клік по сповіщенню «вийшла нова версія» — те саме, що пункт меню «Оновити»
                if BALLOON_IS_UPDATE.swap(false, Ordering::SeqCst) {
                    updater::install();
                }
            } else if ev == NIN_BALLOONTIMEOUT || ev == NIN_BALLOONHIDE {
                BALLOON_IS_UPDATE.store(false, Ordering::SeqCst);
            }
            Some(0)
        }
        WM_COMMAND => {
            command(hwnd, wp & 0xFFFF);
            Some(0)
        }
        WM_NOTICE => {
            let items = std::mem::take(&mut *NOTICES.lock().unwrap_or_else(|e| e.into_inner()));
            with_tray(|t| {
                for (title, text, is_update) in &items {
                    balloon(t, title, text);
                    BALLOON_IS_UPDATE.store(*is_update, Ordering::SeqCst);
                }
            });
            Some(0)
        }
        WM_DESTROY => {
            with_tray(|t| remove_icon(t));
            HWND_PTR.store(null_mut(), Ordering::SeqCst);
            PostQuitMessage(0);
            Some(0)
        }
        WM_ENDSESSION => {
            with_tray(|t| remove_icon(t));
            None
        }
        m if with_tray(|t| t.taskbar_created == m && m != 0).unwrap_or(false) => {
            with_tray(|t| add_icon(t)); // Провідник перезапустився — значок зник
            Some(0)
        }
        _ => None,
    }
}

// ───────────────────────────── меню ─────────────────────────────

unsafe fn item(menu: *mut c_void, flags: u32, id: usize, text: &str) {
    AppendMenuW(menu, flags, id, win::wide(text).as_ptr());
}

fn checked(on: bool) -> u32 {
    if on {
        MF_STRING | MF_CHECKED
    } else {
        MF_STRING
    }
}

unsafe fn show_menu(hwnd: HWND) {
    let enabled = engine::enabled();
    let log_on = logger::log_enabled();
    let words = logger::words_enabled();

    let menu = CreatePopupMenu();
    item(menu, MF_STRING | MF_GRAYED | MF_DISABLED, 0, &format!("layout-switcher {VERSION}"));
    if let Some((version, full)) = updater::available() {
        let text = if full { format!("Вийшла версія {version} — відкрити сторінку…") } else { format!("Оновити до версії {version}…") };
        item(menu, MF_STRING, CMD_UPDATE_INSTALL, &text);
    }
    AppendMenuW(menu, MF_SEPARATOR, 0, null());
    item(menu, MF_STRING, CMD_TOGGLE, if enabled { "Призупинити автовиправлення" } else { "Відновити автовиправлення" });
    // Застосунок, у якому щойно набирали текст: вимкнути/ввімкнути саме в ньому.
    if let Some(exe) = appsoff::current() {
        match app_state(&exe) {
            AppState::Remote => item(menu, MF_STRING | MF_GRAYED | MF_DISABLED, 0, &format!("{exe}: віддалений доступ — не втручаюсь")),
            AppState::FileList => item(menu, MF_STRING | MF_CHECKED | MF_GRAYED | MF_DISABLED, 0, &format!("Не працювати в {exe} (apps-off.txt)")),
            AppState::Menu => item(menu, MF_STRING | MF_CHECKED, CMD_APP_OFF, &format!("Не працювати в {exe}")),
            AppState::On => item(menu, MF_STRING, CMD_APP_OFF, &format!("Не працювати в {exe}")),
        }
    }

    let log_menu = CreatePopupMenu();
    item(log_menu, checked(!log_on), CMD_LOG_OFF, "Вимкнено");
    item(log_menu, checked(log_on && !words), CMD_LOG_PLAIN, "Без слів (лише рішення й статистика)");
    item(log_menu, checked(log_on && words), CMD_LOG_WORDS, "Зі словами (для аналізу помилок)…");
    AppendMenuW(menu, MF_POPUP, log_menu as usize, win::wide("Запис логу").as_ptr());
    item(menu, MF_STRING, CMD_OPEN_LOG, "Показати теку з логом");
    item(menu, MF_STRING, CMD_OPEN_LEARNED, "Вивчені слова…");

    AppendMenuW(menu, MF_SEPARATOR, 0, null());
    let upd_menu = CreatePopupMenu();
    let busy = updater::is_busy();
    item(
        upd_menu,
        if busy { MF_STRING | MF_GRAYED | MF_DISABLED } else { MF_STRING },
        CMD_UPDATE_CHECK,
        if busy { "Зачекайте, іде операція…" } else { "Перевірити оновлення зараз" },
    );
    item(upd_menu, checked(settings::auto_check()), CMD_UPDATE_AUTO, "Перевіряти раз на добу (GitHub)");
    AppendMenuW(menu, MF_POPUP, upd_menu as usize, win::wide("Оновлення").as_ptr());
    item(menu, checked(autostart::current().is_some()), CMD_AUTOSTART, "Запускати разом з Windows");
    AppendMenuW(menu, MF_SEPARATOR, 0, null());
    item(menu, MF_STRING, CMD_EXIT, "Вийти");

    let mut pt: POINT = zeroed();
    GetCursorPos(&mut pt);
    // Без цього меню не закривається кліком повз нього (відома особливість Win32).
    SetForegroundWindow(hwnd);
    let cmd = TrackPopupMenu(menu, TPM_RIGHTBUTTON | TPM_BOTTOMALIGN | TPM_RETURNCMD, pt.x, pt.y, 0, hwnd, null());
    PostMessageW(hwnd, WM_NULL, 0, 0);
    DestroyMenu(menu); // разом із підменю
    if cmd > 0 {
        command(hwnd, cmd as usize);
    }
}

// ───────────────────────────── дії ─────────────────────────────

unsafe fn command(hwnd: HWND, id: usize) {
    match id {
        CMD_TOGGLE => set_enabled(!engine::enabled()),
        CMD_PAUSE => set_enabled(false),
        CMD_RESUME => set_enabled(true),
        CMD_LOG_OFF => {
            logger::disable();
            with_tray(|t| refresh(t));
        }
        CMD_LOG_PLAIN => enable_log(false),
        CMD_LOG_WORDS => {
            let ask = "У лог потраплятимуть набрані слова, зокрема можливі паролі (токени, схожі на пароль, і слова в \
                       терміналах, PuTTY, WinBox, менеджерах паролів не пишуться).\n\nНе набирай паролі, поки йде запис, \
                       а після аналізу видали layout-switcher.log.\n\nУвімкнути запис зі словами?";
            if win::message_box(ask, MB_OKCANCEL | MB_ICONWARNING) == 1 {
                enable_log(true);
            }
        }
        CMD_OPEN_LOG => open_log_folder(),
        CMD_OPEN_LEARNED => open_learned(),
        CMD_APP_OFF => toggle_app(),
        CMD_AUTOSTART => toggle_autostart(),
        CMD_UPDATE_CHECK => updater::check_now(true),
        CMD_UPDATE_INSTALL => updater::install(),
        CMD_UPDATE_AUTO => toggle_update_check(),
        CMD_EXIT => {
            DestroyWindow(hwnd);
        }
        _ => {}
    }
}

fn set_enabled(on: bool) {
    engine::set_enabled(on);
    with_tray(|t| unsafe { refresh(t) });
}

fn enable_log(words: bool) {
    match logger::enable(words, "tray") {
        Some(path) => {
            with_tray(|t| unsafe {
                refresh(t);
                balloon(t, "Запис логу увімкнено", &format!("{}", path.display()));
            });
        }
        None => {
            win::message_box(
                "Не вдалося створити layout-switcher.log: ні поруч з програмою, ні в %LOCALAPPDATA%\\layout-switcher.",
                MB_ICONERROR,
            );
        }
    }
}

unsafe fn open_log_folder() {
    let verb = win::wide("open");
    match logger::path().filter(|p| p.is_file()) {
        Some(path) => {
            // Провідник із виділеним файлом
            let args = win::wide(&format!("/select,\"{}\"", path.display()));
            ShellExecuteW(null_mut(), verb.as_ptr(), win::wide("explorer.exe").as_ptr(), args.as_ptr(), null(), 1);
        }
        None => {
            with_tray(|t| balloon(t, "Логу ще нема", "Увімкни запис логу в меню — файл з'явиться поруч з програмою."));
        }
    }
}

enum AppState {
    On,
    Menu,
    FileList,
    Remote,
}

fn app_state(exe: &str) -> AppState {
    let mut rules = beside_exe_rules();
    if let AppMode::Off(OffReason::UserList) = rules.mode(exe) {
        return AppState::FileList;
    }
    rules.set_menu_list(&appsoff::read());
    match rules.mode(exe) {
        AppMode::Off(OffReason::Menu) => AppState::Menu,
        AppMode::Off(_) => AppState::Remote,
        AppMode::Normal => AppState::On,
    }
}

/// Правила з `apps-off.txt` поруч із програмою (лише щоб показати в меню, звідки вимкнено).
fn beside_exe_rules() -> AppRules {
    std::env::current_exe()
        .ok()
        .and_then(|p| std::fs::read_to_string(p.with_file_name("apps-off.txt")).ok())
        .map(|t| AppRules::from_user_list(&t))
        .unwrap_or_default()
}

unsafe fn toggle_app() {
    let Some(exe) = appsoff::current() else { return };
    let (title, text) = match appsoff::toggle(&exe) {
        Ok(true) => (format!("Вимкнено в {exe}"), "Тут більше нічого не виправляю. Увімкнути — тим самим пунктом меню.".to_string()),
        Ok(false) => (format!("Знову працюю в {exe}"), "Автовиправлення в цьому застосунку ввімкнено.".to_string()),
        Err(e) => ("Не вдалося змінити список".to_string(), format!("%APPDATA%\\layout-switcher\\apps-off.txt: {e}")),
    };
    crate::logger::log_event("app_off", vec![("exe", exe.clone()), ("result", title.clone())]);
    with_tray(|t| balloon(t, &title, &text));
}

/// Провідник із виділеним файлом вивчених слів (файли створюються, якщо їх ще нема).
unsafe fn open_learned() {
    match crate::learned::ensure_files() {
        Some(path) => {
            let args = win::wide(&format!("/select,\"{}\"", path.display()));
            ShellExecuteW(null_mut(), win::wide("open").as_ptr(), win::wide("explorer.exe").as_ptr(), args.as_ptr(), null(), 1);
        }
        None => {
            with_tray(|t| balloon(t, "Не вдалося створити файл", "Не можу записати в %APPDATA%\\layout-switcher."));
        }
    }
}

fn toggle_autostart() {
    let Some(cmd) = with_tray(|t| t.autostart_cmd.clone()) else { return };
    let (ok, now_on) = if autostart::current().is_some() { (autostart::disable(), false) } else { (autostart::enable(&cmd), true) };
    with_tray(|t| unsafe {
        match (ok, now_on) {
            (true, true) => balloon(t, "Автозапуск увімкнено", "Програма стартуватиме разом з Windows (лише для цього користувача)."),
            (true, false) => balloon(t, "Автозапуск вимкнено", "Програма більше не стартує разом з Windows."),
            (false, _) => balloon(t, "Не вдалося змінити автозапуск", "Windows не дозволила записати в реєстр."),
        }
    });
}

fn toggle_update_check() {
    let now_on = !settings::auto_check();
    let ok = settings::set_auto_check(now_on);
    with_tray(|t| unsafe {
        match (ok, now_on) {
            (true, true) => balloon(t, "Перевірка оновлень увімкнена", "Раз на добу програма питає GitHub, чи є нова версія. Ставиться лише за вашим кліком."),
            (true, false) => balloon(t, "Перевірка оновлень вимкнена", "Програма нічого не надсилає в мережу. Перевірити вручну: меню → Оновлення."),
            (false, _) => balloon(t, "Не вдалося зберегти налаштування", "Windows не дозволила записати в реєстр."),
        }
    });
}

// ───────────────────────────── керування ззовні ─────────────────────────────

/// Надіслати команду вже запущеному екземпляру. `false` — програма не запущена.
pub(crate) unsafe fn control(c: Control) -> bool {
    let hwnd = FindWindowW(win::wide(CLASS).as_ptr(), null());
    if hwnd.is_null() {
        return false;
    }
    let id = match c {
        Control::Exit => CMD_EXIT,
        Control::Pause => CMD_PAUSE,
        Control::Resume => CMD_RESUME,
    };
    PostMessageW(hwnd, WM_COMMAND, id, 0) != 0
}

/// Рядок запуску для автозапуску з поточних параметрів.
pub(crate) fn autostart_command(args: &[String]) -> String {
    let exe = std::env::current_exe().map(|p| p.display().to_string()).unwrap_or_default();
    quote_command(&exe, args)
}
