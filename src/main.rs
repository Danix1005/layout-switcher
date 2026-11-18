//! Автоперемикач розкладки EN <-> UK (Windows).
//!
//! Програма без вікна консолі: живе значком біля годинника (меню — права кнопка миші).
//! Низькорівневий хук клавіатури -> буфер поточного слова -> на пробілі/Enter/Tab перевіряємо словником ->
//! стираємо слово, друкуємо виправлене, перемикаємо розкладку вікна.
//!
//! Ручне виправлення: ПОДВІЙНЕ натискання лівого Shift (або Pause/Break) перемикає
//!   1) слово, яке зараз набираєш;  2) інакше — щойно набране слово (після пробілу/Tab; після автовиправлення
//!   це «скасувати»);  3) інакше — ВИДІЛЕНИЙ фрагмент (через буфер обміну, див. selection.rs).
//!
//! Модулі: `engine` — стан і рішення; `win` — обгортки Win32; `selection` — виділений фрагмент; `logger` — лог
//! і консоль; `tray` — значок і меню; `autostart` — запис автозапуску; `updater`, `http`, `settings` — оновлення
//! з GitHub (перевірка раз на добу, установка за кліком). Уся логіка без Windows (словники,
//! розкладки, правила, параметри) — у бібліотеці `layout_switcher`. Параметри запуску: див. `cli.rs`.

// Без вікна консолі. Для налагодження є `--console`.
#![windows_subsystem = "windows"]

mod autostart;
mod engine;
mod http;
mod appsoff;
mod learned;
mod password;
mod logger;
mod selection;
mod settings;
mod tray;
mod updater;
mod win;

use std::ffi::c_void;
use std::path::PathBuf;
use std::ptr::{null, null_mut};
use std::sync::atomic::{AtomicPtr, AtomicU64, Ordering};
use std::time::Duration;

use layout_switcher::apps::AppRules;
use layout_switcher::cli::{self, Command, Options};
use layout_switcher::detect::Dicts;
use layout_switcher::inject::InjectedPolicy;
use layout_switcher::session::Embed;

use windows_sys::Win32::Foundation::{HWND, LPARAM, LRESULT, WPARAM};
use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
use windows_sys::Win32::UI::WindowsAndMessaging::{
    CallNextHookEx, DispatchMessageW, GetMessageW, SetTimer, SetWindowsHookExW, TranslateMessage,
    UnhookWindowsHookEx, KBDLLHOOKSTRUCT, MB_ICONERROR, MB_ICONINFORMATION, MSG, WH_KEYBOARD_LL, WH_MOUSE_LL,
    WM_KEYDOWN, WM_KEYUP, WM_LBUTTONDOWN, WM_MBUTTONDOWN, WM_RBUTTONDOWN, WM_SYSKEYDOWN, WM_SYSKEYUP,
};

use crate::logger::{log_enabled, log_event, say};
use crate::win::hkl_hex;

const LLKHF_INJECTED: u32 = 0x10;
/// Як часто перевіряти, чи можна перевстановити хуки.
const WATCHDOG_MS: u32 = 10_000;
/// Хуки перевстановлюємо лише в паузі між натисканнями, щоб не пропустити клавішу.
const REHOOK_IDLE: Duration = Duration::from_secs(2);
/// Раз на стільки спрацювань сторожа пишемо в лог лічильники (10 хв).
const STATS_EVERY: u64 = 60;

static KB_HOOK: AtomicPtr<c_void> = AtomicPtr::new(null_mut());
static MOUSE_HOOK: AtomicPtr<c_void> = AtomicPtr::new(null_mut());
static REHOOKS: AtomicU64 = AtomicU64::new(0);
static TICKS: AtomicU64 = AtomicU64::new(0);

/// Файл поруч з exe, а якщо там нема — у поточній теці.
fn beside_exe(name: &str) -> Option<PathBuf> {
    let mut candidates = Vec::new();
    if let Ok(exe) = std::env::current_exe() {
        if let Some(dir) = exe.parent() {
            candidates.push(dir.join(name));
        }
    }
    candidates.push(PathBuf::from(name));
    candidates.into_iter().find(|p| p.is_file())
}

/// Помилка запуску: вікно повідомлення (консолі нема) і вихід.
fn fail(msg: &str) -> ! {
    eprintln!("{msg}");
    win::message_box(msg, MB_ICONERROR);
    std::process::exit(1);
}

