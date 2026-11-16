//! Розгортає hunspell-словник (.dic + .aff) у список усіх словоформ (по одній на рядок, може бути з дублями).
//!   cargo run --release --example expand_hunspell -- uk_UA.dic uk_UA.aff > forms_raw.txt
//!
//! Чому не `unmunch`: він пропускає частину прапорців (у uk_UA, наприклад, теперішній час
//! «відображається» лишається нерозгорнутим). Тут: однолітерні прапорці, суфікси й префікси,
//! умови-класи `[^...]`, `.`, перехресне поєднання префікса й суфікса (cross product).
use std::collections::HashMap;
use std::fs;
use std::io::{BufWriter, Write};

enum Tok {
    Any,
    Set { neg: bool, chars: Vec<char> },
}

struct Rule {
    strip: Vec<char>,
    add: String,
    cond: Vec<Tok>,
}

struct Group {
    suffix: bool,
    cross: bool,
    rules: Vec<Rule>,
}

fn parse_cond(s: &str) -> Vec<Tok> {
    let cs: Vec<char> = s.chars().collect();
    let mut out = Vec::new();
    let mut i = 0;
    while i < cs.len() {
        match cs[i] {
            '.' => {
                out.push(Tok::Any);
                i += 1;
            }
            '[' => {
                let mut j = i + 1;
                let neg = cs.get(j) == Some(&'^');
                if neg {
                    j += 1;
                }
                let mut chars = Vec::new();
                while j < cs.len() && cs[j] != ']' {
                    chars.push(cs[j]);
                    j += 1;
                }
                out.push(Tok::Set { neg, chars });
                i = j + 1;
            }
            c => {
                out.push(Tok::Set { neg: false, chars: vec![c] });
                i += 1;
            }
        }
    }
    out
}

fn tok_ok(t: &Tok, c: char) -> bool {
    match t {
        Tok::Any => true,
        Tok::Set { neg, chars } => chars.contains(&c) != *neg,
    }
}

fn cond_suffix(w: &[char], cond: &[Tok]) -> bool {
    w.len() >= cond.len() && cond.iter().enumerate().all(|(i, t)| tok_ok(t, w[w.len() - cond.len() + i]))
}

fn cond_prefix(w: &[char], cond: &[Tok]) -> bool {
    w.len() >= cond.len() && cond.iter().enumerate().all(|(i, t)| tok_ok(t, w[i]))
}

/// Застосувати суфіксні правила групи до слова.
fn apply_sfx(g: &Group, w: &[char], out: &mut Vec<String>) {
    for r in &g.rules {
        if w.len() >= r.strip.len() && w[w.len() - r.strip.len()..] == r.strip[..] && cond_suffix(w, &r.cond) {
            let mut s: String = w[..w.len() - r.strip.len()].iter().collect();
            s.push_str(&r.add);
            out.push(s);
        }
    }
}

fn apply_pfx(g: &Group, w: &[char], out: &mut Vec<String>) {
    for r in &g.rules {
        if w.len() >= r.strip.len() && w[..r.strip.len()] == r.strip[..] && cond_prefix(w, &r.cond) {
            let mut s = r.add.clone();
            s.extend(w[r.strip.len()..].iter());
            out.push(s);
        }
    }
}

fn main() {
    let mut args = std::env::args().skip(1);
    let (Some(dic), Some(aff)) = (args.next(), args.next()) else {
        eprintln!("usage: expand_hunspell <x.dic> <x.aff> > forms.txt");
        std::process::exit(2);
    };

    // ── .aff
    let mut groups: HashMap<char, Group> = HashMap::new();
    for line in fs::read_to_string(&aff).expect("aff").lines() {
        let p: Vec<&str> = line.split_whitespace().collect();
        if p.len() < 4 || !(p[0] == "SFX" || p[0] == "PFX") {
            continue;
        }
        let suffix = p[0] == "SFX";
        let flag = p[1].chars().next().unwrap();
        if p.len() == 4 && (p[2] == "Y" || p[2] == "N") && p[3].parse::<usize>().is_ok() {
            groups.insert(flag, Group { suffix, cross: p[2] == "Y", rules: Vec::new() });
        } else if let Some(g) = groups.get_mut(&flag) {
            let strip: Vec<char> = if p[2] == "0" { vec![] } else { p[2].chars().collect() };
            let add_raw = if p[3] == "0" { "" } else { p[3] };
            let add = add_raw.split('/').next().unwrap_or("").to_string(); // продовжувальних прапорців у uk_UA нема
            let cond = parse_cond(p.get(4).copied().unwrap_or("."));
            g.rules.push(Rule { strip, add, cond });
        }
    }
    eprintln!("прапорців у .aff: {}", groups.len());

    // ── .dic
    let text = fs::read_to_string(&dic).expect("dic");
    let stdout = std::io::stdout();
    let mut out = BufWriter::new(stdout.lock());
    let (mut stems, mut forms) = (0u64, 0u64);
    for line in text.lines().skip(1) {
        let first = line.split_whitespace().next().unwrap_or("");
        if first.is_empty() {
            continue;
        }
        let (word, flags) = match first.split_once('/') {
            Some((w, f)) => (w, f),
            None => (first, ""),
        };
        let w: Vec<char> = word.chars().collect();
        stems += 1;
        let mut all: Vec<String> = vec![word.to_string()];
        let mut sfx_cross: Vec<String> = Vec::new();
        let mut pfx_groups: Vec<&Group> = Vec::new();
        for f in flags.chars() {
            let Some(g) = groups.get(&f) else { continue };
            if g.suffix {
                let mut v = Vec::new();
                apply_sfx(g, &w, &mut v);
                if g.cross {
                    sfx_cross.extend(v.iter().cloned());
                }
                all.extend(v);
            } else {
                pfx_groups.push(g);
            }
        }
        for g in pfx_groups {
            let mut v = Vec::new();
            apply_pfx(g, &w, &mut v);
            all.extend(v);
            if g.cross {
                for s in &sfx_cross {
                    let sc: Vec<char> = s.chars().collect();
                    let mut v2 = Vec::new();
                    apply_pfx(g, &sc, &mut v2);
                    all.extend(v2);
                }
            }
        }
        for s in all {
            forms += 1;
            writeln!(out, "{s}").unwrap();
        }
    }
    eprintln!("основ: {stems}, форм (з дублями): {forms}");
}
