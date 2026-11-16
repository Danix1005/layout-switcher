//! Виправлення виділеного фрагмента.
//!
//! Ми не бачимо виділення в чужому застосунку, тому діємо через буфер обміну:
//!   1. зберігаємо поточний вміст буфера;
//!   2. надсилаємо Ctrl+Insert (копіювати). Якщо буфер не змінився — виділення нема;
//!   3. конвертуємо текст «по клавішах», кладемо результат у буфер і надсилаємо Shift+Insert (вставити);
//!   4. повертаємо початковий вміст буфера, перемикаємо розкладку.
//!
//! Ctrl+Insert / Shift+Insert, а не Ctrl+C / Ctrl+V — свідомо: у терміналах Ctrl+C це SIGINT і він
//! перервав би запущену програму, а Ctrl+Insert завжди лише копіює.
//!
//! Усе виконується у фоновому потоці: потрібно чекати відповіді застосунку, а блокувати
//! хук клавіатури (ліміт ~300 мс) не можна.

use std::ptr::null_mut;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread::sleep;
use std::time::Duration;

use layout_switcher::apps::is_sensitive;
use layout_switcher::keymap::{convert_text, Lang};
use layout_switcher::privacy::redact;

use windows_sys::Win32::System::DataExchange::{
    CloseClipboard, EmptyClipboard, EnumClipboardFormats, GetClipboardData,
    GetClipboardSequenceNumber, OpenClipboard, RegisterClipboardFormatW, SetClipboardData,
};
use windows_sys::Win32::Foundation::GlobalFree;
use windows_sys::Win32::System::Memory::{
    GlobalAlloc, GlobalLock, GlobalSize, GlobalUnlock, GMEM_MOVEABLE,
};
use windows_sys::Win32::UI::Input::KeyboardAndMouse::{
    MapVirtualKeyW, SendInput, INPUT, KEYEVENTF_EXTENDEDKEY, KEYEVENTF_KEYUP, VK_CONTROL,
    VK_INSERT, VK_SHIFT,
};

use crate::logger::{log_event, say, with_log};
use crate::win::{down, key_input, request_layout};

const CF_UNICODETEXT: u32 = 13;
/// Не переносимо в логіку «слово»: довший фрагмент — це вже не виправлення розкладки.
const MAX_CHARS: usize = 5000;
/// Скільки лишається в логу від виділеного тексту (із `--log-words`).
const LOG_CHARS: usize = 300;
/// Ліміт резервної копії буфера обміну (великі зображення тощо).
const MAX_BACKUP: usize = 64 * 1024 * 1024;
/// Скільки чекаємо, поки застосунок вставить текст, перш ніж повертати старий буфер.
const PASTE_WAIT: Duration = Duration::from_millis(350);

static BUSY: AtomicBool = AtomicBool::new(false);

pub struct SelReq {
    pub src: &'static str,
    pub exe: String,
    /// Поточна розкладка вікна — підказка, якщо в тексті порівну літер обох мов.
    pub hint: Option<Lang>,
    pub hkl_en: isize,
    pub hkl_uk: isize,
}

/// Запустити виправлення виділеного у фоні. Якщо попереднє ще триває — ігноруємо.
pub fn spawn(req: SelReq) {
    if BUSY.swap(true, Ordering::SeqCst) {
        return;
    }
    std::thread::spawn(move || {
        let r = std::panic::catch_unwind(|| unsafe { run(&req) });
        if r.is_err() {
            noop(&req, "panic");
        }
        BUSY.store(false, Ordering::SeqCst);
    });
}

fn noop(req: &SelReq, reason: &str) {
    log_event(
        "manual_noop",
        vec![
            ("src", req.src.to_string()),
            ("kind", "selection".into()),
            ("exe", req.exe.clone()),
            ("reason", reason.to_string()),
        ],
    );
}

