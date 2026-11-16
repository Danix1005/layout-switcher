//! Пам'ять про перебіг набору в одному вікні: якою мовою людина пише і чи перемикати розкладку після виправлення.
//!
//! Чиста логіка (без Windows): її використовує і програма, і `examples/simulate` — прогін реальних логів.
//!
//! Головне правило — про **вкраплені слова**. Якщо людина пише українською (кілька останніх слів — українські,
//! набрані в українській розкладці) і раптом набирає `sftp`, не перемикаючись, то текст слова ми виправляємо, а
//! розкладку НЕ чіпаємо: далі вона знову писатиме українською. Якщо ж і наступне слово виявилось англійським —
//! це вже не вкраплення, а зміна мови: тоді перемикаємо.
//!
//! Підстава — три логи користувача. У третьому після 19 виправлень у бік англійської наступним словом
//! 12 разів було українське (розкладку доводилось одразу перемикати назад) і лише 7 разів англійське.
//! `examples/simulate` на всіх трьох логах: автовиправлень 131 -> 96, перемикань розкладки 131 -> 49,
//! слів «не тією мовою» стільки ж (4), хибних виправлень 0.
//!
//! У зворотний бік (українське слово посеред англійського тексту) типово перемикаємо одразу: українська тут —
//! основна мова, і таке слово майже завжди означає «я повернувся до української».

use std::collections::VecDeque;

use crate::detect::{Dicts, Verdict};
use crate::keymap::{Key, Lang};

/// Скільки останніх слів пам'ятаємо.
const RECENT: usize = 6;
/// Після такої паузи (мс) попередній контекст застарів.
const STALE_MS: u64 = 120_000;

/// Коли вважати виправлене слово «вкрапленим» (текст виправити, розкладку лишити).
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum Embed {
    /// Ніколи: після кожного виправлення перемикати розкладку (поведінка до 0.7).
    Off,
    /// Лише англійське слово посеред українського тексту.
    #[default]
    IntoEnglish,
    /// В обох напрямках.
    Both,
}

impl Embed {
    pub fn parse(s: &str) -> Option<Embed> {
        match s {
            "always" => Some(Embed::Off),
            "smart" => Some(Embed::IntoEnglish),
            "smart-both" => Some(Embed::Both),
            _ => None,
        }
    }

    pub fn tag(self) -> &'static str {
        match self {
            Embed::Off => "always",
            Embed::IntoEnglish => "smart",
            Embed::Both => "smart-both",
        }
    }

    fn allows(self, to: Lang) -> bool {
        match self {
            Embed::Off => false,
            Embed::IntoEnglish => to == Lang::En,
            Embed::Both => true,
        }
    }
}

/// Що вирішено про щойно завершене слово.
pub struct Outcome {
    pub verdict: Verdict,
    /// На яку розкладку перемкнутись після виправлення. `None` — розкладку не чіпати.
    pub switch_to: Option<Lang>,
    /// Слово виправлено як вкраплене: текст змінюємо, розкладка лишається.
    pub embedded: bool,
    /// Контекст, з яким оцінювали слово (для логу).
    pub ctx: Option<Lang>,
    /// Розкладка виявилась не тією, яку ми лишили: користувач перемкнув її сам.
    pub switched_by_user: bool,
}

#[derive(Default)]
pub struct Session {
    mode: Embed,
    /// Мови останніх слів, набраних (або виправлених із перемиканням) як слід.
    recent: VecDeque<Lang>,
    /// Якою має бути розкладка після попереднього слова.
    expected: Option<Lang>,
    /// Мова щойно виправленого вкрапленого слова — щоб помітити друге таке поспіль.
    streak: Option<Lang>,
    last_word_ms: Option<u64>,
}

impl Session {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn with_mode(mode: Embed) -> Self {
        Session { mode, ..Self::default() }
    }

    /// Нове вікно: попередній текст до нього не стосується.
    pub fn reset(&mut self) {
        *self = Self::with_mode(self.mode);
    }

    /// Мова, якою набрано щонайменше 70 % останніх слів, якщо їх не менше `min`.
    fn dominant(&self, min: usize) -> Option<Lang> {
        let total = self.recent.len();
        if total < min || total == 0 {
            return None;
        }
        let uk = self.recent.iter().filter(|&&l| l == Lang::Uk).count();
        let en = total - uk;
        if uk * 10 >= total * 7 {
            Some(Lang::Uk)
        } else if en * 10 >= total * 7 {
            Some(Lang::En)
        } else {
            None
        }
    }

    /// Контекст для детектора: мова останніх слів (потрібно щонайменше три).
    pub fn ctx(&self) -> Option<Lang> {
        self.dominant(3)
    }

