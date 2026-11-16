//! Діагностичний лог (`layout-switcher.log`) і вивід у консоль.
//!
//! Лог можна вмикати й вимикати під час роботи (меню значка в треї): файл дописується, а в початок кожного
//! сеансу запису йде подія `start` з тими самими даними про словники й розкладки.
//!
//! Хук клавіатури не має права зависати: якщо його обробник не відповість вчасно, Windows мовчки зніме хук,
//! і перемикач перестане працювати до перезапуску. А запис у консоль може заблокуватись надовго — досить
//! клацнути у вікні консолі (режим виділення). Тому з потоку хука в консоль пишемо лише через `say`: рядок іде
//! в чергу, а друкує його окремий потік.

use std::fs::{File, OpenOptions};
use std::io::Write;
use std::path::PathBuf;
use std::sync::mpsc::{channel, Sender};
use std::sync::{Mutex, OnceLock};
use std::time::{SystemTime, UNIX_EPOCH};

pub(crate) struct Logger {
    file: File,
    /// Писати набрані слова (`--log-words`).
    pub(crate) words: bool,
    /// Лог на паузі (Scroll Lock).
    pub(crate) paused: bool,
}

impl Logger {
    /// Один рядок: `час_мс<TAB>подія<TAB>ключ=значення…`
    pub(crate) fn line(&mut self, event: &str, fields: &[(&str, String)]) {
        if self.paused {
            return;
        }
        let ms = SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_millis()).unwrap_or(0);
        let mut s = format!("{ms}\t{event}");
        for (k, v) in fields {
            s.push('\t');
            s.push_str(k);
            s.push('=');
            s.extend(v.chars().map(|c| if matches!(c, '\t' | '\n' | '\r') { ' ' } else { c }));
        }
        s.push('\n');
        let _ = self.file.write_all(s.as_bytes());
    }
}

/// Лог спільний для потоків: виділений фрагмент обробляється у фоновому потоці.
static LOG: Mutex<Option<Logger>> = Mutex::new(None);
/// Шлях до файлу логу (відомий і тоді, коли запис уже вимкнено: меню «Показати теку з логом»).
static PATH: Mutex<Option<PathBuf>> = Mutex::new(None);
/// Дані для події `start`: словники, розкладки, режими. Заповнює `main` при запуску.
static START_INFO: Mutex<Vec<(&'static str, String)>> = Mutex::new(Vec::new());

pub(crate) fn with_log<R>(f: impl FnOnce(&mut Logger) -> R) -> Option<R> {
    let mut g = LOG.lock().unwrap_or_else(|e| e.into_inner());
    g.as_mut().map(f)
}

pub(crate) fn log_enabled() -> bool {
    with_log(|_| ()).is_some()
}

pub(crate) fn log_event(event: &str, fields: Vec<(&str, String)>) {
    with_log(|l| l.line(event, &fields));
}

/// Відкрити лог поруч з exe (або в поточній теці). Повертає шлях, якщо вдалося.
pub(crate) fn open(words: bool) -> Option<PathBuf> {
    let mut candidates = Vec::new();
    if let Ok(exe) = std::env::current_exe() {
        if let Some(dir) = exe.parent() {
            candidates.push(dir.join("layout-switcher.log"));
        }
    }
    // Якщо поруч з exe писати не можна (Program Files) — у профіль користувача.
    if let Some(dir) = std::env::var_os("LOCALAPPDATA") {
        let dir = PathBuf::from(dir).join("layout-switcher");
        if std::fs::create_dir_all(&dir).is_ok() {
            candidates.push(dir.join("layout-switcher.log"));
        }
    }
    candidates.push(PathBuf::from("layout-switcher.log"));
    let (file, path) = candidates
        .into_iter()
        .find_map(|p| OpenOptions::new().create(true).append(true).open(&p).ok().map(|f| (f, p)))?;
    *LOG.lock().unwrap_or_else(|e| e.into_inner()) = Some(Logger { file, words, paused: false });
    *PATH.lock().unwrap_or_else(|e| e.into_inner()) = Some(path.clone());
    Some(path)
}

pub(crate) fn set_start_info(fields: Vec<(&'static str, String)>) {
    *START_INFO.lock().unwrap_or_else(|e| e.into_inner()) = fields;
}

/// Подія `start`: версія, чи пишуться слова, як увімкнено (`via`: `args` — параметром, `tray` — з меню), решта даних.
pub(crate) fn log_start(words: bool, via: &str) {
    let mut f = vec![
        ("version", env!("CARGO_PKG_VERSION").to_string()),
        ("words", words.to_string()),
        ("via", via.to_string()),
    ];
    f.extend(START_INFO.lock().unwrap_or_else(|e| e.into_inner()).iter().cloned());
    log_event("start", f);
}

/// Увімкнути лог під час роботи. Повертає шлях до файлу.
pub(crate) fn enable(words: bool, via: &str) -> Option<PathBuf> {
    let path = open(words)?;
    log_start(words, via);
    Some(path)
}

/// Вимкнути лог (файл лишається).
pub(crate) fn disable() {
    log_event("stop", vec![]);
    *LOG.lock().unwrap_or_else(|e| e.into_inner()) = None;
}

/// Чи пишуться зараз слова.
pub(crate) fn words_enabled() -> bool {
    with_log(|l| l.words).unwrap_or(false)
}

/// Файл логу, якщо його вже відкривали в цьому запуску.
pub(crate) fn path() -> Option<PathBuf> {
    PATH.lock().unwrap_or_else(|e| e.into_inner()).clone()
}

static OUT: OnceLock<Mutex<Sender<String>>> = OnceLock::new();

/// Запустити потік, який друкує рядки з черги. Після цього `say` ніколи не блокує того, хто її викликав.
pub(crate) fn start_console_thread() {
    let (tx, rx) = channel::<String>();
    if OUT.set(Mutex::new(tx)).is_ok() {
        std::thread::spawn(move || {
            for line in rx {
                println!("{line}");
            }
        });
    }
}

/// Рядок у консоль без ризику зависнути (див. пояснення на початку файлу).
pub(crate) fn say(line: impl Into<String>) {
    let line = line.into();
    match OUT.get() {
        Some(tx) => {
            let _ = tx.lock().unwrap_or_else(|e| e.into_inner()).send(line);
        }
        None => println!("{line}"),
    }
}
