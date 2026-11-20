//! Оновлення з GitHub: перевірка (раз на добу або за запитом), сповіщення, завантаження, перевірка, заміна exe,
//! перезапуск. Мережа — у фонових потоках, вікна й меню — у головному (див. `tray.rs`).
//!
//! Ставиться лише те, що користувач підтвердив кліком. Файл перед запуском перевіряється: адреса, розмір,
//! SHA-256 зі списку `SHA256SUMS` того самого релізу (логіка й тести — `layout_switcher::update`).
//!
//! Заміна запущеного exe: Windows не дає перезаписати файл, що виконується, але дозволяє його перейменувати.
//! Тож `layout-switcher.exe` -> `layout-switcher.exe.old`, новий файл стає на його місце, запускаємо його з
//! `--after-update=<наш номер процесу>` (він дочекається, поки ми завершимось) і виходимо. Нова копія видаляє `.old`.

use std::ffi::OsString;
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::ptr::null;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Mutex;
use std::time::Duration;

use layout_switcher::sha256;
use layout_switcher::update::{self, Source, Update};

use windows_sys::Win32::UI::Shell::ShellExecuteW;
use windows_sys::Win32::UI::WindowsAndMessaging::{MB_ICONERROR, MB_ICONQUESTION, MB_YESNO};

use crate::logger::log_event;
use crate::{http, settings, tray, win};

const VERSION: &str = env!("CARGO_PKG_VERSION");
const IDYES: i32 = 6;
/// Перша перевірка — не одразу після запуску (при вході в Windows мережа ще може не піднятись).
const FIRST_CHECK_DELAY_SECS: u64 = 45;
/// Як часто сторож дивиться, чи настав час перевіряти (сама перевірка — раз на добу).
const POLL_SECS: u64 = 30 * 60;

static AVAILABLE: Mutex<Option<Update>> = Mutex::new(None);
static BUSY: AtomicBool = AtomicBool::new(false);

/// Дозвіл на одну операцію з мережею за раз (перевірка або встановлення).
struct Busy;

impl Busy {
    fn take() -> Option<Busy> {
        BUSY.compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst).ok().map(|_| Busy)
    }
}

impl Drop for Busy {
    fn drop(&mut self) {
        BUSY.store(false, Ordering::SeqCst);
    }
}

pub(crate) fn is_busy() -> bool {
    BUSY.load(Ordering::SeqCst)
}

/// Що знайшли: (версія, чи потрібен повний пакет).
pub(crate) fn available() -> Option<(String, bool)> {
    AVAILABLE.lock().unwrap_or_else(|e| e.into_inner()).as_ref().map(|u| (u.version.to_string(), u.full_package))
}

/// Звідки оновлюємось. `None` — репозиторій у цій збірці не вказано (заповнювач у Cargo.toml).
pub(crate) fn source() -> Option<Source> {
    // Лише для перевірки на локальному сервері; у справжній збірці цього коду нема.
    #[cfg(feature = "update-test")]
    if let Ok(api) = std::env::var("LS_UPDATE_API") {
        let p = update::split_url(&api)?;
        let origin = format!("{}://{}:{}/", if p.https { "https" } else { "http" }, p.host, p.port);
        return Some(Source { api_url: api, download_prefix: origin.clone(), page_prefix: origin, allow_http: true });
    }
    let (owner, repo) = update::repo_from_url(env!("CARGO_PKG_REPOSITORY"))?;
    Some(Source::github(&owner, &repo))
}

fn log(stage: &str, result: &str, extra: &str) {
    // У журнал йдуть лише адреси, версії й помилки мережі — набраного тексту тут нема, тож маскувати нічого.
    log_event("update", vec![("stage", stage.to_string()), ("result", result.to_string()), ("info", extra.to_string())]);
}

// ───────────────────────────── перевірка ─────────────────────────────

fn fetch(src: &Source) -> Result<Option<Update>, String> {
    let body = match http::get(src, &src.api_url, 1 << 20, "application/vnd.github+json") {
        Ok(b) => b,
        Err(e) if e.status == 404 => return Ok(None), // реліз ще не випущено
        Err(e) => return Err(e.message),
    };
    let text = String::from_utf8(body).map_err(|_| "відповідь GitHub не в UTF-8".to_string())?;
    let rel = update::parse_release(&text)?;
    update::evaluate(VERSION, &rel, src)
}