fn main() {
    let opts = match cli::parse(std::env::args().skip(1)) {
        Ok(Command::Run(o)) => o,
        Ok(Command::Help) => {
            win::message_box(&cli::help_text(env!("CARGO_PKG_VERSION")), MB_ICONINFORMATION);
            return;
        }
        Ok(Command::Version) => {
            win::message_box(&format!("layout-switcher {}", env!("CARGO_PKG_VERSION")), MB_ICONINFORMATION);
            return;
        }
        Ok(Command::Control(c)) => {
            // Команда для вже запущеної програми. Код виходу: 0 — надіслано, 1 — програма не запущена.
            std::process::exit(if unsafe { tray::control(c) } { 0 } else { 1 });
        }
        Err(e) => {
            win::message_box(&format!("{e}\n\n{}", cli::help_text(env!("CARGO_PKG_VERSION"))), MB_ICONERROR);
            std::process::exit(2);
        }
    };
    run(opts);
}

fn run(opts: Options) {
    // Консолі нема, і паніки в обробниках хуків ловляться й нічого не друкують — тож лишаємо слід у лозі.
    std::panic::set_hook(Box::new(|info| {
        log_event("panic", vec![("info", layout_switcher::privacy::redact(&info.to_string()))]);
        eprintln!("{info}");
    }));
    if opts.console {
        unsafe { win::alloc_console() };
    }

    // Нас запустила попередня версія після оновлення: чекаємо, поки вона завершиться (вона ще тримає значок).
    if let Some(pid) = opts.after_update {
        win::wait_for_process(pid, 20_000);
    }

    if !unsafe { win::single_instance() } {
        if !opts.quiet {
            win::message_box(
                "layout-switcher уже працює в цьому сеансі Windows — значок біля годинника (біля ^, якщо його приховано).\n\n\
                 Другий екземпляр виправляв би кожне слово двічі, тому цей закривається.",
                MB_ICONINFORMATION,
            );
        }
        return;
    }

    let mut dicts = match Dicts::load() {
        Ok(d) => d,
        Err(e) => fail(&format!("Не вдалося завантажити словники: {e}\n\nТека dict\\ має лежати поруч з layout-switcher.exe.")),
    };
    let (n_en, n_uk) = dicts.len();
    let n_forms = dicts.forms_len();
    let n_it = dicts.it_len();
    // Слова, які програма вивчила з ручних виправлень (%APPDATA%\layout-switcher\learned_*.txt).
    let (n_learned_en, n_learned_uk) = learned::load_into(&mut dicts);

    let rules = beside_exe("apps-off.txt")
        .and_then(|p| std::fs::read_to_string(p).ok())
        .map(|t| AppRules::from_user_list(&t))
        .unwrap_or_default();
    let n_off = rules.user_count();

    unsafe {
        // Беремо розкладки, які реально встановлені, а не вгадуємо: у користувача може бути
        // «Українська (розширена)» (KLID 00020422), а не звичайна (00000422).
        let list = win::installed_layouts();
        let installed = list.iter().map(|&h| hkl_hex(h)).collect::<Vec<_>>().join(", ");
        let pick = |pred: &dyn Fn(usize) -> bool| list.iter().copied().find(|&h| pred(h as usize & 0xFFFF));
        let hkl_uk = pick(&|l| l == 0x0422);
        let hkl_en = pick(&|l| l == 0x0409).or_else(|| pick(&|l| l & 0x3FF == 0x09));
        let (Some(hkl_en), Some(hkl_uk)) = (hkl_en, hkl_uk) else {
            fail(&format!(
                "У Windows має бути встановлена і англійська, і українська розкладка. Знайдено: [{installed}]"
            ));
        };
        let remote = win::is_remote_session();

        // Дані для події `start`: їх же використає лог, увімкнений пізніше з меню значка.
        logger::set_start_info(vec![
            ("dict_en", n_en.to_string()),
            ("dict_uk", n_uk.to_string()),
            ("dict_uk_forms", n_forms.to_string()),
            ("dict_it", n_it.to_string()),
            ("learned_en", n_learned_en.to_string()),
            ("learned_uk", n_learned_uk.to_string()),
            ("installed", installed.replace(", ", ",")),
            ("hkl_en", hkl_hex(hkl_en)),
            ("hkl_uk", hkl_hex(hkl_uk)),
            ("hotkey", "double-lshift".to_string()),
            ("remote_session", (remote as u8).to_string()),
            ("injected", opts.injected.tag().to_string()),
            ("switch", opts.switch.tag().to_string()),
            ("apps_off", n_off.to_string()),
        ]);
        let log_path = if opts.log { logger::open(opts.log_words) } else { None };
        if opts.log && log_path.is_none() {
            eprintln!("Не вдалося відкрити layout-switcher.log — працюю без логу.");
        }
        if log_enabled() {
            logger::log_start(opts.log_words, "args");
        }

        password::start(); // фоновий потік: чи не стоїть курсор у полі пароля
        engine::init(engine::State::new(
            dicts,
            rules,
            InjectedPolicy::new(opts.injected),
            opts.switch,
            hkl_en,
            hkl_uk,
        ));

        if opts.console {
            win::disable_quick_edit();
        }
        if !install_hooks() {
            fail("Не вдалося встановити хуки клавіатури (SetWindowsHookExW).");
        }
        SetTimer(null_mut(), 0, WATCHDOG_MS, Some(watchdog));

        // Значок біля годинника. Якщо створити не вдалося (нема Провідника?), програма все одно працює,
        // але завершити її можна лише через диспетчер завдань або `layout-switcher.exe --exit`.
        let tray_ok = tray::create(tray::autostart_command(&opts.autostart_args()), !opts.quiet && opts.after_update.is_none());
        log_event("tray", vec![("ok", (tray_ok as u8).to_string())]);

        // Оновлення: після перезапуску повідомляємо про нову версію, прибираємо залишки старої, далі — перевірка раз на добу.
        if opts.after_update.is_some() {
            log_event("update", vec![("stage", "restarted".to_string()), ("result", "ok".to_string()), ("info", env!("CARGO_PKG_VERSION").to_string())]);
            tray::notify("Оновлено", &format!("layout-switcher працює у версії {}.", env!("CARGO_PKG_VERSION")));
        }
        updater::cleanup_leftovers();
        updater::start_scheduler();

        if opts.console {
            print_banner(&opts, (n_en, n_uk, n_forms, n_it), n_off, remote, &installed, (hkl_en, hkl_uk), log_path, tray_ok);
            // далі в консоль пишемо лише через чергу: потік хука не має права чекати на консоль
            logger::start_console_thread();
        }

        let mut msg: MSG = std::mem::zeroed();
        while GetMessageW(&mut msg, null_mut(), 0, 0) > 0 {
            TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }
    }
}

