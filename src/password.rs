//! Чи стоїть курсор у полі пароля. У такому полі нічого не виправляємо (пароль, схожий на слово в іншій
//! розкладці, інакше «виправився» б і вхід зламався), нічого не вчимо й не пишемо слова в лог.
//!
//! Як дізнаємось:
//! - звичайне поле Windows (WinBox, діалоги входу): стиль `ES_PASSWORD` у вікна з фокусом — миттєво;
//! - браузери, Electron, WPF, UWP: UI Automation (`IsPassword` у елемента з фокусом).
//!
//! UI Automation питає інший процес і може відповідати десятки мілісекунд, а в зависаючої програми — довше.
//! У потоці хука клавіатури так чекати не можна, тому питає окремий потік: потік хука на початку кожного слова
//! (і при зміні вікна) лише кладе запит у чергу, а до кінця слова відповідь уже готова.
//! Поки відповіді нема — поводимось обережно (вважаємо, що це може бути пароль), але не довше `STALE`.

use std::sync::atomic::{AtomicBool, AtomicIsize, AtomicU32, Ordering};
use std::sync::mpsc::{channel, Sender};
use std::sync::OnceLock;
use std::time::{Duration, Instant};

use windows::Win32::System::Com::{CoCreateInstance, CoInitializeEx, CLSCTX_INPROC_SERVER, COINIT_MULTITHREADED};
use windows::Win32::UI::Accessibility::{CUIAutomation, IUIAutomation};
use windows_sys::Win32::Foundation::HWND;
use windows_sys::Win32::UI::WindowsAndMessaging::{
    GetClassNameW, GetGUIThreadInfo, GetWindowLongW, GetWindowThreadProcessId, GUITHREADINFO, GWL_STYLE,
};

use crate::logger::log_event;

/// Стиль поля Windows «пароль» (`ES_PASSWORD`).
const ES_PASSWORD: u32 = 0x20;
/// Якщо відповідь не прийшла за стільки — UI Automation, мабуть, завис; далі не чекаємо.
const STALE: Duration = Duration::from_millis(1500);

static TX: OnceLock<Sender<(isize, u32)>> = OnceLock::new();
static SENT: AtomicU32 = AtomicU32::new(0);
static DONE: AtomicU32 = AtomicU32::new(0);
static IS_PASSWORD: AtomicBool = AtomicBool::new(false);
static FOR_FG: AtomicIsize = AtomicIsize::new(0);

thread_local! {
    /// Потік хука: коли востаннє питали (щоб не смикати UI Automation на кожну літеру).
    static LAST_ASK: std::cell::Cell<Option<(isize, Instant)>> = const { std::cell::Cell::new(None) };
}

/// Запустити потік, що відповідає на запити.
pub(crate) fn start() {
    let (tx, rx) = channel::<(isize, u32)>();
    if TX.set(tx).is_err() {
        return;
    }
    std::thread::spawn(move || {
        let uia: Option<IUIAutomation> = unsafe {
            let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
            CoCreateInstance(&CUIAutomation, None, CLSCTX_INPROC_SERVER).ok()
        };
        if uia.is_none() {
            log_event("password_check", vec![("uia", "unavailable".into())]);
        }
        let mut last: Option<(isize, bool)> = None;
        while let Ok(mut req) = rx.recv() {
            while let Ok(newer) = rx.try_recv() {
                req = newer; // між запитами набралось кілька — відповідаємо лише на останній
            }
            let (fg, seq) = req;
            let (pw, via) = match win32_password(fg as HWND) {
                true => (true, "style"),
                false => (uia.as_ref().is_some_and(uia_password), "uia"),
            };
            IS_PASSWORD.store(pw, Ordering::Release);
            FOR_FG.store(fg, Ordering::Release);
            DONE.store(seq, Ordering::Release);
            if pw && last != Some((fg, true)) {
                let exe = unsafe { crate::win::exe_of(fg as HWND) };
                log_event("password_field", vec![("exe", exe), ("via", via.into())]);
            }
            last = Some((fg, pw));
        }
    });
}

/// Потік хука: почалось нове слово або змінилось вікно — перевірити поле. Не частіше, ніж раз на 200 мс
/// для того самого вікна (крім `force`).
pub(crate) fn ask(fg: HWND, force: bool) {
    let Some(tx) = TX.get() else { return };
    let fg = fg as isize;
    let now = Instant::now();
    let recent = LAST_ASK.with(|c| c.get()).is_some_and(|(f, t)| f == fg && now.duration_since(t) < Duration::from_millis(200));
    if recent && !force {
        return;
    }
    LAST_ASK.with(|c| c.set(Some((fg, now))));
    let seq = SENT.fetch_add(1, Ordering::AcqRel) + 1;
    let _ = tx.send((fg, seq));
}

/// Чи можна зараз виправляти текст у вікні `fg`. `false` — поле пароля або відповідь ще не прийшла.
pub(crate) fn safe_to_fix(fg: HWND) -> bool {
    if TX.get().is_none() {
        return true; // перевірку не запущено (наприклад, у тестах)
    }
    let pending = DONE.load(Ordering::Acquire) != SENT.load(Ordering::Acquire);
    if pending {
        let asked_long_ago = LAST_ASK.with(|c| c.get()).is_some_and(|(_, t)| t.elapsed() > STALE);
        return asked_long_ago; // відповіді нема надто довго — UI Automation завис; не блокуємо все через нього
    }
    !(FOR_FG.load(Ordering::Acquire) == fg as isize && IS_PASSWORD.load(Ordering::Acquire))
}

/// Для логу: чому не виправили — поле пароля (`field`) чи відповідь ще не прийшла (`pending`).
pub(crate) fn why(fg: HWND) -> &'static str {
    if is_password(fg) && DONE.load(Ordering::Acquire) == SENT.load(Ordering::Acquire) {
        "field"
    } else {
        "pending"
    }
}

/// Чи стоїть курсор у полі пароля (за останньою відповіддю) — для логу й навчання.
pub(crate) fn is_password(fg: HWND) -> bool {
    FOR_FG.load(Ordering::Acquire) == fg as isize && IS_PASSWORD.load(Ordering::Acquire)
}

/// Звичайне поле Windows зі стилем `ES_PASSWORD`.
fn win32_password(fg: HWND) -> bool {
    unsafe {
        let tid = GetWindowThreadProcessId(fg, std::ptr::null_mut());
        let mut gti: GUITHREADINFO = std::mem::zeroed();
        gti.cbSize = std::mem::size_of::<GUITHREADINFO>() as u32;
        if tid == 0 || GetGUIThreadInfo(tid, &mut gti) == 0 || gti.hwndFocus.is_null() {
            return false;
        }
        let mut buf = [0u16; 64];
        let n = GetClassNameW(gti.hwndFocus, buf.as_mut_ptr(), buf.len() as i32).max(0) as usize;
        let class = String::from_utf16_lossy(&buf[..n]).to_lowercase();
        class.contains("edit") && (GetWindowLongW(gti.hwndFocus, GWL_STYLE) as u32 & ES_PASSWORD) != 0
    }
}

/// UI Automation: елемент із фокусом — поле пароля?
fn uia_password(uia: &IUIAutomation) -> bool {
    unsafe {
        uia.GetFocusedElement()
            .and_then(|el| el.CurrentIsPassword())
            .map(|b| b.as_bool())
            .unwrap_or(false)
    }
}
