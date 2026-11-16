//! Прогін реального логу (`--log-words`) через поточний детектор: що змінилось у рішеннях
//! і скільки ручних виправлень стали б автоматичними.
//!   cargo run --release --example replay -- layout-switcher.log
use std::collections::BTreeMap;

use layout_switcher::detect::{Dicts, Verdict};
use layout_switcher::keymap::{convert_text, keys_for, Lang};
use layout_switcher::privacy::looks_secret;

fn lang(s: &str) -> Option<Lang> {
    match s {
        "En" => Some(Lang::En),
        "Uk" => Some(Lang::Uk),
        _ => None,
    }
}

fn main() {
    let path = std::env::args().nth(1).expect("usage: replay <log>");
    let d = Dicts::load().expect("dict/");
    let text = std::fs::read_to_string(path).expect("read log");

    let mut changes: BTreeMap<(String, String), Vec<String>> = BTreeMap::new();
    let (mut words, mut same) = (0, 0);
    let mut retexted: Vec<String> = Vec::new();
    let mut secrets = 0;
    let (mut sel_total, mut sel_auto) = (0, 0);
    let mut sel_missed = Vec::new();

    for line in text.lines() {
        let mut parts = line.split('\t');
        let (_ts, ev) = (parts.next(), parts.next().unwrap_or(""));
        let f: BTreeMap<&str, &str> = parts.filter_map(|kv| kv.split_once('=')).collect();

        if ev == "word" {
            let (Some(cur), Some(typed), Some(old)) = (f.get("cur").and_then(|s| lang(s)), f.get("typed"), f.get("verdict")) else {
                continue;
            };
            if looks_secret(typed) || f.get("other").map_or(false, |o| looks_secret(o)) {
                secrets += 1; // такий токен нова версія в лог не запише; тут його теж не показуємо
                continue;
            }
            let Some(keys) = keys_for(typed, cur) else { continue };
            let ctx = f.get("ctx").and_then(|s| lang(s));
            let new = d.verdict(&keys, cur, ctx);
            words += 1;
            // те саме рішення «виправити», але ІНШИЙ текст (напр. розділові знаки): у лозі `other` — старий текст
            if let (Verdict::Fix(fx), "fixed", Some(was)) = (&new, *old, f.get("other")) {
                if fx.text != *was {
                    retexted.push(format!("{typed} -> було {was}, стало {}", fx.text));
                }
            }
            if new.tag() == *old {
                same += 1;
            } else {
                let shown = match &new {
                    Verdict::Fix(fx) => format!("{typed} -> {}", fx.text),
                    _ => typed.to_string(),
                };
                changes.entry((old.to_string(), new.tag().to_string())).or_default().push(shown);
            }
        } else if ev == "manual" && f.get("kind") == Some(&"selection") {
            let Some(src) = f.get("text") else { continue };
            for tok in src.split_whitespace() {
                let Some(conv) = convert_text(tok, None) else { continue };
                sel_total += 1;
                let auto = keys_for(tok, conv.from)
                    .map(|k| d.verdict(&k, conv.from, None))
                    .map(|v| matches!(&v, Verdict::Fix(fx) if fx.text == conv.text))
                    .unwrap_or(false);
                if auto {
                    sel_auto += 1;
                } else {
                    sel_missed.push(format!("{tok} -> {}", conv.text));
                }
            }
        }
    }

    println!("слів у логу: {words}; рішення не змінились: {same}");
    if secrets > 0 {
        println!("токенів, схожих на пароль (нова версія їх у лог не пише; тут пропущено): {secrets}");
    }
    for ((old, new), v) in &changes {
        println!("\n{old} -> {new}: {}", v.len());
        for s in v.iter().take(40) {
            println!("   {s}");
        }
    }
    if !retexted.is_empty() {
        println!("\nвиправлення з іншим текстом, ніж тоді: {}", retexted.len());
        for s in &retexted {
            println!("   {s}");
        }
    }
    println!("\nручні виправлення виділеного: слів {sel_total}, тепер було б автоматично: {sel_auto}");
    for s in &sel_missed {
        println!("   і далі вручну: {s}");
    }
}