/// Початкові повідомлення для вікна `--console`.
#[allow(clippy::too_many_arguments)]
fn print_banner(
    opts: &Options,
    (n_en, n_uk, n_forms, n_it): (usize, usize, usize, usize),
    n_off: usize,
    remote: bool,
    installed: &str,
    (hkl_en, hkl_uk): (isize, isize),
    log_path: Option<PathBuf>,
    tray_ok: bool,
) {
    println!(
        "layout-switcher {} запущено: {n_en} англ. + {n_uk} укр. слів, {n_forms} укр. словоформ, {n_it} ІТ-термінів{}.",
        env!("CARGO_PKG_VERSION"),
        if n_forms == 0 { " (УВАГА: нема dict/uk_forms.fst — менше слів розпізнається)" } else { "" }
    );
    println!("Розкладки: EN={} UK={} (встановлено: {installed})", hkl_hex(hkl_en), hkl_hex(hkl_uk));
    println!("Автовиправлення на пробілі/Enter/Tab.");
    match opts.switch {
        Embed::Off => println!("Після кожного виправлення перемикаю розкладку (--switch=always)."),
        _ => println!("Англійське слово посеред українського тексту виправляю, не перемикаючи розкладку (вимкнути: --switch=always)."),
    }
    println!("Вручну: подвійний лівий Shift (або Pause/Break) — перемкнути слово, яке набираєш, щойно набране");
    println!("слово (після пробілу; після автовиправлення це «скасувати») або ВИДІЛЕНИЙ фрагмент.");
    if remote {
        println!("Працюю всередині сеансу віддаленого робочого стола (RDP).");
    }
    println!("У вікнах віддаленого доступу (RDP, AnyDesk, TeamViewer, віртуальні машини) не втручаюсь.");
    if n_off > 0 {
        println!("Вимкнено в apps-off.txt: {n_off} застосунк(ів).");
    }
    if !tray_ok {
        println!("УВАГА: значок у треї створити не вдалося. Завершити програму: layout-switcher.exe --exit");
    }
    if let Some(p) = log_path {
        println!("ЛОГ УВІМКНЕНО: {}", p.display());
        if opts.log_words {
            println!("!! У лог пишуться набрані слова (можуть бути паролі). Scroll Lock = пауза/відновлення логу.");
            println!("   (для PuTTY, WinBox, терміналів, VPN, віддаленого доступу слова в лог не пишуться)");
            println!("!! Не набирай паролі при увімкненому --log-words, а після тестування видали лог.");
        } else {
            println!("(Слова в лог не пишуться; для аналізу слів запусти з --log-words.)");
        }
    }
    println!("Вийти: меню значка біля годинника, або закрий це вікно.");
}

