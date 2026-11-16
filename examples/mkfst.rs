//! Генератор dict/uk_forms.fst із відсортованого списку словоформ (по одній на рядок).
//!   cargo run --release --example mkfst -- forms_clean.txt dict/uk_forms.fst
//! Вхід має бути унікальним і відсортованим побайтово (UTF-8 порядок = порядок кодових точок).
use std::fs::File;
use std::io::{BufRead, BufReader, BufWriter};

fn main() {
    let mut args = std::env::args().skip(1);
    let (Some(input), Some(output)) = (args.next(), args.next()) else {
        eprintln!("usage: mkfst <words.txt> <out.fst>");
        std::process::exit(2);
    };
    let reader = BufReader::new(File::open(&input).expect("open input"));
    let mut builder = fst::SetBuilder::new(BufWriter::new(File::create(&output).expect("create output")))
        .expect("builder");
    let mut n = 0u64;
    for line in reader.lines() {
        let line = line.expect("read");
        let w = line.trim();
        if w.is_empty() {
            continue;
        }
        builder.insert(w.as_bytes()).expect("insert (вхід має бути відсортований і унікальний)");
        n += 1;
    }
    builder.finish().expect("finish");
    println!("{n} слів -> {output}");
}
