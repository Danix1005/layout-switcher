//! Порівняння правил перемикання розкладки на реальних логах (`--log-words`).
//!   cargo run --release --example simulate -- log1.log log2.log …
//!
//! Модель: людина друкує «наосліп» ті самі клавіші, що й у лозі, і сама розкладку НЕ перемикає — усе робить
//! перемикач. Для кожного слова з'ясовуємо, якою мовою воно задумане (за поточним детектором, без контексту),
//! і проганяємо весь текст через кожне правило, ведучи «уявну» розкладку:
//!   always      — після кожного виправлення перемикати розкладку (як було до 0.7);
//!   smart       — англійське слово посеред українського тексту виправляти, не перемикаючи;
//!   smart-both  — те саме в обох напрямках.
//! Рахуємо: скільки слів лишилось би на екрані не тією мовою, скільки було автовиправлень і перемикань
//! розкладки, і скільки окремих розділових знаків вийшли б не ті. Що людина хотіла надрукувати знаком, із
//! логу не видно, тому рахуємо за двома припущеннями: знак «мовою попереднього слова» і знак
//! «по-українськи» (користувач, який пише переважно українською, тисне Shift+7 для «?» у будь-якій розкладці).
use std::collections::BTreeMap;

use layout_switcher::detect::{Dicts, Verdict};
use layout_switcher::keymap::{keys_for, render, Key, Lang};
use layout_switcher::session::{Embed, Session};

fn lang(s: &str) -> Option<Lang> {
    match s {
        "En" => Some(Lang::En),
        "Uk" => Some(Lang::Uk),
        _ => None,
    }
}

struct Word {
    t: u64,
    keys: Vec<Key>,
    /// Якою мовою слово задумане; `None` — незрозуміло (невідоме слово, знак, число).
    intent: Option<Lang>,
}

#[derive(Default, Clone, Copy)]
struct Score {
    words: u32,
    wrong: u32,
    /// Знак не той, якщо вважати, що його набирали мовою попереднього слова.
    punct_prev: u32,
    /// Знак не той, якщо вважати, що знаки завжди набирають «по-українськи».
    punct_uk: u32,
    false_fix: u32,
    fixes: u32,
    switches: u32,
}

fn run(d: &Dicts, windows: &[(Lang, Vec<Word>)], mode: Embed) -> Score {
    let mut sc = Score::default();
    for (start, words) in windows {
        let mut s = Session::with_mode(mode);
        let mut layout = *start;
        let mut carried: Option<Lang> = None; // мова останнього зрозумілого слова — для розділових знаків
        for w in words {
            let o = s.on_word(d, &w.keys, layout, w.t);
            let fixed_to = match &o.verdict {
                Verdict::Fix(f) => Some(f.to),
                _ => None,
            };
            match w.intent {
                Some(want) => {
                    sc.words += 1;
                    let shown = fixed_to.unwrap_or(layout);
                    if shown != want {
                        if fixed_to.is_some() {
                            sc.false_fix += 1;
                        } else {
                            sc.wrong += 1;
                        }
                    }
                    carried = Some(want);
                }
                None => {
                    // окремий знак: «не той», якщо в задуманій розкладці ці клавіші дали б інше
                    let shown = render(&w.keys, fixed_to.unwrap_or(layout));
                    let is_punct = |l: Lang| render(&w.keys, l).chars().all(|c| !c.is_alphanumeric());
                    if is_punct(Lang::Uk) && is_punct(Lang::En) {
                        if carried.map_or(false, |want| shown != render(&w.keys, want)) {
                            sc.punct_prev += 1;
                        }
                        if shown != render(&w.keys, Lang::Uk) {
                            sc.punct_uk += 1;
                        }
                    }
                }
            }
            if fixed_to.is_some() {
                sc.fixes += 1;
            }
            if let Some(l) = o.switch_to {
                layout = l;
                sc.switches += 1;
            }
        }
    }
    sc
}

fn main() {
    let files: Vec<String> = std::env::args().skip(1).collect();
    if files.is_empty() {
        eprintln!("usage: simulate <log> [<log>…]");
        std::process::exit(2);
    }
    let d = Dicts::load().expect("dict/ не знайдено — запусти з теки проєкту");
    let mut total: BTreeMap<&'static str, Score> = BTreeMap::new();

    for path in &files {
        let text = std::fs::read_to_string(path).expect("не можу прочитати лог");
        // вікна: від події fg до наступної; початкова розкладка — та, що була в лозі на першому слові
        let mut windows: Vec<(Lang, Vec<Word>)> = Vec::new();
        let mut open = false;
        for line in text.lines() {
            let mut parts = line.split('\t');
            let (ts, ev) = (parts.next().unwrap_or(""), parts.next().unwrap_or(""));
            if ev == "fg" || ev == "start" {
                open = false;
                continue;
            }
            if ev != "word" {
                continue;
            }
            let f: BTreeMap<&str, &str> = parts.filter_map(|kv| kv.split_once('=')).collect();
            let (Some(cur), Some(typed)) = (f.get("cur").and_then(|s| lang(s)), f.get("typed")) else { continue };
            let Some(keys) = keys_for(typed, cur) else { continue };
            let intent = match d.verdict(&keys, cur, None) {
                Verdict::KnownWord => Some(cur),
                Verdict::Fix(fx) => Some(fx.to),
                _ => None,
            };
            if !open {
                windows.push((cur, Vec::new()));
                open = true;
            }
            windows.last_mut().unwrap().1.push(Word { t: ts.parse().unwrap_or(0), keys, intent });
        }

        println!("\n{path}: вікон {}, слів {}", windows.len(), windows.iter().map(|w| w.1.len()).sum::<usize>());
        println!("  {:<11} {:>5} {:>14} {:>11} {:>14} {:>11} {:>18} {:>16}", "правило", "слів", "не тією мовою", "хибні випр.", "автовиправлень", "перемикань", "знаки(за сусідом)", "знаки(по-укр.)");
        for mode in [Embed::Off, Embed::IntoEnglish, Embed::Both] {
            let s = run(&d, &windows, mode);
            println!("  {:<11} {:>5} {:>14} {:>11} {:>14} {:>11} {:>18} {:>16}", mode.tag(), s.words, s.wrong, s.false_fix, s.fixes, s.switches, s.punct_prev, s.punct_uk);
            let t = total.entry(mode.tag()).or_default();
            t.words += s.words;
            t.wrong += s.wrong;
            t.punct_prev += s.punct_prev;
            t.punct_uk += s.punct_uk;
            t.false_fix += s.false_fix;
            t.fixes += s.fixes;
            t.switches += s.switches;
        }
    }
    if files.len() > 1 {
        println!("\nразом:");
        for mode in [Embed::Off, Embed::IntoEnglish, Embed::Both] {
            let s = total[mode.tag()];
            println!("  {:<11} {:>5} {:>14} {:>11} {:>14} {:>11} {:>18} {:>16}", mode.tag(), s.words, s.wrong, s.false_fix, s.fixes, s.switches, s.punct_prev, s.punct_uk);
        }
    }
}
