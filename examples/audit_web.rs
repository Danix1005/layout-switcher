//! Аудит правила «адреса сайту чи пошти»: які справжні українські слова (частотний список і всі словоформи
//! морфологічного індексу), набрані в українській розкладці, детектор переробив би на «адресу».
//!   cargo run --release --example audit_web
use fst::{IntoStreamer, Streamer};
use layout_switcher::detect::{Dicts, Verdict};
use layout_switcher::keymap::{keys_for, Lang};

fn main() {
    let d = Dicts::load().expect("dict/ не знайдено — запусти з теки проєкту");
    let mut words: Vec<String> = std::fs::read_to_string("dict/uk.txt")
        .unwrap_or_default()
        .lines()
        .filter_map(|l| l.split_whitespace().next())
        .map(str::to_lowercase)
        .collect();
    if let Ok(bytes) = std::fs::read("dict/uk_forms.fst") {
        let set = fst::Set::new(bytes).expect("fst");
        let mut st = set.into_stream();
        while let Some(w) = st.next() {
            words.push(String::from_utf8_lossy(w).into_owned());
        }
    }
    let (mut total, mut hits) = (0usize, Vec::new());
    for w in &words {
        if !w.contains('ю') {
            continue;
        }
        let Some(keys) = keys_for(w, Lang::Uk) else { continue };
        total += 1;
        if let Verdict::Fix(f) = d.verdict(&keys, Lang::Uk, None) {
            if f.text.contains('.') || f.text.contains('@') {
                hits.push(format!("{w} -> {}", f.text));
            }
        }
    }
    println!("слів із «ю»: {total}; стали б адресою: {}", hits.len());
    for h in hits.iter().take(50) {
        println!("   {h}");
    }
}