fn run_check(manual: bool) {
    let Some(src) = source() else {
        log("check", "no_repo", "");
        if manual {
            tray::notify("Оновлення недоступні", "У цій збірці не вказано репозиторій GitHub.");
        }
        return;
    };
    match fetch(&src) {
        Ok(None) => {
            settings::set_last_check(settings::now());
            *AVAILABLE.lock().unwrap_or_else(|e| e.into_inner()) = None;
            log("check", "up_to_date", VERSION);
            if manual {
                tray::notify("Оновлень нема", &format!("У вас остання версія: {VERSION}."));
            }
        }
        Ok(Some(u)) => {
            settings::set_last_check(settings::now());
            log("check", "available", &u.version.to_string());
            let is_new = AVAILABLE.lock().unwrap_or_else(|e| e.into_inner()).as_ref().map(|a| a.version) != Some(u.version);
            let (title, text) = if u.full_package {
                (
                    format!("Вийшла версія {}", u.version),
                    "Потрібен повний пакет (змінились словники). Клікніть, щоб відкрити сторінку завантаження.".to_string(),
                )
            } else {
                (
                    format!("Вийшла версія {}", u.version),
                    format!("У вас {VERSION}. Клікніть, щоб оновити (або меню значка → «Оновлення»)."),
                )
            };
            *AVAILABLE.lock().unwrap_or_else(|e| e.into_inner()) = Some(u);
            // про ту саму версію автоматична перевірка нагадує лише раз за запуск; ручна — завжди
            if manual || is_new {
                tray::notify_update(&title, &text);
            }
        }
        Err(e) => {
            log("check", "error", &e);
            if manual {
                tray::notify("Не вдалося перевірити оновлення", &e);
            }
        }
    }
}

/// Перевірити оновлення у фоні. `manual` — з меню (тоді про «оновлень нема» і про помилки теж повідомляємо).
pub(crate) fn check_now(manual: bool) {
    let Some(guard) = Busy::take() else {
        if manual {
            tray::notify("Оновлення", "Попередня операція ще триває.");
        }
        return;
    };
    std::thread::spawn(move || {
        let _guard = guard;
        let _ = std::panic::catch_unwind(|| run_check(manual));
    });
}

/// Фоновий потік: раз на добу (якщо не вимкнено в меню) перевіряє, чи нема нової версії.
pub(crate) fn start_scheduler() {
    #[cfg(feature = "update-test")]
    let (first, poll) = (env_secs("LS_UPDATE_DELAY", FIRST_CHECK_DELAY_SECS), env_secs("LS_UPDATE_POLL", POLL_SECS));
    #[cfg(not(feature = "update-test"))]
    let (first, poll) = (FIRST_CHECK_DELAY_SECS, POLL_SECS);
    std::thread::spawn(move || {
        std::thread::sleep(Duration::from_secs(first));
        loop {
            if settings::auto_check() && source().is_some() && update::due(settings::now(), settings::last_check()) {
                if let Some(guard) = Busy::take() {
                    let _ = std::panic::catch_unwind(|| run_check(false));
                    drop(guard);
                }
            }
            std::thread::sleep(Duration::from_secs(poll));
        }
    });
}

#[cfg(feature = "update-test")]
fn env_secs(name: &str, default: u64) -> u64 {
    std::env::var(name).ok().and_then(|v| v.parse().ok()).unwrap_or(default)
}

// ───────────────────────────── встановлення ─────────────────────────────

struct Fail {
    msg: String,
    /// Запропонувати відкрити сторінку релізу (нема прав на запис у теку програми тощо).
    offer_page: bool,
}

fn fail(msg: impl Into<String>) -> Fail {
    Fail { msg: msg.into(), offer_page: false }
}

fn with_suffix(p: &Path, suffix: &str) -> PathBuf {
    let mut s: OsString = p.as_os_str().to_owned();
    s.push(suffix);
    PathBuf::from(s)
}

