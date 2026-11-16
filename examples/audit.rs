//! Аудит хибних спрацювань: які ПРАВИЛЬНО набрані слова детектор перетворив би на іншу мову.
//!   cargo run --release --example audit            (з теки проєкту, де лежить dict/)
//!   cargo run --release --example audit -- 40000   (перевіряти слова з рангом до 40000; типово 30000)
//!   cargo run --release --example audit -- 30000 my_terms.txt   (ще й свій список англ. термінів, по слову на рядок)
//!
//! Для кожного слова з частотних списків (dict/en.txt, dict/uk.txt) і з ІТ-лексики (dict/en_it.txt)
//! моделюємо «набрано у своїй розкладці» і дивимось вердикт без контексту та з контекстом іншої мови.
//! Усе, що тут з'являється без контексту, — це зіпсований текст у користувача, тому список має бути коротким.
use layout_switcher::detect::{Dicts, Verdict};
use layout_switcher::keymap::{keys_for, Lang};

fn words(path: &str) -> Vec<String> {
    std::fs::read_to_string(path)
        .unwrap_or_default()
        .lines()
        .filter(|l| !l.trim_start().starts_with('#'))
        .filter_map(|l| l.split_whitespace().next())
        .map(str::to_lowercase)
        .collect()
}

struct Flip {
    rank: usize,
    from: String,
    to: String,
}

fn scan(d: &Dicts, list: &[String], lang: Lang, limit: usize, ctx: Option<Lang>) -> (usize, Vec<Flip>) {
    let mut flips = Vec::new();
    let mut total = 0;
    for (rank, w) in list.iter().enumerate().take(limit) {
        if w.chars().count() < 2 {
            continue;
        }
        let Some(keys) = keys_for(w, lang) else { continue };
        total += 1;
        if let Verdict::Fix(f) = d.verdict(&keys, lang, ctx) {
            flips.push(Flip { rank, from: w.clone(), to: f.text });
        }
    }
    (total, flips)
}

fn report(title: &str, total: usize, flips: &[Flip], show: usize) {
    println!("\n{title}: {} із {total}", flips.len());
    for f in flips.iter().take(show) {
        println!("   #{:<6} {} -> {}", f.rank, f.from, f.to);
    }
    if flips.len() > show {
        println!("   … ще {}", flips.len() - show);
    }
}

fn main() {
    let limit: usize = std::env::args().nth(1).and_then(|s| s.parse().ok()).unwrap_or(30_000);
    let d = Dicts::load().expect("dict/ не знайдено — запусти з теки проєкту");
    let en = words("dict/en.txt");
    let uk = words("dict/uk.txt");
    let it = words("dict/en_it.txt");
    println!("перевіряю слова з рангом до {limit} (словоформ у морф. індексі: {})", d.forms_len());

    let (t, f) = scan(&d, &en, Lang::En, limit, None);
    report("англ. слова, набрані в EN, без контексту -> хибно в укр.", t, &f, 40);
    let (t, f) = scan(&d, &uk, Lang::Uk, limit, None);
    report("укр. слова, набрані в UK, без контексту -> хибно в англ.", t, &f, 40);
    let (t, f) = scan(&d, &it, Lang::En, usize::MAX, None);
    report("ІТ-терміни, набрані в EN, без контексту -> хибно в укр.", t, &f, 40);

    // Свій список термінів (другий аргумент): чи не зіпсує їх детектор, якщо набирати в EN.
    if let Some(path) = std::env::args().nth(2) {
        let mine = words(&path);
        let (t, f) = scan(&d, &mine, Lang::En, usize::MAX, None);
        report(&format!("{path}: набрані в EN, без контексту -> в укр."), t, &f, 200);
        let (t, f) = scan(&d, &mine, Lang::En, usize::MAX, Some(Lang::Uk));
        report(&format!("{path}: набрані в EN посеред українського тексту -> в укр."), t, &f, 200);
    }

    // З контекстом іншої мови (попередні слова були нею): тут спрацювання очікувані, але їх має бути небагато.
    let (t, f) = scan(&d, &en, Lang::En, limit, Some(Lang::Uk));
    report("англ. слова в EN посеред УКРАЇНСЬКОГО тексту -> в укр.", t, &f, 25);
    let (t, f) = scan(&d, &uk, Lang::Uk, limit, Some(Lang::En));
    report("укр. слова в UK посеред АНГЛІЙСЬКОГО тексту -> в англ.", t, &f, 25);
}