    fn push(&mut self, lang: Lang) {
        if self.recent.len() == RECENT {
            self.recent.pop_front();
        }
        self.recent.push_back(lang);
    }

    /// Слово завершено (пробіл/Enter/Tab). `cur` — розкладка, в якій воно набране; `now_ms` — довільний лічильник часу.
    pub fn on_word(&mut self, dicts: &Dicts, keys: &[Key], cur: Lang, now_ms: u64) -> Outcome {
        if self.last_word_ms.map_or(false, |t| now_ms.saturating_sub(t) > STALE_MS) {
            self.recent.clear();
            self.streak = None;
        }
        self.last_word_ms = Some(now_ms);

        // Розкладка не та, якою мала б бути після попереднього слова, — користувач перемкнув її сам і знає,
        // що робить: попередні слова більше не підказка (інакше `the` після українського тексту стало б «еру»).
        let switched_by_user = self.expected.map_or(false, |e| e != cur);
        if switched_by_user {
            self.recent.clear();
            self.streak = None;
        }

        let ctx = self.ctx();
        let verdict = dicts.verdict(keys, cur, ctx);
        let (mut switch_to, mut embedded) = (None, false);
        match &verdict {
            Verdict::KnownWord => {
                self.push(cur);
                self.streak = None;
                self.expected = Some(cur);
            }
            Verdict::Fix(fix) => {
                let to = fix.to;
                // Людина пише мовою поточної розкладки, і це перше «чуже» слово поспіль — вкраплення.
                if self.mode.allows(to) && self.dominant(2) == Some(cur) && self.streak != Some(to) {
                    embedded = true;
                    self.streak = Some(to);
                    self.expected = Some(cur);
                } else {
                    if self.streak == Some(to) {
                        self.push(to); // попереднє вкраплене слово теж було цією мовою
                    }
                    self.push(to);
                    self.streak = None;
                    self.expected = Some(to);
                    switch_to = Some(to);
                }
            }
            // невідоме слово, одна літера, число: нічого не доводить і нічого не скасовує
            _ => self.expected = Some(cur),
        }
        Outcome { verdict, switch_to, embedded, ctx, switched_by_user }
    }