fn confirm(text: &str) -> bool {
    #[cfg(feature = "update-test")]
    if std::env::var_os("LS_UPDATE_AUTOYES").is_some() {
        return true;
    }
    win::message_box(text, MB_YESNO | MB_ICONQUESTION) == IDYES
}

/// Відкрити сторінку релізу в браузері. Адресу вже перевірено в `update::evaluate`.
fn open_page(url: &str) {
    unsafe {
        ShellExecuteW(std::ptr::null_mut(), win::wide("open").as_ptr(), win::wide(url).as_ptr(), null(), null(), 1);
    }
}

/// Викликається з меню або кліком по сповіщенню (головний потік): питає підтвердження й ставить оновлення.
pub(crate) fn install() {
    let Some(u) = AVAILABLE.lock().unwrap_or_else(|e| e.into_inner()).clone() else {
        check_now(true);
        return;
    };
    if u.full_package {
        log("install", "full_package", &u.version.to_string());
        open_page(&u.page_url);
        return;
    }
    let text = format!(
        "Вийшла версія {} (у вас {VERSION}).\n\n{}\n\nЗавантажити з GitHub, перевірити контрольну суму SHA-256 і перезапустити програму?",
        u.version,
        update::short_notes(&u.notes, 700)
    );
    if !confirm(&text) {
        log("install", "declined", &u.version.to_string());
        return;
    }
    let Some(guard) = Busy::take() else {
        tray::notify("Оновлення", "Попередня операція ще триває.");
        return;
    };
    std::thread::spawn(move || {
        let r = std::panic::catch_unwind(|| download_and_swap(&u)).unwrap_or_else(|_| Err(fail("внутрішня помилка")));
        // Дозвіл віддаємо до вікна з помилкою: поки воно відкрите, меню не має бути «зайнятим».
        drop(guard);
        if let Err(f) = r {
            log("install", "error", &f.msg);
            if f.offer_page {
                let q = format!("{}\n\nВідкрити сторінку релізу, щоб завантажити оновлення вручну?", f.msg);
                if win::message_box(&q, MB_YESNO | MB_ICONERROR) == IDYES {
                    open_page(&u.page_url);
                }
            } else {
                win::message_box(&format!("Оновлення не встановлено.\n\n{}", f.msg), MB_ICONERROR);
            }
        }
    });
}

fn download_and_swap(u: &Update) -> Result<(), Fail> {
    let src = source().ok_or_else(|| fail("не вказано репозиторій оновлень"))?;
    let exe = std::env::current_exe().map_err(|e| fail(format!("не знаю, де лежить програма: {e}")))?;
    let (new_path, old_path) = (with_suffix(&exe, ".new"), with_suffix(&exe, ".old"));

    // 1. Контрольна сума й сам файл
    log("install", "download", &u.exe_url);
    let sums = http::get(&src, &u.sums_url, 64 * 1024, "application/octet-stream").map_err(|e| fail(format!("SHA256SUMS: {}", e.message)))?;
    let expected = update::sum_for(&String::from_utf8_lossy(&sums), update::EXE_ASSET)
        .ok_or_else(|| fail("у SHA256SUMS нема (або неоднозначний) запис для layout-switcher.exe"))?;
    let data = http::get(&src, &u.exe_url, update::MAX_EXE_BYTES, "application/octet-stream")
        .map_err(|e| fail(format!("завантаження: {}", e.message)))?;
    update::verify_exe(&data, u.exe_size, &expected).map_err(fail)?;
    log("install", "verified", &sha256::to_hex(&expected));

    // 2. Записуємо поруч і перечитуємо: те, що лежить на диску, має мати ту саму суму
    let _ = fs::remove_file(&new_path);
    let write = || -> std::io::Result<()> {
        let mut f = fs::File::create(&new_path)?;
        f.write_all(&data)?;
        f.sync_all()
    };
    if let Err(e) = write() {
        let _ = fs::remove_file(&new_path);
        return Err(Fail { msg: format!("Не вдалося записати файл поруч з програмою ({e}). Можливо, тека захищена від запису."), offer_page: true });
    }
    let on_disk = fs::read(&new_path).map_err(|e| fail(format!("не вдалося перечитати файл: {e}")))?;
    if sha256::digest(&on_disk) != expected {
        let _ = fs::remove_file(&new_path);
        return Err(fail("файл на диску не збігається із завантаженим (помилка запису?)"));
    }

    // 3. Підміна: запущений exe можна перейменувати, але не перезаписати
    let _ = fs::remove_file(&old_path);
    if let Err(e) = fs::rename(&exe, &old_path) {
        let _ = fs::remove_file(&new_path);
        return Err(Fail { msg: format!("Не вдалося замінити програму ({e}). Можливо, тека захищена від запису."), offer_page: true });
    }
    if let Err(e) = fs::rename(&new_path, &exe) {
        let back = fs::rename(&old_path, &exe);
        let _ = fs::remove_file(&new_path);
        return Err(fail(format!("не вдалося поставити новий файл ({e}); стару версію {}", if back.is_ok() { "повернуто" } else { "НЕ вдалося повернути — вона лежить як layout-switcher.exe.old" })));
    }

    // 4. Запускаємо нову версію й виходимо
    let mut args: Vec<String> = std::env::args().skip(1).filter(|a| !a.starts_with("--after-update=")).collect();
    args.push(format!("--after-update={}", std::process::id()));
    log("install", "restart", &args.join(" "));
    match std::process::Command::new(&exe).args(&args).spawn() {
        Ok(_) => {
            tray::request_exit();
            Ok(())
        }
        Err(e) => {
            // назад: нову прибираємо, стару ставимо на місце
            let aside = fs::rename(&exe, &new_path);
            let back = fs::rename(&old_path, &exe);
            let _ = fs::remove_file(&new_path);
            let state = if aside.is_ok() && back.is_ok() { "стару версію повернуто" } else { "стара версія лежить як layout-switcher.exe.old" };
            Err(fail(format!("не вдалося запустити нову версію ({e}); {state}")))
        }
    }
}

