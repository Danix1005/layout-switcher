//! «Не працювати в цьому застосунку» — пункт меню значка.
//!
//! Список лежить у `%APPDATA%\layout-switcher\apps-off.txt` (окремо від `apps-off.txt` поруч із програмою: туди
//! можна не мати права писати, і його перезапише новий пакет). Меню працює в головному потоці, а рішення
//! «вмикатись чи ні» приймає потік хука, тому між ними — лічильник змін: потік хука перечитує список, коли
//! лічильник змінився, і одразу перераховує режим для поточного вікна (без перезапуску програми).

use std::path::PathBuf;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::Mutex;

use layout_switcher::apps::toggle_in_list;

/// Застосунок, у якому користувач востаннє набирав текст (його й пропонує меню).
static CURRENT: Mutex<String> = Mutex::new(String::new());
/// Скільки разів список змінювали (потік хука порівнює зі своїм).
static GEN: AtomicU32 = AtomicU32::new(0);

fn file() -> Option<PathBuf> {
    std::env::var_os("APPDATA").map(|d| PathBuf::from(d).join("layout-switcher").join("apps-off.txt"))
}

/// Текст списку (порожній, якщо файлу ще нема).
pub(crate) fn read() -> String {
    file().and_then(|p| std::fs::read_to_string(p).ok()).unwrap_or_default()
}

pub(crate) fn generation() -> u32 {
    GEN.load(Ordering::Acquire)
}

/// Потік хука: користувач перейшов у вікно цього застосунку.
pub(crate) fn set_current(exe: &str) {
    if exe.is_empty() || exe.starts_with('?') {
        return;
    }
    if let Ok(mut c) = CURRENT.try_lock() {
        if *c != exe {
            *c = exe.to_string();
        }
    }
}

pub(crate) fn current() -> Option<String> {
    CURRENT.lock().ok().map(|c| c.clone()).filter(|c| !c.is_empty())
}

/// Вимкнути або знову ввімкнути застосунок. `Ok(true)` — тепер вимкнено.
pub(crate) fn toggle(exe: &str) -> std::io::Result<bool> {
    let path = file().ok_or_else(|| std::io::Error::other("нема %APPDATA%"))?;
    let (text, off) = toggle_in_list(&read(), exe);
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let body = if text.starts_with('#') {
        text
    } else {
        format!("# Застосунки, вимкнені з меню значка layout-switcher (одне ім'я exe на рядок).\n{text}")
    };
    std::fs::write(&path, body)?;
    GEN.fetch_add(1, Ordering::AcqRel);
    Ok(off)
}