    /// Користувач сам переніс слово в розкладку `to` (подвійний Shift): це найпевніший сигнал про мову.
    pub fn on_manual(&mut self, to: Lang) {
        self.push(to);
        self.streak = None;
        self.expected = Some(to);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::keymap::keys_for;

    fn dicts() -> Dicts {
        Dicts::from_words(
            ["sftp", "ssh", "admin", "user", "hello", "world", "please", "open", "the", "file"],
            ["папки", "через", "для", "потрібно", "знову", "привіт", "добре", "тепер", "треба"],
        )
    }

    /// Набрати слова по черзі; розкладка змінюється лише тоді, коли так вирішила сесія.
    fn run(words: &[&str], start: Lang) -> (Vec<String>, Lang) {
        run_mode(words, start, Embed::default())
    }

    fn run_mode(words: &[&str], start: Lang, mode: Embed) -> (Vec<String>, Lang) {
        let d = dicts();
        let mut s = Session::with_mode(mode);
        let mut layout = start;
        let mut out = Vec::new();
        for (i, w) in words.iter().enumerate() {
            // слово задано так, як його бачить той, хто друкує наосліп: клавіші беремо з «правильної» мови
            let lang = if w.chars().any(|c| c.is_ascii_alphabetic()) { Lang::En } else { Lang::Uk };
            let keys = keys_for(w, lang).unwrap();
            let o = s.on_word(&d, &keys, layout, i as u64 * 1000);
            out.push(match &o.verdict {
                Verdict::Fix(f) => format!("{}{}", f.text, if o.embedded { "*" } else { "!" }),
                _ => crate::keymap::render(&keys, layout),
            });
            if let Some(l) = o.switch_to {
                layout = l;
            }
        }
        (out, layout)
    }

    #[test]
    fn english_term_inside_ukrainian_text_is_fixed_without_switching() {
        // «* » — виправлено як вкраплення (розкладка лишилась), «!» — виправлено з перемиканням
        let (out, layout) = run(&["папки", "через", "sftp", "для", "потрібно"], Lang::Uk);
        assert_eq!(out, ["папки", "через", "sftp*", "для", "потрібно"]);
        assert_eq!(layout, Lang::Uk);
    }

    #[test]
    fn second_foreign_word_in_a_row_switches_the_layout() {
        let (out, layout) = run(&["папки", "через", "please", "open", "the", "file"], Lang::Uk);
        assert_eq!(out, ["папки", "через", "please*", "open!", "the", "file"]);
        assert_eq!(layout, Lang::En);
    }

    #[test]
    fn without_context_the_first_fix_switches_at_once() {
        // нове вікно, розкладка англійська, людина пише українською
        let (out, layout) = run(&["привіт", "добре", "тепер"], Lang::En);
        assert_eq!(out, ["привіт!", "добре", "тепер"]);
        assert_eq!(layout, Lang::Uk);
    }

    #[test]
    fn ukrainian_word_after_real_english_switches_at_once() {
        // англійською писали як слід, далі українською, не перемкнувшись: це повернення до основної мови
        let (out, layout) = run(&["hello", "world", "знову", "треба", "тепер"], Lang::En);
        assert_eq!(out, ["hello", "world", "знову!", "треба", "тепер"]);
        assert_eq!(layout, Lang::Uk);
    }

    #[test]
    fn modes() {
        // always: після кожного виправлення перемикаємо (як до 0.7)
        let (out, layout) = run_mode(&["папки", "через", "sftp", "для"], Lang::Uk, Embed::Off);
        assert_eq!(out, ["папки", "через", "sftp!", "для!"]);
        assert_eq!(layout, Lang::Uk);
        // smart-both: вкраплення в обох напрямках
        let (out, layout) = run_mode(&["hello", "world", "знову", "треба", "тепер"], Lang::En, Embed::Both);
        assert_eq!(out, ["hello", "world", "знову*", "треба!", "тепер"]);
        assert_eq!(layout, Lang::Uk);
        for m in [Embed::Off, Embed::IntoEnglish, Embed::Both] {
            assert_eq!(Embed::parse(m.tag()), Some(m));
        }
        assert_eq!(Embed::parse("sometimes"), None);
    }

    #[test]
    fn mode_survives_a_window_change() {
        let mut s = Session::with_mode(Embed::Off);
        s.reset();
        let d = dicts();
        let k = |w: &str, l| keys_for(w, l).unwrap();
        s.on_word(&d, &k("папки", Lang::Uk), Lang::Uk, 0);
        s.on_word(&d, &k("через", Lang::Uk), Lang::Uk, 1000);
        let o = s.on_word(&d, &k("sftp", Lang::En), Lang::Uk, 2000);
        assert!(!o.embedded);
        assert_eq!(o.switch_to, Some(Lang::En));
    }

    #[test]
    fn manual_layout_switch_by_the_user_resets_context() {
        let d = dicts();
        let mut s = Session::new();
        let k = |w: &str, l| keys_for(w, l).unwrap();
        s.on_word(&d, &k("папки", Lang::Uk), Lang::Uk, 0);
        s.on_word(&d, &k("через", Lang::Uk), Lang::Uk, 1000);
        s.on_word(&d, &k("для", Lang::Uk), Lang::Uk, 2000);
        assert_eq!(s.ctx(), Some(Lang::Uk));
        // користувач сам перемкнувся на англійську й набирає англійське слово
        let o = s.on_word(&d, &k("hello", Lang::En), Lang::En, 3000);
        assert!(o.switched_by_user);
        assert_eq!(o.ctx, None);
        assert!(matches!(o.verdict, Verdict::KnownWord));
    }

    #[test]
    fn unknown_word_between_two_foreign_words_does_not_break_the_streak() {
        let (out, layout) = run(&["папки", "через", "sftp", "qwrtx", "ssh"], Lang::Uk);
        // «qwrtx» набрано в українській розкладці й невідоме обом словникам — лишається як є
        assert_eq!(out, ["папки", "через", "sftp*", "йцкеч", "ssh!"]);
        assert_eq!(layout, Lang::En);
    }

    #[test]
    fn context_goes_stale_after_a_long_pause() {
        let d = dicts();
        let mut s = Session::new();
        let k = |w: &str, l| keys_for(w, l).unwrap();
        s.on_word(&d, &k("папки", Lang::Uk), Lang::Uk, 0);
        s.on_word(&d, &k("через", Lang::Uk), Lang::Uk, 1000);
        // через три хвилини — нова думка: перше ж англійське слово перемикає розкладку
        let o = s.on_word(&d, &k("sftp", Lang::En), Lang::Uk, 200_000);
        assert!(!o.embedded);
        assert_eq!(o.switch_to, Some(Lang::En));
    }

    #[test]
    fn manual_fix_counts_as_a_word_of_that_language() {
        let mut s = Session::new();
        s.on_manual(Lang::En);
        s.on_manual(Lang::En);
        s.on_manual(Lang::En);
        assert_eq!(s.ctx(), Some(Lang::En));
    }
}