/// Нова версія не змогла запуститися одразу після оновлення (`fail` у `main.rs`): ставимо назад попередню
/// (`.old`), кажемо чому й запускаємо її. Так зламаний випуск не лишає людину без програми (0.9.6 на деяких
/// комп'ютерах не знаходила українську розкладку й одразу закривалась). `false` — повертати нічого.
pub(crate) fn roll_back(reason: &str) -> bool {
    let Ok(exe) = std::env::current_exe() else { return false };
    let (old, failed) = (with_suffix(&exe, ".old"), with_suffix(&exe, ".failed"));
    if !old.exists() {
        return false;
    }
    let _ = fs::remove_file(&failed);
    if fs::rename(&exe, &failed).is_err() {
        return false;
    }
    if fs::rename(&old, &exe).is_err() {
        let _ = fs::rename(&failed, &exe);
        return false;
    }
    log("rollback", "ok", VERSION);
    win::message_box(
        &format!(
            "Нова версія {VERSION} не змогла запуститися:\n\n{reason}\n\nПовернуто попередню версію — вона зараз \
             запуститься. Виправлену версію програма запропонує, щойно вона вийде."
        ),
        MB_ICONERROR,
    );
    // Попередня копія дочекається, поки ми завершимось (значок і «один екземпляр» поки наші).
    let mut args: Vec<String> = std::env::args().skip(1).filter(|a| !a.starts_with("--after-update=")).collect();
    args.push(format!("--after-update={}", std::process::id()));
    let _ = std::process::Command::new(&exe).args(&args).spawn();
    true
}

/// Прибрати залишки попереднього оновлення (`.old`, `.new`, `.failed`). Стара копія може ще кілька секунд не відпускати файл.
pub(crate) fn cleanup_leftovers() {
    let Ok(exe) = std::env::current_exe() else { return };
    let paths = [with_suffix(&exe, ".old"), with_suffix(&exe, ".new"), with_suffix(&exe, ".failed")];
    if !paths.iter().any(|p| p.exists()) {
        return;
    }
    std::thread::spawn(move || {
        for _ in 0..20 {
            for p in &paths {
                if p.exists() {
                    let _ = fs::remove_file(p);
                }
            }
            if !paths.iter().any(|p| p.exists()) {
                return;
            }
            std::thread::sleep(Duration::from_secs(1));
        }
    });
}
