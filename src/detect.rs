//! Детектор: чи схоже, що слово набрали не в тій розкладці.
//!
//! Правило консервативне. Конвертуємо, лише якщо ОДНОЧАСНО:
//!   1) слово, як воно надруковане, НЕ є словом поточної мови;
//!   2) те саме слово в іншій розкладці — Є словом тієї мови.
//!
//! Словники: англійський і український частотні списки (`en.txt`, `uk.txt`), а також
//! морфологічний індекс усіх українських словоформ (`uk_forms.fst`, ~3,4 млн форм).
//! Морфологічний індекс використовується для «відомих» слів будь-якої довжини, але як ЦІЛЬ
//! конвертації — лише для слів від 5 літер (для коротких випадкових збігів забагато).
//! Слова мають РАНГ частотності (0 = найчастіше). Коли «слово» є в обох мовах, дивимось, наскільки кожне з них
//! НАДІЙНЕ (`solid`): англ. — із першої 20-тисячки корпусу або додане явно; укр. — підтверджене морфологічним
//! словником або додане явно. Частотні списки зі субтитрів містять сміття (англ. слова, набрані не в тій розкладці,
//! російські слова), тому сама наявність у списку ще нічого не доводить.
//! `en_it.txt` — ІТ-лексика (cmd, dhcp, mtu…), якої нема в корпусі субтитрів.
//!
//! Слова з цифрами й дефісом (`KIT005`, `start-config`) розбираються по сегментах між цифрами/дефісами.

use std::collections::{BTreeMap, HashSet};
use std::io;
use std::ops::Bound;
use std::path::{Path, PathBuf};

use fst::{IntoStreamer, Set, Streamer};

use crate::keymap::{char_for, convert, is_tracked_key, render, Key, Lang};
use crate::privacy::looks_secret;

/// Звичайні слова (з основних словників) — від цієї довжини. Двобуквені — окремий режим нижче.
pub const MIN_LEN: usize = 3;

/// Слова, які користувач додав сам (`*_user.txt`): явний сигнал — вважаємо дуже частотними.
const RANK_USER: u32 = 500;
/// ІТ-лексика (`en_it.txt`).
const RANK_IT: u32 = 1500;
/// Слово з рангом від цього значення вважаємо «шумовим» (хвіст корпусу): його можна перебити частотнішим словом іншої мови.
const NOISE_RANK: u32 = 20_000;
/// Виправлення посеред слова: щонайменше стільки літер (менше — надто часто збігається випадково) і не більше
/// стільки (довге слово, яке досі «не впізнали», нехай перевірить пробіл).
const EARLY_MIN: usize = 3;
const EARLY_MAX: usize = 12;
/// Скільки слів з однаковим початком переглядати, шукаючи частотне (щоб не гальмувати хук).
const EARLY_SCAN: usize = 400;
/// Укр. частотний список шумніший за англ., зате довший: межа «частого» слова для виправлення посеред слова.
const EARLY_RANK_UK: u32 = 30_000;
/// У скільки разів частотнішим має бути слово іншої мови, щоб перебити «шумове»
/// (і наскільки рідшим воно може бути, щоб контекст усе ще вирішував на його користь).
const RANK_MARGIN: u32 = 4;

/// Двобуквені слова вирішуються НЕ основними словниками (там забагато шуму: `jr`, `ye`, `lt`
/// трапляються як «англійські слова», а в укр. корпусі повно російських `он`, `да`, `из`),
/// а двома короткими перевіреними списками. Правило: конвертуємо, лише якщо набране НЕ є
/// словом списку поточної мови, а результат — Є словом списку іншої мови.
/// Власні слова довжини 2 можна додати в dict/uk_user.txt / dict/en_user.txt.
///
/// Кілька пар є в ОБОХ списках (`AMBIGUOUS_SHORT`): «шт»/`in`, «см»/`cv`, «ша»/`if`, «св»/`cd`, «зс»/`pc`.
/// Обидва варіанти — звичайні слова, тому такі пари автоматично не чіпаємо в жоден бік (є подвійний Shift).
const SHORT_UK: &[&str] = &[
    "не", "що", "на", "це", "ти", "ми", "як", "за", "ви", "до", "ні", "ну", "то", "ще", "та", "по",
    "же", "її", "чи", "де", "от", "їх", "те", "бо", "зі", "із", "ці", "їй", "їм", "ті", "би", "об",
    "ой", "ох", "ах", "ух", "аж", "ок", "хм", "он", "ех",
    // ще часті слова
    "ця", "цю", "ту", "їв", "їж", "ай", "ей", "га", "ба",
    // скорочення, які інакше ПСУВАЛИСЯ б (їхній англ. вигляд — слово): 5 шт, 10 см, 100 гр, ЗС
    "шт", "см", "гр", "ша", "св", "зс",
];
const SHORT_EN: &[&str] = &[
    "to", "it", "of", "is", "in", "we", "me", "he", "my", "on", "do", "no", "be", "so", "go", "if",
    "up", "at", "oh", "as", "an", "us", "or", "by", "am", "ok", "hi", "ah", "um",
    // ІТ-скорочення: не конвертуємо їх, а набрані в UK-розкладці перетворюємо назад
    "tv", "pc", "ip", "id", "os", "ui", "ux", "vm", "qa", "hr", "pr", "ap", "ac", "dc", "cd", "ci",
    "js", "ts", "rm", "ls", "cp", "mv", "cv", "nm", "rx", "tx", "gw", "fw", "hw", "sw",
];
/// Пари (укр., англ.) на одних клавішах, де обидва варіанти — справжні слова. Перевіряється тестом.
#[cfg(test)]
const AMBIGUOUS_SHORT: &[(&str, &str)] = &[("шт", "in"), ("см", "cv"), ("ша", "if"), ("св", "cd"), ("зс", "pc")];

pub struct Dicts {
    /// слово -> ранг частотності (0 = найчастіше)
    /// (упорядковано — щоб швидко шукати слова за початком, див. `early_fix`)
    en: BTreeMap<String, u32>,
    uk: BTreeMap<String, u32>,
    short_en: HashSet<String>,
    short_uk: HashSet<String>,
    /// Усі українські словоформи (морфологічний словник); може бракувати — тоді працюємо без нього.
    forms: Option<Set<Vec<u8>>>,
    it_count: usize,
    /// Слова, додані явно (ІТ-лексика, особисті словники): лише вони перебивають слова з морфологічного індексу.
    explicit_en: HashSet<String>,
    explicit_uk: HashSet<String>,
}

/// Результат перевірки слова.
pub enum Verdict {
    /// Слово набране не в тій розкладці — ось що друкувати.
    Fix(Fix),
    /// Менше двох літер (однолітерні слова не чіпаємо).
    Short,
    /// Нема літер для перевірки (самі цифри/розділові знаки) або є заборонені клавіші (`=`, `\`, AltGr).
    NonWord,
    /// Є словом поточної мови — усе гаразд.
    KnownWord,
    /// Невідоме жодній мові (немає в словниках) — потенційна прогалина словника.
    Unknown,
}

impl Verdict {
    pub fn tag(&self) -> &'static str {
        match self {
            Verdict::Fix(_) => "fixed",
            Verdict::Short => "short",
            Verdict::NonWord => "nonword",
            Verdict::KnownWord => "known",
            Verdict::Unknown => "unknown",
        }
    }
}

pub struct Fix {
    /// Те, що треба надрукувати замість набраного (разом із розділовими знаками).
    pub text: String,
    /// У яку розкладку перемкнутись після виправлення.
    pub to: Lang,
}

impl Dicts {
    /// Слова беруться в порядку частотності: ранг = номер у списку.
    fn from_lists(en: Vec<String>, uk: Vec<String>) -> Self {
        let mut d = Dicts {
            en: BTreeMap::new(),
            uk: BTreeMap::new(),
            short_en: SHORT_EN.iter().map(|s| s.to_string()).collect(),
            short_uk: SHORT_UK.iter().map(|s| s.to_string()).collect(),
            forms: None,
            it_count: 0,
            explicit_en: HashSet::new(),
            explicit_uk: HashSet::new(),
        };
        for (i, w) in en.into_iter().enumerate() {
            d.en.entry(w).or_insert(i as u32);
        }
        for (i, w) in uk.into_iter().enumerate() {
            if w.contains('\'') {
                // у чатах апостроф часто пропускають: «зявляється», «привязуватись»
                d.uk.entry(w.replace('\'', "")).or_insert(i as u32);
            }
            d.uk.entry(w).or_insert(i as u32);
        }
        d
    }

