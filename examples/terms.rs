//! Перевірка списку англійських термінів перед додаванням у dict/en_it.txt або dict/en_user.txt.
//!   cargo run --release --example terms -- my_terms.txt        (звіт)
//!   cargo run --release --example terms -- my_terms.txt --ok   (лише терміни, які варто додати, по одному на рядок)
//!
//! Термін зі списку стає «явним»: його ніколи не буде перетворено на українське слово. Ціна — те українське
//! слово, яке набирається тими самими клавішами, перестає автоматично виправлятись із англійської розкладки.
//! Тому терміни, чий український вигляд — ЧАСТЕ справжнє слово (`dsl` = «від», `dcs` = «всі»), додавати не варто.
//! Двобуквені терміни сюди теж не годяться: вони блокують «ви», «ті», «зі», «це»… (див. SHORT_EN у detect.rs).
use layout_switcher::detect::{Dicts, Verdict};
use layout_switcher::keymap::{keys_for, render, Lang};

/// Український вигляд терміна вважаємо «частим справжнім словом», якщо він є в морфологічному словнику
/// і входить у першу 10-тисячку частотного списку.
const COMMON_UK_RANK: u32 = 10_000;

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let Some(path) = args.iter().find(|a| !a.starts_with("--")) else {
        eprintln!("usage: terms <terms.txt> [--ok]");
        std::process::exit(2);
    };
    let only_ok = args.iter().any(|a| a == "--ok");
    let d = Dicts::load().expect("dict/ не знайдено — запусти з теки проєкту");
    let text = std::fs::read_to_string(path).expect("не можу прочитати список");
    let mut seen = std::collections::BTreeSet::new();
    let (mut ok, mut skipped) = (Vec::new(), Vec::new());

    for term in text.split_whitespace().map(str::to_lowercase) {
        if term.starts_with('#') || !seen.insert(term.clone()) {
            continue;
        }
        let n = term.chars().count();
        let Some(keys) = keys_for(&term, Lang::En) else {
            ok.push((term, String::from("(є символи поза розкладкою — колізій нема)")));
            continue;
        };
        let uk = render(&keys, Lang::Uk).to_lowercase();
        if n < 3 && term.chars().all(char::is_alphabetic) {
            skipped.push((term, format!("двобуквений (укр. вигляд «{uk}»)")));
            continue;
        }
        let rank = d.freq_rank(Lang::Uk, &uk);
        if d.in_forms(&uk) && rank.map_or(false, |r| r < COMMON_UK_RANK) {
            skipped.push((term, format!("укр. вигляд «{uk}» — часте слово (ранг {})", rank.unwrap())));
            continue;
        }
        let now = match d.verdict(&keys, Lang::En, None) {
            Verdict::Fix(f) => format!("зараз ПСУЄТЬСЯ: -> «{}»", f.text),
            v => format!("зараз: {}", v.tag()),
        };
        ok.push((term, now));
    }

    if only_ok {
        for (t, _) in &ok {
            println!("{t}");
        }
        return;
    }
    println!("варто додати: {}", ok.len());
    for (t, why) in ok.iter().filter(|(_, w)| w.contains("ПСУЄТЬСЯ")) {
        println!("   {t:<14} {why}");
    }
    println!("не додавати: {}", skipped.len());
    for (t, why) in &skipped {
        println!("   {t:<14} {why}");
    }
}
