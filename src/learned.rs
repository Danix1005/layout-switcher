//! Особистий словник, який програма поповнює сама: `%APPDATA%\layout-switcher\learned_en.txt` і `learned_uk.txt`.
//!
//! Слово потрапляє сюди, коли користувач переніс його в іншу розкладку подвійним Shift (а словники про нього
//! нічого не знали: назви, прізвища, терміни), або коли скасував подвійним Shift АВТОМАТИЧНЕ виправлення (тоді
//! запам'ятовується те, що він набрав, — і більше не виправляється). Що саме вчити, вирішує бібліотека
//! (`Dicts::learnable`, `Dicts::learnable_revert`); тут — лише файли й черга від фонового виправлення виділеного.
//!
//! Файли звичайні текстові: зайве слово можна видалити в Блокноті (діє після перезапуску програми).
//! Запис у файл — у фоновому потоці: у потоці хука клавіатури нічого повільного робити не можна.

use std::io::Write;
use std::path::PathBuf;
use std::sync::Mutex;

use layout_switcher::detect::Dicts;
use layout_switcher::keymap::{Key, Lang};

const HEADER: &str = "\
# Слова, які layout-switcher вивчив сам: ви вручну перенесли їх у цю розкладку подвійним Shift
# (або скасували ним автоматичне виправлення). Тепер вони виправляються (чи не чіпаються) автоматично.
# Зайве слово можна просто видалити з файлу — зміни діють після перезапуску програми.
";

/// Слова з виправлення виділеного (фоновий потік): їх перевіряє й додає потік хука, коли до нього дійде черга.
static OFFERS: Mutex<Vec<(Vec<Key>, Lang)>> = Mutex::new(Vec::new());
/// Щоб два фонові записи не переплутали рядки у файлі.
static WRITE: Mutex<()> = Mutex::new(());

fn dir() -> Option<PathBuf> {
    std::env::var_os("APPDATA").map(|d| PathBuf::from(d).join("layout-switcher"))
}

fn file(lang: Lang) -> Option<PathBuf> {
    let name = match lang {
        Lang::En => "learned_en.txt",
        Lang::Uk => "learned_uk.txt",
    };
    dir().map(|d| d.join(name))
}

/// Додати вивчені раніше слова до словників. Повертає (англ., укр.).
pub(crate) fn load_into(d: &mut Dicts) -> (usize, usize) {
    let mut n = |lang| file(lang).and_then(|p| d.add_words_from(&p, lang).ok()).unwrap_or(0);
    (n(Lang::En), n(Lang::Uk))
}

/// Дописати слово у файл (у фоні).
pub(crate) fn save(lang: Lang, word: String) {
    std::thread::spawn(move || {
        let _guard = WRITE.lock();
        let Some(path) = file(lang) else { return };
        let _ = append(&path, &word);
    });
}

fn append(path: &PathBuf, word: &str) -> std::io::Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let fresh = !path.exists();
    let mut f = std::fs::OpenOptions::new().create(true).append(true).open(path)?;
    if fresh {
        f.write_all(HEADER.as_bytes())?;
    }
    writeln!(f, "{word}")
}

/// Створити порожні файли (з поясненням), якщо їх ще нема, і повернути шлях до англійського — для «показати».
pub(crate) fn ensure_files() -> Option<PathBuf> {
    let _guard = WRITE.lock();
    for lang in [Lang::En, Lang::Uk] {
        let path = file(lang)?;
        if !path.exists() {
            std::fs::create_dir_all(path.parent()?).ok()?;
            std::fs::write(&path, HEADER).ok()?;
        }
    }
    file(Lang::En)
}

/// Фоновий потік виправив виділене слово: запропонувати його потоку хука (той перевірить словниками).
pub(crate) fn offer(keys: Vec<Key>, lang: Lang) {
    if let Ok(mut q) = OFFERS.lock() {
        if q.len() < 32 {
            q.push((keys, lang));
        }
    }
}

/// Забрати запропоновані слова (з потоку хука: не чекаємо, якщо черга саме зайнята).
pub(crate) fn take_offers() -> Vec<(Vec<Key>, Lang)> {
    match OFFERS.try_lock() {
        Ok(mut q) if !q.is_empty() => std::mem::take(&mut *q),
        _ => Vec::new(),
    }
}