    /// Підключити морфологічний індекс із байтів `.fst` (див. examples/mkfst.rs).
    pub fn set_forms(&mut self, bytes: Vec<u8>) -> io::Result<()> {
        let set = Set::new(bytes).map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e.to_string()))?;
        self.forms = Some(set);
        Ok(())
    }

    /// Для тестів: побудувати морфологічний індекс у пам'яті зі списку слів.
    pub fn with_forms_from<'a>(mut self, words: impl IntoIterator<Item = &'a str>) -> Self {
        let mut v: Vec<String> = words.into_iter().map(str::to_lowercase).collect();
        v.sort();
        v.dedup();
        self.forms = Some(Set::from_iter(v).expect("fst"));
        self
    }

    pub fn from_words<'a>(
        en: impl IntoIterator<Item = &'a str>,
        uk: impl IntoIterator<Item = &'a str>,
    ) -> Self {
        Self::from_lists(
            en.into_iter().map(str::to_lowercase).collect(),
            uk.into_iter().map(str::to_lowercase).collect(),
        )
    }

    /// Для тестів: слово частотного списку із заданим рангом (не «явне», на відміну від `add_word_ranked`).
    #[cfg(test)]
    fn with_rank(mut self, lang: Lang, word: &str, rank: u32) -> Self {
        match lang {
            Lang::En => self.en.insert(word.to_string(), rank),
            Lang::Uk => self.uk.insert(word.to_string(), rank),
        };
        self
    }

    /// Додати слово (наприклад, особисте): вважається дуже частотним.
    pub fn add_word(&mut self, lang: Lang, word: &str) {
        self.add_word_ranked(lang, word, RANK_USER);
    }

    /// Додати слово із заданим рангом (менше = частотніше). Двобуквені потрапляють і в короткий список мови.
    pub fn add_word_ranked(&mut self, lang: Lang, word: &str, rank: u32) {
        let w = word.to_lowercase();
        let (main, short) = match lang {
            Lang::En => (&mut self.en, &mut self.short_en),
            Lang::Uk => (&mut self.uk, &mut self.short_uk),
        };
        match lang {
            Lang::En => self.explicit_en.insert(w.clone()),
            Lang::Uk => self.explicit_uk.insert(w.clone()),
        };
        if w.chars().count() == 2 && w.chars().all(char::is_alphabetic) {
            short.insert(w.clone());
        }
        if lang == Lang::Uk && w.contains('\'') {
            main.entry(w.replace('\'', "")).and_modify(|r| *r = (*r).min(rank)).or_insert(rank);
        }
        main.entry(w).and_modify(|r| *r = (*r).min(rank)).or_insert(rank);
    }

    /// Шукає `dict/en.txt` і `dict/uk.txt` поруч з exe, а потім у поточній теці.
    pub fn load() -> io::Result<Self> {
        let dir = find_dir().ok_or_else(|| {
            io::Error::new(io::ErrorKind::NotFound, "не знайшов теку dict/ з en.txt і uk.txt")
        })?;
        let mut d = Self::from_lists(read_words(&dir.join("en.txt"))?, read_words(&dir.join("uk.txt"))?);
        // Морфологічний індекс (необов'язковий): без нього працює лише частотний список.
        if let Ok(bytes) = std::fs::read(dir.join("uk_forms.fst")) {
            d.set_forms(bytes)?;
        }
        // ІТ-лексика (необов'язково): cmd, dhcp, mtu, eoip…
        if let Ok(extra) = read_words(&dir.join("en_it.txt")) {
            d.it_count = extra.len();
            for w in extra {
                d.add_word_ranked(Lang::En, &w, RANK_IT);
            }
        }
        // Особистий словник (необов'язковий): власні терміни, імена, сленг, захист двобуквених слів.
        for (name, lang) in [("en_user.txt", Lang::En), ("uk_user.txt", Lang::Uk)] {
            if let Ok(extra) = read_words(&dir.join(name)) {
                for w in extra {
                    d.add_word(lang, &w);
                }
            }
        }
        Ok(d)
    }

    pub fn len(&self) -> (usize, usize) {
        (self.en.len(), self.uk.len())
    }

    /// Скільки слів у списку ІТ-лексики (0, якщо його нема).
    pub fn it_len(&self) -> usize {
        self.it_count
    }

    /// Скільки українських словоформ у морфологічному індексі (0, якщо його нема).
    pub fn forms_len(&self) -> usize {
        self.forms.as_ref().map_or(0, |s| s.len())
    }

    fn short_has(&self, lang: Lang, word: &str) -> bool {
        match lang {
            Lang::En => self.short_en.contains(word),
            Lang::Uk => self.short_uk.contains(word),
        }
    }

    fn forms_has(&self, word: &str) -> bool {
        self.forms.as_ref().map_or(false, |s| s.contains(word))
    }

    /// Чи є слово «відомим» словом мови (для захисту правильно набраних слів): будь-яка довжина.
    fn known(&self, lang: Lang, word: &str) -> bool {
        match lang {
            Lang::En => self.en.contains_key(word),
            Lang::Uk => self.uk.contains_key(word) || self.forms_has(word),
        }
    }

    /// Чи можна слово вважати ЦІЛЛЮ конвертації. Для коротких укр. слів (≤4) — лише частотний список:
    /// у повному морфологічному індексі випадкових збігів із англ. словами надто багато.
    fn target(&self, lang: Lang, word: &str) -> bool {
        match lang {
            Lang::En => self.en.contains_key(word),
            Lang::Uk => self.uk.contains_key(word) || (word.chars().count() >= 5 && self.forms_has(word)),
        }
    }

    /// Частотний ранг слова у списку своєї мови (0 = найчастіше); `None` — слова в частотному списку нема
    /// (воно може бути відоме лише з морфологічного індексу).
    pub fn freq_rank(&self, lang: Lang, word: &str) -> Option<u32> {
        match lang {
            Lang::En => self.en.get(word).copied(),
            Lang::Uk => self.uk.get(word).copied(),
        }
    }

    fn is_explicit(&self, lang: Lang, word: &str) -> bool {
        match lang {
            Lang::En => self.explicit_en.contains(word),
            Lang::Uk => self.explicit_uk.contains(word),
        }
    }

    /// Чи є слово в морфологічному словнику української (для аудиту й діагностики).
    pub fn in_forms(&self, word: &str) -> bool {
        self.forms_has(word)
    }

    /// «Надійне» слово — не шум корпусу. Англ.: додане явно або з першої 20-тисячки. Укр.: додане явно або
    /// підтверджене морфологічним словником (сам частотний список містить і російські слова, і сміття на
    /// кшталт «ьуе», «сфе» — це англійські `met`, `cat`, набрані в субтитрах не в тій розкладці).
    fn solid(&self, lang: Lang, word: &str) -> bool {
        if self.is_explicit(lang, word) {
            return true;
        }
        let top = |m: &BTreeMap<String, u32>| m.get(word).map_or(false, |&r| r < NOISE_RANK);
        match lang {
            Lang::En => top(&self.en),
            // без морфологічного індексу (файл відсутній) лишається тільки частота
            Lang::Uk => if self.forms.is_some() { self.forms_has(word) } else { top(&self.uk) },
        }
    }

    /// Обидва прочитання — «слова». Чи варто все ж замінити набране `typed` (мова `cur`) на `conv` (мова `to`)?
    fn override_known(&self, cur: Lang, typed: &str, conv: &str, same_len: bool, ctx: Option<Lang>) -> bool {
        let to = cur.other();
        if self.is_explicit(cur, typed) || !self.solid(to, conv) {
            return false; // явно додане слово не чіпаємо ніколи; і міняти на «шум» немає сенсу
        }
        let typed_solid = self.solid(cur, typed);
        let rt = self.freq_rank(cur, typed);
        let rc = self.freq_rank(to, conv);

        // 1) Набране — шум корпусу (хвіст списку, не підтверджене), а інше прочитання — надійне й значно частотніше:
        //    `ghb` (ранг 27 тис.) проти «при» (ранг 400). Надійні слова («суддю», `herb`) так не перебиваються.
        //    У бік англійської — лише якщо жодна літера не стала розділовим знаком по краях: людина, яка набирає
        //    англійською, не почне слово з `]` чи `,` (аудит: «їсиш» -> `]cbi`, «берут» -> `,then`).
        if let (Some(rt), Some(rc)) = (rt, rc) {
            let plausible = to == Lang::Uk || same_len;
            if plausible && !typed_solid && rt >= NOISE_RANK && rc.saturating_mul(RANK_MARGIN) < rt {
                return true;
            }
        }
        // 2) Коротка укр. форма, якої нема в частотному списку («щів»), проти явно доданого терміна (`osd`).
        if cur == Lang::Uk && rt.is_none() && typed.chars().count() <= 3 && self.is_explicit(to, conv) {
            return true;
        }
        // 3) Попередні слова були мовою `to`. Цього досить, якщо набране ненадійне або інше прочитання
        //    не набагато рідше за нього: `the` посеред українського тексту лишається `the` (а не «еру»),
        //    зате «руку» посеред англійського стає `here`.
        if same_len && ctx == Some(to) {
            let rt = rt.unwrap_or(u32::MAX);
            let rc = rc.unwrap_or(u32::MAX);
            return !typed_solid || rc <= rt.saturating_mul(RANK_MARGIN);
        }
        false
    }

    /// Детальний вердикт: чому саме слово виправили або не чіпали (для логу й для `judge`).
    /// `keys` — слово, набране в розкладці `cur`; `ctx` — мова попередніх слів (для рівних випадків).
    pub fn verdict(&self, keys: &[Key], cur: Lang, ctx: Option<Lang>) -> Verdict {
        // `=`, `\`, AltGr та інші службові клавіші — не чіпаємо.
        if keys.iter().any(|k| !is_tracked_key(k.vk) || matches!(k.vk, 0xBB | 0xDC)) {
            return Verdict::NonWord;
        }
        // Адреса сайту чи пошти, набрана в українській розкладці: `аучютуе` -> `fex.net`.
        if let Some(fix) = self.web_fix(keys, cur) {
            return Verdict::Fix(fix);
        }
        // Цифри й дефіс всередині — слово «складене» (KIT005, start-config, 192ю168ю1ю10).
        let verdict = if keys.iter().any(is_separator) {
            self.verdict_compound(keys, cur, ctx)
        } else {
            self.verdict_simple(keys, cur, ctx)
        };
        // «Виправлення», яке нічого не змінює на екрані, — не виправлення: інакше ми лише даремно
        // перемкнули б розкладку (у лозі число «23», набране в українській, «виправлялось» на «23»).
        match verdict {
            Verdict::Fix(f) if f.text == render(keys, cur) => Verdict::NonWord,
            v => v,
        }
    }

    /// Слово без цифр і дефіса. Shift+цифра (`?` `:` `;` `"` `!` у UK) — це розділовий знак по краях.
    fn verdict_simple(&self, keys: &[Key], cur: Lang, ctx: Option<Lang>) -> Verdict {
        let to = cur.other();
        let as_typed = core(&render(keys, cur));
        let converted = convert(keys, cur);
        let as_conv = core(&converted);
        let (nt, nc) = (as_typed.chars().count(), as_conv.chars().count());
        // Режим вибираємо за довшим із двох «ядер»: у правильно набраного слова в «чужому» вигляді
        // краєві клавіші-літери (`,` `.` `'`) обрізаються й збивали б довжину (так «без» ≈ `,tp`).
        let n = nt.max(nc);

        if n < 2 {
            return Verdict::Short;
        }
        if n < MIN_LEN {
            // Двобуквене слово: тільки за короткими списками (див. SHORT_UK / SHORT_EN).
            if self.short_has(cur, &as_typed) {
                return Verdict::KnownWord;
            }
            if self.short_has(to, &as_conv) {
                return Verdict::Fix(Fix { text: converted, to });
            }
            return Verdict::Unknown;
        }

        // Двобуквене «ядро» перевіряємо за перевіреним списком (справжні слова), а не за шумним словником.
        let typed_known = if nt <= 2 { self.short_has(cur, &as_typed) } else { self.known(cur, &as_typed) };
        let conv_ok = match nc {
            0 | 1 => false,
            2 => self.short_has(to, &as_conv),
            _ => self.target(to, &as_conv),
        };
        match (typed_known, conv_ok) {
            (false, true) => Verdict::Fix(Fix { text: converted, to }),
            (false, false) => Verdict::Unknown,
            (true, false) => Verdict::KnownWord,
            // Обидва варіанти — «слова» (напр. шумне англ. `ghb` і укр. «при»): зважуємо надійність, частоту й контекст.
            (true, true) => {
                if nt >= MIN_LEN && nc >= MIN_LEN && self.override_known(cur, &as_typed, &as_conv, nc == nt, ctx) {
                    Verdict::Fix(Fix { text: converted, to })
                } else {
                    Verdict::KnownWord
                }
            }
        }
    }

    /// Слово з цифрами/дефісом: розбираємо по сегментах між ними.
    fn verdict_compound(&self, keys: &[Key], cur: Lang, ctx: Option<Lang>) -> Verdict {
        let to = cur.other();
        // Числовий токен із клавішами-літерами замість розділових знаків: `192ю168ю1ю10`, `12Ж00`.
        if cur == Lang::Uk {
            if let Some(text) = numeric_punct_fix(keys) {
                return Verdict::Fix(Fix { text, to });
            }
        }
        // Токен цілком є серед явно доданих слів (`3d`): сегментами його не розібрати — «d» лише одна літера.
        if self.is_explicit(cur, &core_alnum(&render(keys, cur))) {
            return Verdict::KnownWord;
        }
        let converted = convert(keys, cur);
        if self.is_explicit(to, &core_alnum(&converted)) {
            return Verdict::Fix(Fix { text: converted, to });
        }
        let segs: Vec<&[Key]> = keys.split(is_separator).filter(|s| !s.is_empty()).collect();
        let verdicts: Vec<Verdict> = segs.iter().map(|s| self.verdict_simple(s, cur, ctx)).collect();

        // Хоч один сегмент — справжнє слово поточної мови (`будь-який`) → усе гаразд.
        if verdicts.iter().any(|v| matches!(v, Verdict::KnownWord)) {
            return Verdict::KnownWord;
        }
        if verdicts.iter().all(|v| matches!(v, Verdict::Short)) {
            return Verdict::NonWord; // самі цифри/розділові знаки, нічого конвертувати
        }
        // Конвертуємо, лише якщо КОЖЕН значущий сегмент — виправлення (одну літеру між цифрами не рахуємо).
        let all_ok = verdicts.iter().all(|v| matches!(v, Verdict::Fix(_) | Verdict::Short));
        let any_fix = verdicts.iter().any(|v| matches!(v, Verdict::Fix(_)));
        if any_fix && all_ok {
            Verdict::Fix(Fix { text: converted, to })
        } else {
            Verdict::Unknown
        }
    }

    pub fn judge(&self, keys: &[Key], cur: Lang) -> Option<Fix> {
        match self.verdict(keys, cur, None) {
            Verdict::Fix(f) => Some(f),
            _ => None,
        }
    }

    /// Чи починається з `prefix` хоч одне слово мови, яке ми знаємо (частотний список, особисті й вивчені слова,
    /// для української — ще й 3,4 млн словоформ). `prefix` — малими літерами.
    pub fn has_prefix(&self, lang: Lang, prefix: &str) -> bool {
        let map = match lang {
            Lang::En => &self.en,
            Lang::Uk => &self.uk,
        };
        let in_map = map.range::<str, _>((Bound::Included(prefix), Bound::Unbounded)).next().is_some_and(|(w, _)| w.starts_with(prefix));
        in_map
            || (lang == Lang::Uk
                && self.forms.as_ref().is_some_and(|f| {
                    let mut st = f.range().ge(prefix.as_bytes()).into_stream();
                    st.next().is_some_and(|w| w.starts_with(prefix.as_bytes()))
                }))
    }

    /// Чи починається з `prefix` хоч одне ЧАСТЕ й надійне слово мови (не шум корпусу): англ. — з першої
    /// 20-тисячки, укр. — з першої 30-тисячки й підтверджене морфологічним словником; або додане явно.
    pub fn has_frequent_prefix(&self, lang: Lang, prefix: &str) -> bool {
        let (map, limit) = match lang {
            Lang::En => (&self.en, NOISE_RANK),
            Lang::Uk => (&self.uk, EARLY_RANK_UK),
        };
        map.range::<str, _>((Bound::Included(prefix), Bound::Unbounded))
            .take_while(|(w, _)| w.starts_with(prefix))
            .take(EARLY_SCAN)
            .any(|(w, &r)| self.is_explicit(lang, w) || (r < limit && self.solid(lang, w)))
    }

    /// Слово ще набирається, а вже видно, що не в тій розкладці: з набраних літер не починається жодне слово
    /// мови `cur` (`ghb…`), зате в іншій це початок частого слова («при…»). Тоді перемикаємо розкладку одразу —
    /// решту слова людина набере вже правильно. Повертає, чим замінити набране (без правил для розділових
    /// знаків на краях: слово ще не скінчилось). `ctx` — мова останніх слів: якщо це `cur`, а слово схоже на
    /// іншу мову — це «вкраплене» слово (англійський термін в українському тексті), його виправить пробіл.
    pub fn early_fix(&self, keys: &[Key], cur: Lang, ctx: Option<Lang>) -> Option<Fix> {
        if keys.len() < EARLY_MIN || keys.len() > EARLY_MAX || ctx == Some(cur) {
            return None;
        }
        if keys.iter().any(|k| !is_tracked_key(k.vk) || is_separator(k) || matches!(k.vk, 0xBB | 0xDC)) {
            return None;
        }
        let to = cur.other();
        let typed = render(keys, cur);
        let text = render(keys, to);
        // Обидва прочитання мають бути «словом»: літери (і апостроф не на початку). Лапки, дужки, `.` `,` —
        // ні, крім клавіш, які в українській розкладці — літери (б ю ж х ї: так починаються «буде», «юрист»).
        let wordish = |s: &str, keys_are_uk_letters: bool| {
            s.chars().enumerate().all(|(i, c)| {
                c.is_alphabetic()
                    || (i > 0 && matches!(c, '\'' | 'ʼ' | '`'))
                    || (keys_are_uk_letters && matches!(c, ',' | '.' | ';' | '[' | ']'))
            })
        };
        if !wordish(&typed, cur == Lang::En) || !wordish(&text, false) {
            return None;
        }
        let t = typed.to_lowercase();
        let c = text.to_lowercase();
        if looks_secret(&typed) || self.has_prefix(cur, &t) || !self.has_frequent_prefix(to, &c) {
            return None;
        }
        // Уся комбінація — частий початок і в рідній мові без апострофа/дефіса? (`зявл…`) — has_prefix це врахував.
        Some(Fix { text, to })
    }

    /// Адреса сайту або пошти, набрана в українській розкладці (`аучютуе` -> `fex.net`,
    /// `гіук"фвьшт.сщь` -> `user@admin.com`). Словників тут нема, тому правило суворе: англійський вигляд має бути
    /// доменом із відомою зоною (`.com`, `.net`, `.ua`…) або поштою, а український — не словом.
    fn web_fix(&self, keys: &[Key], cur: Lang) -> Option<Fix> {
        if cur != Lang::Uk || keys.len() < 4 {
            return None;
        }
        let text = convert(keys, cur);
        let bare = text
            .trim_start_matches(['(', '"', '\'', '«'])
            .trim_end_matches(['.', ',', ';', ':', '!', '?', ')', '"', '\'', '»']);
        if !looks_like_web_address(bare) {
            return None;
        }
        if self.known(Lang::Uk, &core(&render(keys, cur))) {
            return None;
        }
        Some(Fix { text, to: Lang::En })
    }

    /// Читає додатковий список слів (по слову на рядок, `#` — коментар) і додає їх як явні слова мови.
    /// Повертає, скільки слів прочитано. Нема файлу — помилка (її можна ігнорувати).
    pub fn add_words_from(&mut self, path: &Path, lang: Lang) -> io::Result<usize> {
        let words = read_words(path)?;
        for w in &words {
            self.add_word(lang, w);
        }
        Ok(words.len())
    }

    /// Користувач сам переніс слово в розкладку `lang` (подвійним Shift), і воно так і лишилось. Чи варто його
    /// запам'ятати, щоб наступного разу виправляти без нього? Так — лише якщо словники про нього нічого не знають
    /// в обох прочитаннях (назви, прізвища, терміни: `LuxOpt`, `SiTime`). Повертає слово для особистого словника.
    /// Слова, схожі на пароль, і «справжні» слова іншої мови (`сім` ↔ `csv`) — ні.
    pub fn learnable(&self, keys: &[Key], lang: Lang) -> Option<String> {
        let (word, other) = learn_forms(keys, lang)?;
        if word.chars().count() < MIN_LEN {
            return None;
        }
        if self.known(lang, &word) || self.is_explicit(lang, &word) {
            return None; // уже відоме — автомат і так його не зіпсує; не вистачало іншого
        }
        if self.known(lang.other(), &other) || self.is_explicit(lang.other(), &other) {
            return None; // «чуже» прочитання — теж слово: вирішувати автоматично не можна
        }
        let letters = |s: &str| core(s);
        if self.known(lang, &letters(&word)) || self.known(lang.other(), &letters(&other)) {
            return None;
        }
        Some(word)
    }

    /// Користувач скасував АВТОМАТИЧНЕ виправлення подвійним Shift: слово, яке він набрав у розкладці `lang`, —
    /// справжнє, і чіпати його більше не треба. Повертає слово для особистого словника.
    pub fn learnable_revert(&self, keys: &[Key], lang: Lang) -> Option<String> {
        let (word, _) = learn_forms(keys, lang)?;
        if word.chars().count() < 2 || self.is_explicit(lang, &word) {
            return None;
        }
        Some(word)
    }
}

