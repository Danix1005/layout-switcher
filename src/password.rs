//! Чи стоїть курсор у полі пароля. У такому полі нічого не виправляємо (пароль, схожий на слово в іншій
//! розкладці, інакше «виправився» б і вхід зламався), нічого не вчимо й не пишемо слова в лог.
//!
//! Як дізнаємось:
//! - звичайне поле Windows (WinBox, діалоги входу): стиль `ES_PASSWORD` у вікна з фокусом — миттєво;
//! - браузери, Electron, WPF, UWP: UI Automation (`IsPassword` у елемента з фокусом).
//!
//! UI Automation питає інший процес і може відповідати десятки мілісекунд, а в зависаючої програми — довше.
//! У потоці хука клавіатури так чекати не можна, тому питає окремий потік. Запит кладуть у чергу: зміна фокусу
//! (WinEvent `EVENT_OBJECT_FOCUS` / `EVENT_SYSTEM_FOREGROUND` — відповідь готова ще до першої літери), а також
//! потік хука на початку кожного слова. Поки відповіді нема — поводимось обережно (вважаємо, що це може бути
//! пароль: не виправляємо, слів у лог не пишемо), але не довше `STALE_MS`.

use std::sync::atomic::{AtomicBool, AtomicIsize, AtomicU32, AtomicU64, Ordering};
use std::sync::mpsc::{channel, Sender};
use std::sync::OnceLock;
use std::time::Instant;

use windows::Win32::System::Com::{CoCreateInstance, CoInitializeEx, CLSCTX_INPROC_SERVER, COINIT_MULTITHREADED};
use windows::Win32::UI::Accessibility::{CUIAutomation, IUIAutomation};
use windows_sys::Win32::Foundation::HWND;
use windows_sys::Win32::UI::Accessibility::{SetWinEventHook, HWINEVENTHOOK};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    DispatchMessageW, GetClassNameW, GetForegroundWindow, GetGUIThreadInfo, GetMessageW, GetWindowLongW,
    GetWindowThreadProcessId, EVENT_OBJECT_FOCUS, EVENT_SYSTEM_FOREGROUND, GUITHREADINFO, GWL_STYLE, MSG,
    WINEVENT_OUTOFCONTEXT, WINEVENT_SKIPOWNPROCESS,
};

use crate::logger::log_event;

/// Стиль поля Windows «пароль» (`ES_PASSWORD`).
const ES_PASSWORD: u32 = 0x20;
/// Якщо відповідь не прийшла за стільки — UI Automation, мабуть, завис; далі не чекаємо. Перший запит до
/// Chrome/Opera буває повільним (браузер саме вмикає доступність): на логах — понад 1,5 с.
const STALE_MS: u64 = 3000;
/// Не питати про те саме вікно частіше.
const DEBOUNCE_MS: u64 = 150;

static TX: OnceLock<Sender<(isize, u32)>> = OnceLock::new();
static EPOCH: OnceLock<Instant> = OnceLock::new();
static SENT: AtomicU32 = AtomicU32::new(0);
static DONE: AtomicU32 = AtomicU32::new(0);
static IS_PASSWORD: AtomicBool = AtomicBool::new(false);
static FOR_FG: AtomicIsize = AtomicIsize::new(0);
/// Останній запит: для якого вікна й коли (мс від запуску).
static LAST_FG: AtomicIsize = AtomicIsize::new(0);
static LAST_AT: AtomicU64 = AtomicU64::new(0);
/// Відколи чекаємо відповіді (мс від запуску; 0 — не чекаємо).
static PENDING_SINCE: AtomicU64 = AtomicU64::new(0);

fn now_ms() -> u64 {
    EPOCH.get_or_init(Instant::now).elapsed().as_millis() as u64 + 1
}