unsafe fn run(req: &SelReq) {
    // 0) чекаємо, поки Shift (подвійне натискання) справді відпущено: інакше Ctrl+Insert стане Ctrl+Shift+Insert.
    for _ in 0..30 {
        if !down(VK_SHIFT) && !down(0xA0) && !down(0xA1) {
            break;
        }
        sleep(Duration::from_millis(10));
    }

    // 1) резервна копія буфера: без неї нічого не чіпаємо
    let Some(backup) = backup() else {
        noop(req, "clipboard_unavailable");
        return;
    };

    // 2) копіюємо виділене
    let seq0 = GetClipboardSequenceNumber();
    send_chord(VK_CONTROL, VK_INSERT);
    let mut changed = false;
    for _ in 0..40 {
        sleep(Duration::from_millis(10));
        if GetClipboardSequenceNumber() != seq0 {
            changed = true;
            break;
        }
    }
    if !changed {
        noop(req, "no_selection"); // буфер не змінився: нічого не виділено (або застосунок ігнорує Ctrl+Insert)
        return;
    }
    sleep(Duration::from_millis(20)); // даємо застосунку дописати всі формати

    let Some(text) = get_text() else {
        restore(&backup);
        noop(req, "no_text");
        return;
    };
    if text.is_empty() {
        restore(&backup);
        noop(req, "empty");
        return;
    }
    // Редактори на кшталт VS Code при Ctrl+Insert без виділення копіюють поточний рядок (з переносом у кінці):
    // це не виділення — вставка дублювала б рядок.
    if text.ends_with('\n') || text.ends_with('\r') {
        restore(&backup);
        noop(req, "ambiguous_line");
        return;
    }
    let n_chars = text.chars().count();
    if n_chars > MAX_CHARS {
        restore(&backup);
        noop(req, "too_long");
        return;
    }
    let Some(conv) = convert_text(&text, req.hint) else {
        restore(&backup);
        noop(req, "no_letters");
        return;
    };
    if conv.text == text {
        restore(&backup);
        noop(req, "unchanged");
        return;
    }

    // 3) вставляємо результат
    if !set_text_private(&conv.text) {
        restore(&backup);
        noop(req, "set_failed");
        return;
    }
    send_chord(VK_SHIFT, VK_INSERT);
    sleep(PASTE_WAIT);

    // 4) повертаємо буфер і розкладку
    restore(&backup);
    let to = conv.from.other();
    let hkl = match to {
        Lang::En => req.hkl_en,
        Lang::Uk => req.hkl_uk,
    };
    request_layout(hkl);
    say(format!("-> {to:?}: виправлено виділене ({n_chars} симв.)"));

    let masked = is_sensitive(&req.exe);
    with_log(|lg| {
        let mut f: Vec<(&str, String)> = vec![
            ("src", req.src.to_string()),
            ("kind", "selection".into()),
            ("exe", req.exe.clone()),
            ("from", format!("{:?}", conv.from)),
            ("to", format!("{:?}", to)),
            ("chars", n_chars.to_string()),
        ];
        if lg.words && masked {
            f.push(("masked", "1".into()));
        } else if lg.words {
            // слова, схожі на пароль, у лог не потрапляють (privacy.rs)
            f.push(("text", redact(&text).chars().take(LOG_CHARS).collect()));
            f.push(("result", redact(&conv.text).chars().take(LOG_CHARS).collect()));
        }
        lg.line("manual", &f);
    });
}

// ───────────────────────────── введення ─────────────────────────────

unsafe fn scan(vk: u16) -> u16 {
    MapVirtualKeyW(vk as u32, 0) as u16
}

/// Modifier+Insert (Insert — розширена клавіша).
unsafe fn send_chord(modifier: u16, key: u16) {
    let v: Vec<INPUT> = vec![
        key_input(modifier, scan(modifier), 0),
        key_input(key, scan(key), KEYEVENTF_EXTENDEDKEY),
        key_input(key, scan(key), KEYEVENTF_EXTENDEDKEY | KEYEVENTF_KEYUP),
        key_input(modifier, scan(modifier), KEYEVENTF_KEYUP),
    ];
    SendInput(v.len() as u32, v.as_ptr(), std::mem::size_of::<INPUT>() as i32);
}

// ───────────────────────────── буфер обміну ─────────────────────────────

type Backup = Vec<(u32, Vec<u8>)>;

/// Буфер обміну одночасно може тримати лише одна програма; менеджери буфера й браузери займають його
/// на сотні мілісекунд (у лозі була відмова `set_failed`), тож пробуємо до пів секунди.
unsafe fn open_clipboard() -> bool {
    for _ in 0..50 {
        if OpenClipboard(null_mut()) != 0 {
            return true;
        }
        sleep(Duration::from_millis(10));
    }
    false
}