/// Обидва прочитання слова для особистого словника: `(у розкладці lang, в іншій)`. `None` — слово не годиться:
/// службові клавіші, схоже на пароль, задовге, без літер або із символами, яких у словах цієї мови не буває.
fn learn_forms(keys: &[Key], lang: Lang) -> Option<(String, String)> {
    if keys.is_empty() || keys.iter().any(|k| !is_tracked_key(k.vk) || matches!(k.vk, 0xBB | 0xDC)) {
        return None;
    }
    let shown = render(keys, lang);
    let other = render(keys, lang.other());
    if looks_secret(&shown) || looks_secret(&other) {
        return None;
    }
    let word = core_alnum(&shown);
    let n = word.chars().count();
    if n == 0 || n > 40 || !word.chars().any(char::is_alphabetic) {
        return None;
    }
    let fits = match lang {
        Lang::En => word.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.' | '@' | '\'')),
        Lang::Uk => word.chars().all(|c| is_uk_letter(c) || c.is_ascii_digit() || matches!(c, '-' | '\'' | 'ʼ')),
    };
    fits.then(|| (word, core_alnum(&other)))
}

fn is_uk_letter(c: char) -> bool {
    matches!(c, 'а'..='я' | 'А'..='Я' | 'і' | 'ї' | 'є' | 'ґ' | 'І' | 'Ї' | 'Є' | 'Ґ')
}