// ───────────────────────────── хуки ─────────────────────────────

/// Встановити (або перевстановити) хуки клавіатури й миші.
unsafe fn install_hooks() -> bool {
    let hmod = GetModuleHandleW(null());
    for (slot, id, proc_) in [
        (&KB_HOOK, WH_KEYBOARD_LL, kb_proc as unsafe extern "system" fn(i32, WPARAM, LPARAM) -> LRESULT),
        (&MOUSE_HOOK, WH_MOUSE_LL, mouse_proc as unsafe extern "system" fn(i32, WPARAM, LPARAM) -> LRESULT),
    ] {
        let old = slot.swap(null_mut(), Ordering::SeqCst);
        if !old.is_null() {
            UnhookWindowsHookEx(old);
        }
        let new = SetWindowsHookExW(id, Some(proc_), hmod, 0);
        if new.is_null() {
            return false;
        }
        slot.store(new, Ordering::SeqCst);
    }
    true
}

/// Сторож. Якщо обробник хука хоч раз не встиг відповісти (система зайнята, диск «задумався»), Windows знімає
/// хук БЕЗ попередження — і перемикач мовчки перестає працювати. Тому в паузах між натисканнями ставимо хуки
/// заново. Заодно наш хук лишається першим у ланцюжку, попереду хуків, які інші програми поставили пізніше.
unsafe extern "system" fn watchdog(_hwnd: HWND, _msg: u32, _id: usize, _time: u32) {
    let _ = std::panic::catch_unwind(|| {
        if engine::idle() >= REHOOK_IDLE {
            if unsafe { install_hooks() } {
                REHOOKS.fetch_add(1, Ordering::Relaxed);
            } else {
                say("УВАГА: не вдалося перевстановити хуки — перезапусти layout-switcher.");
                tray::notify("layout-switcher", "Не вдалося перевстановити перехоплення клавіатури. Перезапусти програму.");
            }
        }
        if TICKS.fetch_add(1, Ordering::Relaxed) % STATS_EVERY == STATS_EVERY - 1 {
            engine::log_stats(REHOOKS.load(Ordering::Relaxed));
        }
    });
}

unsafe extern "system" fn kb_proc(code: i32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    if code == 0 {
        let kb = &*(lp as *const KBDLLHOOKSTRUCT);
        // власні події (з міткою) пропускаємо завжди; чужі програмні — як вирішить політика в engine
        if kb.dwExtraInfo != win::MAGIC {
            let injected = (kb.flags & LLKHF_INJECTED) != 0;
            let is_down = wp as u32 == WM_KEYDOWN || wp as u32 == WM_SYSKEYDOWN;
            let is_up = wp as u32 == WM_KEYUP || wp as u32 == WM_SYSKEYUP;
            let vk = kb.vkCode;
            let swallow = std::panic::catch_unwind(|| engine::on_key(vk, is_down, is_up, injected)).unwrap_or(false);
            if swallow {
                return 1;
            }
        }
    }
    CallNextHookEx(null_mut(), code, wp, lp)
}

unsafe extern "system" fn mouse_proc(code: i32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    if code == 0 {
        let m = wp as u32;
        if m == WM_LBUTTONDOWN || m == WM_RBUTTONDOWN || m == WM_MBUTTONDOWN {
            let _ = std::panic::catch_unwind(engine::on_click);
        }
    }
    CallNextHookEx(null_mut(), code, wp, lp)
}