/// Формати, які не є простою пам'яттю (GDI-об'єкти, власні формати відображення) — їх не копіюємо.
fn skip_format(fmt: u32) -> bool {
    matches!(fmt, 2 | 3 | 9 | 14 | 0x80..=0x8E | 0x200..=0x3FF)
}

unsafe fn backup() -> Option<Backup> {
    if !open_clipboard() {
        return None;
    }
    let mut out: Backup = Vec::new();
    let mut total = 0usize;
    let mut fmt = 0u32;
    loop {
        fmt = EnumClipboardFormats(fmt);
        if fmt == 0 {
            break;
        }
        if skip_format(fmt) {
            continue;
        }
        let h = GetClipboardData(fmt);
        if h.is_null() {
            continue;
        }
        let size = GlobalSize(h);
        if size == 0 {
            continue;
        }
        total += size;
        if total > MAX_BACKUP {
            CloseClipboard();
            return None;
        }
        let p = GlobalLock(h);
        if p.is_null() {
            continue;
        }
        out.push((fmt, std::slice::from_raw_parts(p as *const u8, size).to_vec()));
        GlobalUnlock(h);
    }
    CloseClipboard();
    Some(out)
}

unsafe fn alloc_bytes(bytes: &[u8]) -> *mut core::ffi::c_void {
    let h = GlobalAlloc(GMEM_MOVEABLE, bytes.len().max(1));
    if h.is_null() {
        return null_mut();
    }
    let p = GlobalLock(h);
    if p.is_null() {
        GlobalFree(h);
        return null_mut();
    }
    std::ptr::copy_nonoverlapping(bytes.as_ptr(), p as *mut u8, bytes.len());
    GlobalUnlock(h);
    h
}

unsafe fn restore(b: &Backup) {
    if !open_clipboard() {
        return;
    }
    EmptyClipboard();
    for (fmt, bytes) in b {
        let h = alloc_bytes(bytes);
        if h.is_null() {
            continue;
        }
        if SetClipboardData(*fmt, h).is_null() {
            GlobalFree(h);
        }
    }
    CloseClipboard();
}

unsafe fn get_text() -> Option<String> {
    if !open_clipboard() {
        return None;
    }
    let mut result = None;
    let h = GetClipboardData(CF_UNICODETEXT);
    if !h.is_null() {
        let p = GlobalLock(h) as *const u16;
        if !p.is_null() {
            let max = GlobalSize(h) / 2;
            let mut len = 0usize;
            while len < max && *p.add(len) != 0 {
                len += 1;
            }
            result = Some(String::from_utf16_lossy(std::slice::from_raw_parts(p, len)));
            GlobalUnlock(h);
        }
    }
    CloseClipboard();
    result
}

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

/// Кладемо текст у буфер так, щоб він не потрапив в історію буфера Windows (Win+V) і хмарну синхронізацію.
unsafe fn set_text_private(text: &str) -> bool {
    if !open_clipboard() {
        return false;
    }
    EmptyClipboard();
    let units: Vec<u16> = text.encode_utf16().chain(std::iter::once(0)).collect();
    let bytes: Vec<u8> = units.iter().flat_map(|u| u.to_le_bytes()).collect();
    let h = alloc_bytes(&bytes);
    let ok = !h.is_null() && !SetClipboardData(CF_UNICODETEXT, h).is_null();
    if !ok && !h.is_null() {
        GlobalFree(h);
    }
    // Службові формати-маркери Windows: не заносити в історію / хмару / монітори буфера.
    for name in [
        "ExcludeClipboardContentFromMonitorProcessing",
        "CanIncludeInClipboardHistory",
        "CanUploadToCloudClipboard",
    ] {
        let fmt = RegisterClipboardFormatW(wide(name).as_ptr());
        if fmt != 0 {
            let flag = alloc_bytes(&0u32.to_le_bytes());
            if !flag.is_null() && SetClipboardData(fmt, flag).is_null() {
                GlobalFree(flag);
            }
        }
    }
    CloseClipboard();
    ok
}