/// Однолітерні слова: українські («в базу», «а потім») і англійські (`a`, `I`).
const SINGLE_UK: &[char] = &['а', 'в', 'з', 'і', 'й', 'о', 'у', 'я'];
const SINGLE_EN: &[char] = &['a', 'i'];

fn single_word(c: char, lang: Lang) -> bool {
    let c = c.to_lowercase().next().unwrap_or(c);
    match lang {
        Lang::Uk => SINGLE_UK.contains(&c),
        Lang::En => SINGLE_EN.contains(&c),
    }
}

/// Однолітерне слово перед словом, яке щойно виправили з розкладки `cur` на іншу: `d` + `,fpe` -> «в базу».
/// Якщо ця літера в розкладці `cur` — не слово, а в іншій — слово, повертає, якою вона має стати.
/// Літеру, набрану свідомо (`D` — диск D посеред українського тексту), відсікає програма: вона пропонує
/// лише ту літеру, перед якою розкладку ніхто не перемикав.
pub fn single_letter_fix(key: Key, cur: Lang) -> Option<String> {
    if !is_tracked_key(key.vk) {
        return None;
    }
    let typed = char_for(key, cur);
    let conv = char_for(key, cur.other());
    (!single_word(typed, cur) && single_word(conv, cur.other())).then(|| conv.to_string())
}

/// Відомі доменні зони (з них складається більшість адрес, які набирають наші користувачі).
const TLDS: &[&str] = &[
    "com", "net", "org", "ua", "io", "dev", "info", "biz", "me", "co", "app", "ai", "uk", "de", "pl", "eu", "gov",
    "edu", "tv", "cc", "site", "online", "store", "shop", "tech", "cloud", "pro", "xyz", "us", "ca", "fr", "it",
    "es", "nl", "cz", "sk", "lt", "lv", "ee", "md", "ge", "kz", "at", "ch", "se", "no", "fi", "dk", "be", "ro",
    "hu", "bg", "gr", "pt", "il", "tr", "jp", "cn", "in", "br", "au", "nz", "mil", "int", "local", "lan", "email",
    "news", "blog", "page", "link", "live", "world", "agency", "digital", "media", "team", "group", "club",
    "space", "website", "top",
];

/// Чи схожий текст на адресу сайту (`fex.net`, `www.olx.ua/uk`, `https://site.com`) або пошти (`a.b@ukr.net`).
pub fn looks_like_web_address(s: &str) -> bool {
    let s = s.to_lowercase();
    if let Some((local, domain)) = s.split_once('@') {
        let local_ok = !local.is_empty()
            && local.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-' | '+' | '%'));
        return local_ok && !domain.contains('/') && is_host(domain);
    }
    let rest = s.strip_prefix("https://").or_else(|| s.strip_prefix("http://")).unwrap_or(&s);
    let (host, path) = match rest.split_once('/') {
        Some((h, p)) => (h, p),
        None => (rest, ""),
    };
    let path_ok = path
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || matches!(c, '/' | '.' | '-' | '_' | '?' | '=' | '&' | '%' | '#' | '+' | '~'));
    path_ok && is_host(host)
}

fn is_host(host: &str) -> bool {
    let labels: Vec<&str> = host.split('.').collect();
    if labels.len() < 2 || labels.len() > 6 {
        return false;
    }
    let label_ok = |l: &str| {
        !l.is_empty()
            && l.len() <= 63
            && !l.starts_with('-')
            && !l.ends_with('-')
            && l.chars().all(|c| c.is_ascii_alphanumeric() || c == '-')
    };
    let tld = labels[labels.len() - 1];
    labels.iter().all(|l| label_ok(l))
        && TLDS.contains(&tld)
        && labels[..labels.len() - 1].iter().any(|l| l.chars().any(|c| c.is_ascii_alphabetic() || c.is_ascii_digit()))
}

/// Роздільник у складеному слові: цифра без Shift або дефіс/підкреслення.
fn is_separator(k: &Key) -> bool {
    (matches!(k.vk, 0x30..=0x39) && !k.shift) || k.vk == 0xBD
}

/// У UK-розкладці `.` `,` `:` біля цифр часто набирають клавішами англ. розкладки, і виходять літери:
/// `192ю168ю1ю10` -> `192.168.1.10`, `12Ж00` -> `12:00`, `10б5` -> `10,5`. Лише якщо у слові нема інших літер.
fn numeric_punct_fix(keys: &[Key]) -> Option<String> {
    let is_digit = |k: &Key| matches!(k.vk, 0x30..=0x39) && !k.shift;
    if !keys.iter().any(is_digit) {
        return None;
    }
    if keys.iter().all(is_digit) {
        return None; // просто число — міняти нічого
    }
    let n = keys.len();
    for (i, k) in keys.iter().enumerate() {
        if is_digit(k) {
            continue;
        }
        let prev = i > 0 && is_digit(&keys[i - 1]);
        let next = i + 1 < n && is_digit(&keys[i + 1]);
        let ok = match (k.vk, k.shift) {
            (0xBE, false) => prev,        // «ю» після цифри -> крапка (192ю168, 5ю)
            (0xBC, false) => prev && next, // «б» між цифрами -> кома (10б5); «5б» (клас) не чіпаємо
            (0xBA, true) => prev && next,  // «Ж» між цифрами -> двокрапка (12Ж00)
            _ => false,
        };
        if !ok {
            return None;
        }
    }
    Some(render(keys, Lang::En))
}

/// Токен без розділових знаків по краях (цифри лишаються), малими літерами: `3D.` -> `3d`.
fn core_alnum(s: &str) -> String {
    s.trim_matches(|c: char| !c.is_alphanumeric()).to_lowercase()
}

/// Слово без розділових знаків по краях, малими літерами.
fn core(s: &str) -> String {
    s.trim_matches(|c: char| !c.is_alphabetic()).to_lowercase()
}

fn read_words(path: &Path) -> io::Result<Vec<String>> {
    let text = std::fs::read_to_string(path)?;
    // Підтримуємо і «слово», і «слово частота». Порядок рядків = порядок частотності.
    Ok(text
        .trim_start_matches('\u{feff}') // Блокнот може зберегти файл із позначкою BOM на початку
        .lines()
        .filter(|l| !l.trim_start().starts_with('#'))
        .filter_map(|l| l.split_whitespace().next())
        .map(str::to_lowercase)
        .collect())
}

