//! Симуляція виправлення посеред слова (`Dicts::early_fix`) на логах користувача (`--log-words`):
//! скільки слів виправилось би раніше, на якій літері, і чи не зачепило б щось зайве.
//!   cargo run --release --example simulate_early -- layout-switcher.log [ще.log…]
use std::collections::BTreeMap;

use layout_switcher::detect::Dicts;
use layout_switcher::keymap::{keys_for, Lang};

fn lang(s: &str) -> Option<Lang> {
    match s {
        "En" => Some(Lang::En),
        "Uk" => Some(Lang::Uk),
        _ => None,
    }
}

fn main() {
    let d = Dicts::load().expect("dict/ не знайдено — запусти з теки проєкту");
    let (mut fixed, mut fixed_early, mut wrong_target, mut bad) = (0, 0, 0, Vec::new());
    let mut at: BTreeMap<usize, usize> = BTreeMap::new();
    let (mut manual_mid, mut manual_mid_early) = (0, 0);
    for path in std::env::args().skip(1) {
        let text = std::fs::read_to_string(&path).expect("лог");
        for line in text.lines() {
            let cols: Vec<&str> = line.split('\t').collect();
            if cols.len() < 3 {
                continue;
            }
            let f: BTreeMap<&str, &str> = cols[2..].iter().filter_map(|c| c.split_once('=')).collect();
            let ctx = f.get("ctx").and_then(|s| lang(s));
            match cols[1] {
                "word" => {
                    let (Some(cur), Some(typed)) = (f.get("cur").and_then(|s| lang(s)), f.get("typed")) else { continue };
                    let Some(keys) = keys_for(typed, cur) else { continue };
                    let verdict = f.get("verdict").copied().unwrap_or("");
                    let first = (3..=keys.len()).find_map(|n| d.early_fix(&keys[..n], cur, ctx).map(|fx| (n, fx)));
                    if verdict == "fixed" {
                        fixed += 1;
                        if let Some((n, fx)) = &first {
                            fixed_early += 1;
                            *at.entry(*n).or_default() += 1;
                            if Some(fx.to) != f.get("to").and_then(|s| lang(s)) && f.get("emb").is_none() {
                                wrong_target += 1;
                            }
                        }
                    } else if let Some((n, fx)) = first {
                        bad.push(format!("{verdict:8} {typed} -> на {n}-й літері «{}»", fx.text));
                    }
                }
                "manual" if f.get("kind") == Some(&"word") => {
                    let (Some(shown), Some(typed)) = (f.get("shown").and_then(|s| lang(s)), f.get("typed")) else { continue };
                    let Some(keys) = keys_for(typed, shown) else { continue };
                    manual_mid += 1;
                    if (3..=keys.len()).any(|n| d.early_fix(&keys[..n], shown, ctx).is_some()) {
                        manual_mid_early += 1;
                    }
                }
                _ => {}
            }
        }
    }
    println!("виправлених пробілом слів: {fixed}; з них виправилось би посеред слова: {fixed_early}");
    println!("   на якій літері: {at:?}; інша мова, ніж у виправлення пробілом: {wrong_target}");
    println!("ручних Shift посеред слова: {manual_mid}; програма встигла б сама: {manual_mid_early}");
    println!("спрацювало б на словах, які пробіл НЕ виправляв: {}", bad.len());
    for b in &bad {
        println!("   {b}");
    }
}