/// Запустити потік, що відповідає на запити, і потік, що стежить за фокусом.
pub(crate) fn start() {
    let (tx, rx) = channel::<(isize, u32)>();
    if TX.set(tx).is_err() {
        return;
    }
    now_ms();
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
            let t0 = now_ms();
            let (pw, via) = match win32_password(fg as HWND) {
                true => (true, "style"),
                false => (uia.as_ref().is_some_and(uia_password), "uia"),
            };
            IS_PASSWORD.store(pw, Ordering::Release);
            FOR_FG.store(fg, Ordering::Release);
            DONE.store(seq, Ordering::Release);
            if DONE.load(Ordering::Acquire) == SENT.load(Ordering::Acquire) {
                PENDING_SINCE.store(0, Ordering::Release);
            }
            if pw && last != Some((fg, true)) {
                let exe = unsafe { crate::win::exe_of(fg as HWND) };
                let ms = (now_ms() - t0).to_string();
                log_event("password_field", vec![("exe", exe), ("via", via.into()), ("ms", ms)]);
            }
            last = Some((fg, pw));
        }
    });
    // Фокус перейшов (інше вікно, інше поле): питаємо одразу, щоб відповідь була готова до першої літери.
    std::thread::spawn(|| unsafe {
        let flags = WINEVENT_OUTOFCONTEXT | WINEVENT_SKIPOWNPROCESS;
        SetWinEventHook(EVENT_SYSTEM_FOREGROUND, EVENT_SYSTEM_FOREGROUND, std::ptr::null_mut(), Some(on_focus), 0, 0, flags);
        SetWinEventHook(EVENT_OBJECT_FOCUS, EVENT_OBJECT_FOCUS, std::ptr::null_mut(), Some(on_focus), 0, 0, flags);
        let mut msg: MSG = std::mem::zeroed();
        while GetMessageW(&mut msg, std::ptr::null_mut(), 0, 0) > 0 {
            DispatchMessageW(&msg);
        }
    });
}

unsafe extern "system" fn on_focus(_: HWINEVENTHOOK, _event: u32, _: HWND, _: i32, _: i32, _: u32, _: u32) {
    let fg = GetForegroundWindow();
    if !fg.is_null() {
        ask(fg, false);
    }
}

/// Почалось нове слово, змінилось вікно чи поле — перевірити. Для того самого вікна не частіше, ніж раз на
/// `DEBOUNCE_MS` (крім `force`). Можна викликати з будь-якого потоку: лише кладе запит у чергу.
pub(crate) fn ask(fg: HWND, force: bool) {
    let Some(tx) = TX.get() else { return };
    let fg = fg as isize;
    let now = now_ms();
    let recent = LAST_FG.load(Ordering::Acquire) == fg && now.saturating_sub(LAST_AT.load(Ordering::Acquire)) < DEBOUNCE_MS;
    if recent && !force {
        return;
    }
    LAST_FG.store(fg, Ordering::Release);
    LAST_AT.store(now, Ordering::Release);
    let _ = PENDING_SINCE.compare_exchange(0, now, Ordering::AcqRel, Ordering::Acquire);
    let seq = SENT.fetch_add(1, Ordering::AcqRel) + 1;
    let _ = tx.send((fg, seq));
}

fn pending() -> bool {
    DONE.load(Ordering::Acquire) != SENT.load(Ordering::Acquire)
}

/// Відповіді чекаємо вже надто довго — UI Automation, мабуть, завис.
fn stuck() -> bool {
    let since = PENDING_SINCE.load(Ordering::Acquire);
    since != 0 && now_ms().saturating_sub(since) > STALE_MS
}

/// Чи можна зараз виправляти текст у вікні `fg`. `false` — поле пароля або відповідь ще не прийшла.
pub(crate) fn safe_to_fix(fg: HWND) -> bool {
    if TX.get().is_none() {
        return true; // перевірку не запущено (наприклад, у тестах)
    }
    if pending() {
        return stuck(); // UI Automation завис — не блокуємо через нього все виправлення
    }
    !is_password(fg)
}

/// Для логу слів: точно не поле пароля (відповідь прийшла й вона «ні»). Поки чекаємо відповіді — слова не пишемо.
pub(crate) fn known_not_password(fg: HWND) -> bool {
    TX.get().is_none() || (!pending() && !is_password(fg)) || (pending() && stuck() && !is_password(fg))
}

/// Для логу: чому не виправили — поле пароля (`field`) чи відповідь ще не прийшла (`pending`).
pub(crate) fn why(fg: HWND) -> &'static str {
    if is_password(fg) && !pending() {
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