fn find_dir() -> Option<PathBuf> {
    let mut candidates = Vec::new();
    if let Ok(exe) = std::env::current_exe() {
        if let Some(parent) = exe.parent() {
            candidates.push(parent.join("dict"));
        }
    }
    candidates.push(PathBuf::from("dict"));
    candidates
        .into_iter()
        .find(|d| d.join("en.txt").is_file() && d.join("uk.txt").is_file())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::keymap::keys_for;

    fn dicts() -> Dicts {
        Dicts::from_words(["hello", "world", "don't"], ["привіт", "хліб", "п'ять", "світ"])
    }

    fn typed(text: &str, lang: Lang) -> Vec<Key> {
        keys_for(text, lang).unwrap()
    }

    #[test]
    fn wrong_layout_en_to_uk() {
        let fix = dicts().judge(&typed("ghbdsn", Lang::En), Lang::En).unwrap();
        assert_eq!((fix.text.as_str(), fix.to), ("привіт", Lang::Uk));
    }

    #[test]
    fn wrong_layout_uk_to_en() {
        let fix = dicts().judge(&typed("руддщ", Lang::Uk), Lang::Uk).unwrap();
        assert_eq!((fix.text.as_str(), fix.to), ("hello", Lang::En));
    }

    #[test]
    fn correct_word_is_left_alone() {
        assert!(dicts().judge(&typed("hello", Lang::En), Lang::En).is_none());
        assert!(dicts().judge(&typed("привіт", Lang::Uk), Lang::Uk).is_none());
    }

    #[test]
    fn keeps_case_and_trailing_punctuation() {
        let fix = dicts().judge(&typed("Ghbdsn/", Lang::En), Lang::En).unwrap();
        assert_eq!(fix.text, "Привіт.");
    }

    #[test]
    fn apostrophe_key() {
        let fix = dicts().judge(&typed("g`znm", Lang::En), Lang::En).unwrap();
        assert_eq!(fix.text, "п'ять");
    }

    #[test]
    fn digits_and_short_words_are_skipped() {
        // слово з цифрою в кінці тепер конвертується разом із цифрою (як ЛШЕ005 -> KIT005)
        assert_eq!(dicts().judge(&typed("ghbdsn1", Lang::En), Lang::En).unwrap().text, "привіт1");
        assert!(dicts().judge(&typed("y", Lang::En), Lang::En).is_none()); // 1 літера
        assert!(dicts().judge(&typed("xz", Lang::En), Lang::En).is_none()); // 2 літери, нема в списках
    }

    #[test]
    fn iso_key_gives_ge_with_upturn_and_backslash_is_not_a_letter() {
        let dd = Dicts::from_words([], ["ґанок"]);
        let mut keys = vec![Key::new(0xE2, false)];
        keys.extend(typed("fyjr", Lang::En));
        let fix = dd.judge(&keys, Lang::En).unwrap();
        assert_eq!(fix.text, "ґанок");
        // `\` посеред слова — це вже не слово
        let mut bad = typed("fy", Lang::En);
        bad.push(Key::new(0xDC, false));
        bad.extend(typed("jr", Lang::En));
        assert!(matches!(dd.verdict(&bad, Lang::En, None), Verdict::NonWord));
    }

    #[test]
    fn verdict_reasons() {
        let d = dicts();
        assert!(matches!(d.verdict(&typed("hello", Lang::En), Lang::En, None), Verdict::KnownWord));
        assert!(matches!(d.verdict(&typed("kubectl", Lang::En), Lang::En, None), Verdict::Unknown));
        assert!(matches!(d.verdict(&typed("y", Lang::En), Lang::En, None), Verdict::Short));
        assert!(matches!(d.verdict(&typed("xz", Lang::En), Lang::En, None), Verdict::Unknown));
    }

    #[test]
    fn two_letter_words() {
        let d = dicts();
        let fix = |s: &str, l: Lang| d.judge(&typed(s, l), l).map(|f| (f.text, f.to));
        // «ок,» з коми на клавіші `/` (Shift+/ = `?` в EN, `,` в UK)
        assert_eq!(fix("jr?", Lang::En), Some(("ок,".to_string(), Lang::Uk)));
        assert_eq!(fix("yt", Lang::En), Some(("не".to_string(), Lang::Uk)));
        assert_eq!(fix(",j", Lang::En), Some(("бо".to_string(), Lang::Uk)));
        // англійські слова й укр. слова в правильній розкладці — не чіпаємо
        assert_eq!(fix("it", Lang::En), None);
        assert_eq!(fix("ok", Lang::En), None);
        assert_eq!(fix("не", Lang::Uk), None);
        // навпаки: англійське слово, набране в UK-розкладці
        assert_eq!(fix("щл", Lang::Uk), Some(("ok".to_string(), Lang::En)));
        assert_eq!(fix("ше", Lang::Uk), Some(("it".to_string(), Lang::En)));
        // посесив `'s` і маркер `/s` не мають перетворюватись
        assert_eq!(fix("'s", Lang::En), None);
        assert_eq!(fix("/s", Lang::En), None);
    }

    #[test]
    fn personal_dictionary_protects_two_letter_word() {
        let mut d = dicts();
        assert!(d.judge(&typed("db", Lang::En), Lang::En).is_some()); // «ви»
        d.add_word(Lang::En, "db");
        assert!(d.judge(&typed("db", Lang::En), Lang::En).is_none());
    }

    #[test]
    fn short_lists_collide_only_on_the_documented_pairs() {
        use crate::keymap::{keys_for, render};
        let mut found: Vec<(String, String)> = Vec::new();
        for w in SHORT_UK {
            let r = render(&keys_for(w, Lang::Uk).unwrap(), Lang::En);
            if SHORT_EN.contains(&r.as_str()) {
                found.push((w.to_string(), r));
            }
        }
        let mut expected: Vec<(String, String)> =
            AMBIGUOUS_SHORT.iter().map(|(u, e)| (u.to_string(), e.to_string())).collect();
        found.sort();
        expected.sort();
        assert_eq!(found, expected, "нова колізія коротких списків — або прибери слово, або додай пару в AMBIGUOUS_SHORT");
    }

    /// Аудит показав: «5 шт», «10 см», «100 гр», «ЗС» перетворювались на in / cv / uh / PC.
    #[test]
    fn two_letter_ukrainian_abbreviations_are_not_destroyed() {
        let d = dicts();
        let fix = |s: &str, l: Lang| d.judge(&typed(s, l), l).map(|f| f.text);
        for w in ["шт", "см", "гр", "ша", "св", "зс", "ЗС", "ця", "цю", "ту"] {
            assert_eq!(fix(w, Lang::Uk), None, "{w}");
        }
        // неоднозначні пари не чіпаємо в жоден бік
        for (uk, en) in AMBIGUOUS_SHORT {
            assert_eq!(fix(en, Lang::En), None, "{en}");
            assert_eq!(fix(uk, Lang::Uk), None, "{uk}");
        }
        // а однозначні — як і раніше виправляються
        assert_eq!(fix("wz", Lang::En).as_deref(), Some("ця"));
        assert_eq!(fix("uh", Lang::En).as_deref(), Some("гр"));
    }

    /// Усе, що пішло не так (або мало піти) у реальному логу.
    #[test]
    fn regressions_from_real_log() {
        let Ok(d) = Dicts::load() else { return };
        if d.forms_len() == 0 {
            // Якщо файл індексу є, а завантажився порожнім — це помилка, а не «пропускаємо».
            assert!(!std::path::Path::new("dict/uk_forms.fst").exists(), "uk_forms.fst є, але не завантажився");
            return;
        }
        eprintln!("[regressions_from_real_log] словоформ у індексі: {}", d.forms_len());
        let fix = |s: &str| d.judge(&typed(s, Lang::En), Lang::En).map(|f| f.text);
        // слова з відмінковими формами, яких бракувало в частотному списку
        assert_eq!(fix("drkflws").as_deref(), Some("вкладці"));
        assert_eq!(fix("pfgjdyb").as_deref(), Some("заповни"));
        assert_eq!(fix("utythfws'.").as_deref(), Some("генерацією"));
        assert_eq!(fix("lj,ht?").as_deref(), Some("добре,"));
        // слова з крайовими клавішами-літерами (`,` `.` `'`)
        assert_eq!(fix(",tp").as_deref(), Some("без"));
        assert_eq!(fix("vf.").as_deref(), Some("маю"));
        assert_eq!(fix("lf'").as_deref(), Some("дає"));
        // правильно набрані укр. слова — «відомі» (раніше «без/маю/дає» помилково були unknown)
        for w in ["без", "маю", "дає", "вкладці", "інтерфейсу", "видаляти"] {
            let v = d.verdict(&typed(w, Lang::Uk), Lang::Uk, None);
            assert!(matches!(v, Verdict::KnownWord), "{w}: {}", v.tag());
        }
        // `?` у UK-розкладці — це Shift+7: більше не робить слово «нeслівним»
        assert_eq!(fix("cfvbq&").as_deref(), Some("самий?"));
        // KIT005, набране правильно в EN, — справжнє слово з цифрами
        assert!(matches!(d.verdict(&typed("KIT005", Lang::En), Lang::En, None), Verdict::KnownWord));
    }

    /// Англійське слово + розділовий знак (кінець речення!) не має ставати укр. словом.
    #[test]
    fn english_short_word_with_punctuation_is_protected() {
        for (en, uk) in [("it.", "шею"), ("at.", "фею"), ("dc.", "всю"), ("ls.", "дію"), ("ls]", "дії")] {
            let d = Dicts::from_words([], [uk]);
            assert!(d.judge(&typed(en, Lang::En), Lang::En).is_none(), "{en} -> {uk}");
        }
    }

    #[test]
    fn context_breaks_ties_between_two_known_readings() {
        let d = Dicts::from_words(["dcs"], ["всі"]);
        let keys = typed("dcs", Lang::En);
        assert!(matches!(d.verdict(&keys, Lang::En, None), Verdict::KnownWord));
        assert!(matches!(d.verdict(&keys, Lang::En, Some(Lang::En)), Verdict::KnownWord));
        assert!(matches!(d.verdict(&keys, Lang::En, Some(Lang::Uk)), Verdict::Fix(_)));
    }

    /// Аудит показав: посеред українського тексту `the`, `cat`, `del`, `pass` перетворювались на
    /// «еру», «сфе», «вуд», «зфіі» — бо частотний список зі субтитрів повний такого сміття.
    #[test]
    fn context_does_not_turn_common_words_into_corpus_noise() {
        let d = Dicts::from_words([], [])
            .with_forms_from(["еру", "руку", "інше"])
            .with_rank(Lang::En, "the", 2)
            .with_rank(Lang::Uk, "еру", 26_934) // справжнє, але рідкісне слово
            .with_rank(Lang::En, "cat", 1_026)
            .with_rank(Lang::Uk, "сфе", 14_625) // сміття: нема в морфологічному словнику
            .with_rank(Lang::En, "here", 36)
            .with_rank(Lang::Uk, "руку", 599);
        let v = |s: &str, cur: Lang, ctx: Lang| d.verdict(&typed(s, cur), cur, Some(ctx));
        // англійські слова посеред українського тексту лишаються
        assert!(matches!(v("the", Lang::En, Lang::Uk), Verdict::KnownWord));
        assert!(matches!(v("cat", Lang::En, Lang::Uk), Verdict::KnownWord));
        assert!(matches!(v("here", Lang::En, Lang::Uk), Verdict::KnownWord));
        // «руку» посеред англійського тексту — це майже напевно `here`; «еру» там — `the`
        assert!(matches!(v("руку", Lang::Uk, Lang::En), Verdict::Fix(f) if f.text == "here"));
        assert!(matches!(v("еру", Lang::Uk, Lang::En), Verdict::Fix(f) if f.text == "the"));
        // а посеред українського обидва лишаються українськими
        assert!(matches!(v("руку", Lang::Uk, Lang::Uk), Verdict::KnownWord));
        assert!(matches!(v("еру", Lang::Uk, Lang::Uk), Verdict::KnownWord));
    }

    #[test]
    fn shifted_digit_is_punctuation_but_plain_digit_is_not() {
        let d = Dicts::from_words([], ["самий"]);
        let fix = d.judge(&typed("cfvbq&", Lang::En), Lang::En).unwrap();
        assert_eq!(fix.text, "самий?");
        // звичайна цифра — роздільник складеного слова, а не пунктуація: конвертується разом зі словом
        assert_eq!(d.judge(&typed("cfvbq7", Lang::En), Lang::En).unwrap().text, "самий7");
    }

    #[test]
    fn morphological_forms_are_targets_only_for_long_words() {
        let d = Dicts::from_words([], []).with_forms_from(["вкладці", "кіт", "кіта"]);
        // ≥5 літер — ціль із морфологічного індексу
        assert_eq!(d.judge(&typed("drkflws", Lang::En), Lang::En).unwrap().text, "вкладці");
        // ≤4 літер — лише з частотного списку (тут його нема), тож не конвертуємо
        assert!(d.judge(&typed("rsn", Lang::En), Lang::En).is_none());
        assert!(d.judge(&typed("rsnf", Lang::En), Lang::En).is_none());
        // але для захисту правильно набраних слів індекс діє на будь-якій довжині
        assert!(matches!(d.verdict(&typed("кіт", Lang::Uk), Lang::Uk, None), Verdict::KnownWord));
    }

    #[test]
    fn compound_words_with_digits_and_hyphens() {
        let d = Dicts::from_words(["kit", "start", "config", "peer", "hub"], ["будь", "який"]);
        let fix = |s: &str, l: Lang| d.judge(&typed(s, l), l).map(|f| f.text);
        // з реального логу: ці слова доводилось виправляти вручну
        assert_eq!(fix("ЛШЕ005", Lang::Uk).as_deref(), Some("KIT005"));
        assert_eq!(fix("іефке-сщташп", Lang::Uk).as_deref(), Some("start-config"));
        assert_eq!(fix("зуук-рги-зуук", Lang::Uk).as_deref(), Some("peer-hub-peer"));
        // правильні слова не чіпаємо
        assert_eq!(fix("KIT005", Lang::En), None);
        assert_eq!(fix("start-config", Lang::En), None);
        assert!(matches!(d.verdict(&typed("будь-який", Lang::Uk), Lang::Uk, None), Verdict::KnownWord));
        // самі цифри/IP, набрані в EN, і «k8s» — нічого конвертувати
        assert_eq!(fix("192.168.1.10", Lang::En), None);
        assert!(matches!(d.verdict(&typed("192.168.1.10", Lang::En), Lang::En, None), Verdict::NonWord));
        assert_eq!(fix("k8s", Lang::En), None);
        assert_eq!(fix("10сек", Lang::Uk), None);
    }

    #[test]
    fn numeric_tokens_typed_with_letter_keys_in_uk_layout() {
        let d = Dicts::from_words([], []);
        let fix = |s: &str| d.judge(&typed(s, Lang::Uk), Lang::Uk).map(|f| f.text);
        assert_eq!(fix("192ю168ю1ю10").as_deref(), Some("192.168.1.10")); // IP-адреса
        assert_eq!(fix("12Ж00").as_deref(), Some("12:00")); // час (Shift+; = Ж)
        assert_eq!(fix("10б5").as_deref(), Some("10,5"));
        assert_eq!(fix("192ю").as_deref(), Some("192."));
        // а ось це — не пунктуація: «5б» (клас), «5х» (множення), слова з літерами
        assert_eq!(fix("5б"), None);
        assert_eq!(fix("5х"), None);
        assert_eq!(fix("12ж00"), None); // малу «ж» (`;`) не чіпаємо
    }

    #[test]
    fn rank_breaks_ties_only_for_noise_words() {
        let d = Dicts::from_words([], [])
            .with_forms_from(["при", "буде", "руки", "руку", "суддю"])
            .with_rank(Lang::En, "ghb", 27_299) // шум корпусу
            .with_rank(Lang::Uk, "при", 408)
            .with_rank(Lang::En, "elt", 69_749)
            .with_rank(Lang::Uk, "буде", 79)
            .with_rank(Lang::En, "herb", 7_657) // справжнє англ. слово
            .with_rank(Lang::Uk, "руки", 326)
            .with_rank(Lang::Uk, "руку", 599) // часте укр. слово
            .with_rank(Lang::En, "here", 36)
            .with_rank(Lang::Uk, "суддю", 24_664) // рідкісне в субтитрах, але справжнє укр. слово
            .with_rank(Lang::En, "cell", 3_000);
        let fix = |s: &str, l: Lang| d.judge(&typed(s, l), l).map(|f| f.text);
        // з реального логу: потрібна була ручна правка
        assert_eq!(fix("ghb", Lang::En).as_deref(), Some("при"));
        assert_eq!(fix(",elt", Lang::En).as_deref(), Some("буде")); // кома по краю обрізається з «ядра»
        // справжні слова обох мов не перебиваються
        assert_eq!(fix("herb", Lang::En), None);
        assert_eq!(fix("руку", Lang::Uk), None);
        // аудит: «суддю» ставало `cell.` лише через низьку частоту в субтитрах
        assert_eq!(fix("суддю", Lang::Uk), None);
    }

    #[test]
    fn rare_ukrainian_form_is_not_overridden_by_a_common_english_word() {
        // «Іру» (ім'я Іра в знахідному) є лише в морфологічному індексі; `she` — частотне англ. слово
        // `she` — звичайне частотне слово (ранг 100 за порядком у списку), а не явно додане
        let filler: Vec<String> = (0..100).map(|i| format!("w{i}")).collect();
        let mut en: Vec<&str> = filler.iter().map(|s| s.as_str()).collect();
        en.push("she");
        let d = Dicts::from_words(en, []).with_forms_from(["іру"]);
        assert!(d.judge(&typed("іру", Lang::Uk), Lang::Uk).is_none());
        // але явний ІТ-термін (en_it.txt) таке слово перебиває: «щів» -> OSD
        let mut d = Dicts::from_words([], []).with_forms_from(["щів"]);
        d.add_word_ranked(Lang::En, "osd", RANK_IT);
        assert_eq!(d.judge(&typed("щів", Lang::Uk), Lang::Uk).unwrap().text, "osd");
    }

    #[test]
    fn rank_tiebreak_needs_the_target_to_be_verified() {
        // «цру» та російське «еще» є в частотному списку, але морфологічний словник їх не підтверджує
        let d = Dicts::from_words([], [])
            .with_forms_from(["інше"])
            .with_rank(Lang::En, "whe", 36_903)
            .with_rank(Lang::Uk, "цру", 2_662)
            .with_rank(Lang::En, "tot", 22_439)
            .with_rank(Lang::Uk, "еще", 137);
        assert!(d.judge(&typed("whe", Lang::En), Lang::En).is_none());
        assert!(d.judge(&typed("tot", Lang::En), Lang::En).is_none());
    }

    #[test]
    fn explicitly_added_words_are_never_overridden() {
        // користувач додав `her` у свій словник — жоден контекст не зробить із нього «рук»
        let mut d = Dicts::from_words([], []).with_forms_from(["рук"]).with_rank(Lang::Uk, "рук", 1_665);
        d.add_word(Lang::En, "her");
        let keys = typed("her", Lang::En);
        assert!(matches!(d.verdict(&keys, Lang::En, Some(Lang::Uk)), Verdict::KnownWord));
        assert!(matches!(d.verdict(&keys, Lang::En, None), Verdict::KnownWord));
    }

    /// Слова, які в логу доводилось виправляти вручну: ІТ-лексика, приголосні шуми, апостроф, відмінки.
    #[test]
    fn regressions_from_second_log() {
        let Ok(d) = Dicts::load() else { return };
        if d.forms_len() == 0 || d.it_len() == 0 {
            return;
        }
        let fix = |s: &str, l: Lang| d.judge(&typed(s, l), l).map(|f| f.text);
        // ІТ-терміни, набрані в UK-розкладці
        for (uk, en) in [("врсз", "dhcp"), ("сьв", "cmd"), ("ЬЕГ", "MTU"), ("ЩІВ", "OSD"), ("УщШЗ", "EoIP"), ("щзею", "opt.")] {
            assert_eq!(fix(uk, Lang::Uk).as_deref(), Some(en), "{uk}");
        }
        // шумні англ. «слова» проти частотних українських
        assert_eq!(fix("ghb", Lang::En).as_deref(), Some("при"));
        assert_eq!(fix(",elt", Lang::En).as_deref(), Some("буде"));
        // слово без апострофа (користувач його часто пропускає)
        assert_eq!(fix("pzdkz'nmcz", Lang::En).as_deref(), Some("зявляється"));
        // дієслова на -ся, яких бракувало (раніше `unmunch` їх не розгортав)
        for w in ["відображається", "завантажилась", "виконався", "перешивається", "підключається"] {
            let v = d.verdict(&typed(w, Lang::Uk), Lang::Uk, None);
            assert!(matches!(v, Verdict::KnownWord), "{w}: {}", v.tag());
        }
        // складені слова з логу
        assert_eq!(fix("ЛШЕ005", Lang::Uk).as_deref(), Some("KIT005"));
        assert_eq!(fix("іефке-сщташп", Lang::Uk).as_deref(), Some("start-config"));
        assert_eq!(fix("зуук-рги-зуук", Lang::Uk).as_deref(), Some("peer-hub-peer"));
        // справжнє укр. ім'я в знахідному («Іру») не перетворюється на англ. `she`
        assert_eq!(fix("Іру", Lang::Uk), None);
        // правильні IP та ІТ-слова в EN лишаються
        assert_eq!(fix("192.168.1.10", Lang::En), None);
        assert_eq!(fix("dhcp", Lang::En), None);
        assert_eq!(fix("start-config", Lang::En), None);
    }

    /// Аудит: sfp, exe, src, rsa, nfs, apn… ПСУВАЛИСЯ при правильному наборі (їхній укр. вигляд є в частотному
    /// списку як сміття). Жоден термін з en_it.txt не має конвертуватись — ні сам по собі, ні посеред укр. тексту.
    #[test]
    fn it_terms_are_never_converted() {
        let Ok(d) = Dicts::load() else { return };
        if d.it_len() == 0 {
            return;
        }
        let text = std::fs::read_to_string("dict/en_it.txt").unwrap();
        let mut checked = 0;
        for term in text.lines().filter(|l| !l.starts_with('#')).filter_map(|l| l.split_whitespace().next()) {
            assert!(
                term.chars().count() >= 3 || term.chars().any(|c| c.is_ascii_digit()),
                "двобуквений термін у en_it.txt: {term}"
            );
            let Some(keys) = keys_for(term, Lang::En) else { continue };
            for ctx in [None, Some(Lang::Uk), Some(Lang::En)] {
                let v = d.verdict(&keys, Lang::En, ctx);
                assert!(!matches!(v, Verdict::Fix(_)), "{term} (ctx {ctx:?}) -> {}", v.tag());
            }
            checked += 1;
        }
        assert!(checked > 300, "перевірено лише {checked}");
        for term in ["sfp", "exe", "src", "rsa", "nfs", "apn", "dll", "gcc", "otp", "rsc", "rtk", "tls", "cert", "sim"] {
            assert!(text.lines().any(|l| l == term), "нема терміна {term}");
        }
        // і навпаки: термін, набраний в UK-розкладці, повертається
        let back = d.judge(&typed("іаз", Lang::Uk), Lang::Uk).map(|f| f.text);
        assert_eq!(back.as_deref(), Some("sfp"));
    }

    /// Хибні спрацювання, знайдені аудитом (examples/audit.rs) на справжніх словниках.
    #[test]
    fn regressions_from_audit() {
        let Ok(d) = Dicts::load() else { return };
        if d.forms_len() == 0 {
            return;
        }
        let fix = |s: &str, l: Lang, ctx: Option<Lang>| match d.verdict(&typed(s, l), l, ctx) {
            Verdict::Fix(f) => Some(f.text),
            _ => None,
        };
        // правильно набрані укр. слова без контексту
        for w in ["суддю", "бігу", "еру", "шт", "см", "гр"] {
            assert_eq!(fix(w, Lang::Uk, None), None, "{w}");
        }
        // англ. `tot` не стає російським «еще»
        assert_eq!(fix("tot", Lang::En, None), None);
        // англ. слова посеред українського тексту
        for w in ["the", "here", "her", "ask", "met", "buy", "eye", "pass", "cat", "del", "self"] {
            assert_eq!(fix(w, Lang::En, Some(Lang::Uk)), None, "{w}");
        }
        // укр. слова посеред англійського тексту, де англ. прочитання — шум
        for w in ["тут", "від", "всі", "два", "усе", "при"] {
            assert_eq!(fix(w, Lang::Uk, Some(Lang::En)), None, "{w}");
        }
        // укр. слово з літерою на краю, яка в англ. розкладці — розділовий знак
        for w in ["берут", "їсиш", "хірню"] {
            assert_eq!(fix(w, Lang::Uk, None), None, "{w}");
        }
        // те, що мало лишитися, лишилось
        assert_eq!(fix("ghb", Lang::En, None).as_deref(), Some("при"));
        assert_eq!(fix("dcs", Lang::En, None).as_deref(), Some("всі"));
        assert_eq!(fix("руку", Lang::Uk, Some(Lang::En)).as_deref(), Some("here"));
    }

    #[test]
    fn word_typed_with_capslock_keeps_its_case_after_conversion() {
        let d = Dicts::from_words([], ["хліб", "жек"]);
        let caps = |s: &str| -> Vec<Key> { typed(s, Lang::En).into_iter().map(|k| Key { caps: true, ..k }).collect() };
        assert_eq!(d.judge(&caps("[ks,"), Lang::En).unwrap().text, "ХЛІБ"); // було б «хЛІб»
        assert_eq!(d.judge(&caps(";tr"), Lang::En).unwrap().text, "ЖЕК");
    }

    /// Третій лог: число «23», набране в українській розкладці, «виправлялось» на «23» — і розкладка
    /// перемикалась на англійську.
    #[test]
    fn plain_numbers_are_not_fixed() {
        let d = dicts();
        for (s, l) in [("23", Lang::Uk), ("2024", Lang::Uk), ("23", Lang::En), ("4567890-", Lang::Uk), ("-1", Lang::En)] {
            let v = d.verdict(&typed(s, l), l, None);
            assert!(!matches!(v, Verdict::Fix(_)), "{s}: {}", v.tag());
        }
        // а числа з «літерами замість знаків» — як і раніше
        assert_eq!(d.judge(&typed("192ю168ю1ю10", Lang::Uk), Lang::Uk).unwrap().text, "192.168.1.10");
    }

    /// Третій лог: `іір?` -> `ssh&`, `іір,` -> `ssh?`, `іаез:` -> `sftp^` (користувач одразу стирав зайвий знак).
    #[test]
    fn english_term_inside_ukrainian_sentence_keeps_its_punctuation() {
        let d = Dicts::from_words(["ssh", "sftp", "hello"], ["привіт", "перевір", "зможемо"]);
        let fix = |s: &str, l: Lang| d.judge(&typed(s, l), l).map(|f| f.text);
        assert_eq!(fix("іір?", Lang::Uk).as_deref(), Some("ssh?"));
        assert_eq!(fix("іір,", Lang::Uk).as_deref(), Some("ssh,"));
        assert_eq!(fix("іаез:", Lang::Uk).as_deref(), Some("sftp:"));
        assert_eq!(fix("\"руддщ\"", Lang::Uk).as_deref(), Some("\"hello\""));
        // у бік української — по клавішах, як і було (у логах 5 із 5 правильно)
        assert_eq!(fix("Ghbdsn?", Lang::En).as_deref(), Some("Привіт,"));
        assert_eq!(fix("gthtdsh?", Lang::En).as_deref(), Some("перевір,"));
        assert_eq!(fix("pvj;tvj&", Lang::En).as_deref(), Some("зможемо?"));
    }

    /// Третій лог: «3В» двічі виправляли вручну на `3D`. Сегментами токен не розібрати, тож шукаємо його цілим
    /// серед явно доданих слів.
    #[test]
    fn whole_token_with_a_digit_from_explicit_list() {
        let mut d = dicts();
        assert!(d.judge(&typed("3В", Lang::Uk), Lang::Uk).is_none());
        d.add_word_ranked(Lang::En, "3d", RANK_IT);
        assert_eq!(d.judge(&typed("3В", Lang::Uk), Lang::Uk).unwrap().text, "3D");
        assert_eq!(d.judge(&typed("3в.", Lang::Uk), Lang::Uk).unwrap().text, "3d.");
        // правильно набране `3D` лишається; сусідні токени не зачеплено
        assert!(matches!(d.verdict(&typed("3D", Lang::En), Lang::En, None), Verdict::KnownWord));
        assert!(d.judge(&typed("5В", Lang::Uk), Lang::Uk).is_none());
        assert!(d.judge(&typed("12В", Lang::Uk), Lang::Uk).is_none());
    }

    /// Третій лог: «5шт» стало `5in`, і користувач виправляв назад вручну.
    #[test]
    fn number_with_ukrainian_abbreviation_is_left_alone() {
        let d = dicts();
        for s in ["5шт", "10см", "100гр"] {
            let v = d.verdict(&typed(s, Lang::Uk), Lang::Uk, Some(Lang::Uk));
            assert!(!matches!(v, Verdict::Fix(_)), "{s}: {}", v.tag());
        }
    }

    #[test]
    fn unknown_both_ways_is_skipped() {
        assert!(dicts().judge(&typed("kubectl", Lang::En), Lang::En).is_none());
    }

    /// Регресія: «тестую» (клавіші `ntcne.`) мало не конвертувалось через прогалину в словнику.
    #[test]
    fn real_dictionary_knows_testuyu() {
        let Ok(d) = Dicts::load() else { return };
        let fix = d.judge(&typed("ntcne.", Lang::En), Lang::En).expect("має конвертувати");
        assert_eq!((fix.text.as_str(), fix.to), ("тестую", Lang::Uk));
    }

    /// Прогін по справжніх словниках (пропускається, якщо dict/ нема).
    #[test]
    fn real_dictionaries_recall_and_collisions() {
        let Ok(d) = Dicts::load() else { return };
        let sample = |set: &BTreeMap<String, u32>, lang: Lang, wrong: Lang| {
            let (mut total, mut fixed) = (0u32, 0u32);
            for w in set.keys().filter(|w| w.chars().count() >= MIN_LEN).step_by(7) {
                let Some(keys) = keys_for(w, lang) else { continue };
                total += 1;
                // слово `w` мови `lang`, набране у «чужій» розкладці `wrong`
                if let Some(f) = d.judge(&keys, wrong) {
                    if f.text == *w {
                        fixed += 1;
                    }
                }
            }
            (fixed, total)
        };
        let (f1, t1) = sample(&d.uk, Lang::Uk, Lang::En);
        let (f2, t2) = sample(&d.en, Lang::En, Lang::Uk);
        eprintln!("укр. слова, набрані в EN: виправлено {f1}/{t1} ({:.1}%)", 100.0 * f1 as f64 / t1 as f64);
        eprintln!("англ. слова, набрані в UK: виправлено {f2}/{t2} ({:.1}%)", 100.0 * f2 as f64 / t2 as f64);
        assert!(f1 as f64 / t1 as f64 > 0.9);
        assert!(f2 as f64 / t2 as f64 > 0.9);
    }

    // ── 0.9.1: адреси, особистий словник, однолітерні слова ──

    /// Як було б набрано англійський текст `en`, якщо розкладка — українська.
    fn in_uk_layout(en: &str) -> Vec<Key> {
        typed(en, Lang::En)
    }

    #[test]
    fn web_addresses_typed_in_uk_layout_are_fixed() {
        let d = dicts();
        for addr in ["fex.net", "aernd.com", "www.olx.ua", "user@ukr.net", "a.b-c@mail.example.com", "site.com.ua/uk"] {
            let fix = d.judge(&in_uk_layout(addr), Lang::Uk).unwrap_or_else(|| panic!("{addr}"));
            assert_eq!((fix.text.as_str(), fix.to), (addr, Lang::En), "{addr}");
        }
        // розділовий знак речення після адреси лишається українським
        let mut keys = in_uk_layout("fex.net");
        keys.extend(typed(",", Lang::Uk));
        assert_eq!(d.judge(&keys, Lang::Uk).unwrap().text, "fex.net,");
    }

    #[test]
    fn web_rule_leaves_ordinary_text_alone() {
        let d = Dicts::from_words(["hello"], ["люди", "сюди"]);
        // українські слова з «ю», справжні адреси в англійській розкладці, скорочення «м.Львів»
        assert!(d.judge(&typed("люди", Lang::Uk), Lang::Uk).is_none());
        assert!(d.judge(&typed("сюди", Lang::Uk), Lang::Uk).is_none());
        assert!(d.judge(&typed("fex.net", Lang::En), Lang::En).is_none());
        assert!(d.judge(&typed("м.Львів", Lang::Uk), Lang::Uk).is_none());
        // невідома зона — не адреса
        assert!(d.judge(&in_uk_layout("fex.qqq"), Lang::Uk).is_none());
    }

    #[test]
    fn web_address_shapes() {
        for ok in ["fex.net", "WWW.Google.COM", "https://site.ua/a?b=1", "x1.io", "a@b.ua", "1c.ua"] {
            assert!(looks_like_web_address(ok), "{ok}");
        }
        for bad in ["fex", "fex.", ".net", "a..net", "-a.com", "a@", "@a.com", "v/kmdsd", "a@b.com/x", "tel.qq"] {
            assert!(!looks_like_web_address(bad), "{bad}");
        }
    }

    #[test]
    fn learns_unknown_words_moved_by_hand() {
        let d = Dicts::from_words(["csv", "dell"], ["сім", "база"]);
        assert_eq!(d.learnable(&typed("LuxOpt", Lang::En), Lang::En).as_deref(), Some("luxopt"));
        assert_eq!(d.learnable(&typed("SiTime,", Lang::En), Lang::En).as_deref(), Some("sitime"));
        assert_eq!(d.learnable(&typed("ae-work", Lang::En), Lang::En).as_deref(), Some("ae-work"));
        assert_eq!(d.learnable(&typed("Шевчук", Lang::Uk), Lang::Uk).as_deref(), Some("шевчук"));
        // «сім» — справжнє слово: перенесення в `csv` не вчимо (інакше «сім» завжди ставало б `csv`)
        assert_eq!(d.learnable(&typed("csv", Lang::En), Lang::En), None);
        // уже відоме слово — вчити нічого
        assert_eq!(d.learnable(&typed("dell", Lang::En), Lang::En), None);
        // коротке, схоже на пароль, задовге
        assert_eq!(d.learnable(&typed("ab", Lang::En), Lang::En), None);
        assert_eq!(d.learnable(&typed("Qwerty123!", Lang::En), Lang::En), None);
        assert_eq!(d.learnable(&typed(&"a".repeat(41), Lang::En), Lang::En), None);
    }

    #[test]
    fn learned_word_is_fixed_next_time() {
        let mut d = Dicts::from_words(["hello"], ["привіт"]);
        let keys = typed("ДгчЩзе", Lang::Uk); // LuxOpt, набране в українській
        assert!(d.judge(&keys, Lang::Uk).is_none());
        let w = d.learnable(&keys, Lang::En).unwrap();
        d.add_word(Lang::En, &w);
        assert_eq!(d.judge(&keys, Lang::Uk).unwrap().text, "LuxOpt");
        // складене слово теж
        let keys = typed("фу-цщкл", Lang::Uk);
        let w = d.learnable(&keys, Lang::En).unwrap();
        d.add_word(Lang::En, &w);
        assert_eq!(d.judge(&keys, Lang::Uk).unwrap().text, "ae-work");
    }

    #[test]
    fn revert_teaches_to_leave_the_word_alone() {
        let mut d = Dicts::from_words(["itch"], ["привіт"]);
        let keys = typed("шеср", Lang::Uk);
        assert_eq!(d.judge(&keys, Lang::Uk).unwrap().text, "itch");
        let w = d.learnable_revert(&keys, Lang::Uk).unwrap();
        assert_eq!(w, "шеср");
        d.add_word(Lang::Uk, &w);
        assert!(d.judge(&keys, Lang::Uk).is_none());
        assert!(d.judge(&keys, Lang::Uk).is_none());
    }

    #[test]
    fn single_letter_before_a_fixed_word() {
        let k = |c: &str, l| typed(c, l)[0];
        assert_eq!(single_letter_fix(k("d", Lang::En), Lang::En).as_deref(), Some("в"));
        assert_eq!(single_letter_fix(k("D", Lang::En), Lang::En).as_deref(), Some("В"));
        assert_eq!(single_letter_fix(k("f", Lang::En), Lang::En).as_deref(), Some("а"));
        assert_eq!(single_letter_fix(k("s", Lang::En), Lang::En).as_deref(), Some("і"));
        assert_eq!(single_letter_fix(k("ф", Lang::Uk), Lang::Uk).as_deref(), Some("a"));
        assert_eq!(single_letter_fix(k("Ш", Lang::Uk), Lang::Uk).as_deref(), Some("I"));
        // справжнє слово своєї мови не чіпаємо: «a» в англійській, «в» в українській
        assert_eq!(single_letter_fix(k("a", Lang::En), Lang::En), None);
        assert_eq!(single_letter_fix(k("в", Lang::Uk), Lang::Uk), None);
        // літера, яка ніде не слово
        assert_eq!(single_letter_fix(k("x", Lang::En), Lang::En), None);
        assert_eq!(single_letter_fix(k("5", Lang::En), Lang::En), None);
    }

    // ── 0.9.2: виправлення посеред слова ──

    #[test]
    fn early_fix_after_three_letters() {
        let d = Dicts::from_words(["windows", "the", "there"], ["привіт", "буде"]);
        let f = d.early_fix(&typed("ghb", Lang::En), Lang::En, None).unwrap();
        assert_eq!((f.text.as_str(), f.to), ("при", Lang::Uk));
        let f = d.early_fix(&typed("Цшт", Lang::Uk), Lang::Uk, Some(Lang::En)).unwrap();
        assert_eq!((f.text.as_str(), f.to), ("Win", Lang::En));
        // «б» на клавіші коми: ,eltn -> буде
        assert_eq!(d.early_fix(&typed(",el", Lang::En), Lang::En, None).unwrap().text, "буд");
    }

    #[test]
    fn early_fix_stays_quiet_when_unsure() {
        let d = Dicts::from_words(["windows", "the", "there"], ["привіт"]);
        // дві літери — замало
        assert!(d.early_fix(&typed("gh", Lang::En), Lang::En, None).is_none());
        // з такого початку є слово поточної мови
        assert!(d.early_fix(&typed("the", Lang::En), Lang::En, None).is_none());
        // вкраплене слово (останні слова — поточною мовою): виправить пробіл, розкладку не перемикаючи
        assert!(d.early_fix(&typed("ghb", Lang::En), Lang::En, Some(Lang::En)).is_none());
        // в іншій мові такого частого початку нема
        assert!(d.early_fix(&typed("qzx", Lang::En), Lang::En, None).is_none());
        // цифри, дефіс, лапки — не слово
        assert!(d.early_fix(&typed("gh1", Lang::En), Lang::En, None).is_none());
        assert!(d.early_fix(&typed("\"gh", Lang::En), Lang::En, None).is_none());
        // рідкісне (шумове) слово іншої мови — не привід
        let noisy = Dicts::from_words(["the"], []).with_rank(Lang::Uk, "приклад", 90_000);
        assert!(noisy.early_fix(&typed("ghb", Lang::En), Lang::En, None).is_none());
    }

    #[test]
    fn learned_word_blocks_early_fix() {
        let mut d = Dicts::from_words(["the"], ["привіт"]);
        assert!(d.early_fix(&typed("ghbdsn", Lang::En), Lang::En, None).is_some());
        d.add_word(Lang::En, "ghbx"); // вивчене «не чіпати»
        assert!(d.early_fix(&typed("ghb", Lang::En), Lang::En, None).is_none());
    }

    #[test]
    fn prefix_lookup_uses_morphology_too() {
        let d = Dicts::from_words(["the"], []).with_forms_from(["приклад"]);
        assert!(d.has_prefix(Lang::Uk, "прик"));
        assert!(!d.has_prefix(Lang::Uk, "прж"));
        assert!(d.has_prefix(Lang::En, "th"));
    }
}
