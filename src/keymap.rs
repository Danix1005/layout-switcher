//! Таблиці розкладок: US QWERTY <-> Ukrainian (Windows).
//!
//! Ми запам'ятовуємо не символи, а *фізичні клавіші* (virtual-key + Shift + CapsLock).
//! Тоді одне й те саме слово можна «показати» в будь-якій розкладці.

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Lang {
    En,
    Uk,
}

impl Lang {
    pub fn other(self) -> Lang {
        match self {
            Lang::En => Lang::Uk,
            Lang::Uk => Lang::En,
        }
    }
}

/// Натиснута клавіша: віртуальний код і стан Shift та CapsLock у момент натискання.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Key {
    pub vk: u32,
    pub shift: bool,
    /// Чи був увімкнений CapsLock. На які клавіші він діє, залежить від розкладки (див. `caps_applies`).
    pub caps: bool,
}

impl Key {
    pub const fn new(vk: u32, shift: bool) -> Key {
        Key { vk, shift, caps: false }
    }
}

/// На які клавіші діє CapsLock. A–Z — в обох розкладках. В українській ще й літери на «знакових» клавішах
/// (х ї ж є б ю ґ): у файлі розкладки Windows (KLC) у них стоїть прапорець CAPLOK, а в англійській `[ ] ; ' , .`
/// від CapsLock не залежать. Тому «ХЛІБ», набраний із CapsLock не в тій розкладці, — це `[KS,`, а не `{KS<`.
fn caps_applies(vk: u32, lang: Lang) -> bool {
    match vk {
        0x41..=0x5A => true,
        0xBA | 0xBC | 0xBE | 0xDB | 0xDD | 0xDE | 0xE2 => lang == Lang::Uk,
        _ => false,
    }
}

/// Чи дає клавіша «верхній» символ у цій розкладці (Shift, перевернутий CapsLock-ом там, де він діє).
fn upper(k: Key, lang: Lang) -> bool {
    k.shift ^ (k.caps && caps_applies(k.vk, lang))
}

/// Літери та клавіші, які в одній з розкладок є літерами або кінцевою пунктуацією
/// (; , . / ` [ ] ' та додаткова ISO-клавіша, де в укр. розкладці стоїть ґ).
/// Клавіша `\` сюди НЕ входить: в обох розкладках це знак, не літера.
pub fn is_word_key(vk: u32) -> bool {
    matches!(vk, 0x41..=0x5A | 0xBA | 0xBC | 0xBE..=0xC0 | 0xDB | 0xDD | 0xDE | 0xE2)
}

/// Усе, що ми записуємо в буфер слова: word-клавіші + цифри + `=` `-` `\`.
pub fn is_tracked_key(vk: u32) -> bool {
    is_word_key(vk) || matches!(vk, 0x30..=0x39 | 0xBB | 0xBD | 0xDC)
}

// A..Z на ЙЦУКЕН (Windows, «Українська»)
const UK_LETTERS: [char; 26] = [
    'ф', 'и', 'с', 'в', 'у', 'а', 'п', 'р', 'ш', 'о', 'л', 'д', 'ь', 'т', 'щ', 'з', 'й', 'к', 'і',
    'е', 'г', 'м', 'ц', 'ч', 'н', 'я',
];
const EN_DIGITS_SHIFT: [char; 10] = [')', '!', '@', '#', '$', '%', '^', '&', '*', '('];
const UK_DIGITS_SHIFT: [char; 10] = [')', '!', '"', '№', ';', '%', ':', '?', '*', '('];

/// Який символ дає клавіша в заданій розкладці.
pub fn char_for(k: Key, lang: Lang) -> char {
    use Lang::{En, Uk};
    let up = upper(k, lang);
    let pick = |lo: char, hi: char| if up { hi } else { lo };
    match k.vk {
        0x41..=0x5A => {
            let i = (k.vk - 0x41) as usize;
            let lo = match lang {
                En => (b'a' + i as u8) as char,
                Uk => UK_LETTERS[i],
            };
            if up {
                lo.to_uppercase().next().unwrap_or(lo)
            } else {
                lo
            }
        }
        0x30..=0x39 => {
            let i = (k.vk - 0x30) as usize;
            if up {
                match lang {
                    En => EN_DIGITS_SHIFT[i],
                    Uk => UK_DIGITS_SHIFT[i],
                }
            } else {
                (b'0' + i as u8) as char
            }
        }
        0xBB => pick('=', '+'),
        0xBD => pick('-', '_'),
        0xBA => match lang { En => pick(';', ':'), Uk => pick('ж', 'Ж') },
        0xBC => match lang { En => pick(',', '<'), Uk => pick('б', 'Б') },
        0xBE => match lang { En => pick('.', '>'), Uk => pick('ю', 'Ю') },
        0xBF => match lang { En => pick('/', '?'), Uk => pick('.', ',') },
        // У «Ukrainian (Enhanced)» тут апостроф U+0027 і ₴. У звичайній «Ukrainian» тут ё — для слів нерелевантно.
        0xC0 => match lang { En => pick('`', '~'), Uk => pick('\'', '₴') },
        0xDB => match lang { En => pick('[', '{'), Uk => pick('х', 'Х') },
        // Клавіша `\`: в обох укр. розкладках (звичайна й розширена) — `\` та `/`, а не ґ.
        0xDC => match lang { En => pick('\\', '|'), Uk => pick('\\', '/') },
        // Додаткова клавіша ISO-клавіатур (поруч з лівим Shift): ґ/Ґ в обох укр. розкладках.
        0xE2 => match lang { En => pick('\\', '|'), Uk => pick('ґ', 'Ґ') },
        0xDD => match lang { En => pick(']', '}'), Uk => pick('ї', 'Ї') },
        0xDE => match lang { En => pick('\'', '"'), Uk => pick('є', 'Є') },
        _ => '\u{FFFD}',
    }
}

/// Як виглядає послідовність клавіш у заданій розкладці.
pub fn render(keys: &[Key], lang: Lang) -> String {
    keys.iter().map(|&k| char_for(k, lang)).collect()
}

/// «Розділовий знак речення» української розкладки, на клавіші якого в англійській стоїть ІНШИЙ знак:
/// `.` → `/`,  `,` → `?`,  `"` → `@`,  `;` → `$`,  `:` → `^`,  `?` → `&`.
/// Знака `№` (в англійській `#`) тут нема навмисно: в англійському слові він не трапляється, а `C#` — так.
fn uk_sentence_punct(k: Key) -> bool {
    matches!((k.vk, k.shift), (0xBF, _) | (0x32, true) | (0x34, true) | (0x36, true) | (0x37, true))
}

/// Клавіша, яка в обох розкладках дає не літеру й не цифру.
fn punct_in_both(k: Key) -> bool {
    !char_for(k, Lang::En).is_alphanumeric() && !char_for(k, Lang::Uk).is_alphanumeric()
}

/// Перенести одне слово з розкладки `from` в іншу. `items` — клавіша (якщо символ є в розкладці) і сам символ.
fn convert_token(items: &[(Option<Key>, char)], from: Lang) -> String {
    let to = from.other();
    let edge = |it: &&(Option<Key>, char)| it.0.map_or(false, punct_in_both);
    let n = items.len();
    let lead = items.iter().take_while(edge).count();
    let trail = items.iter().rev().take_while(edge).count().min(n - lead);
    items
        .iter()
        .enumerate()
        .map(|(i, &(key, shown))| {
            let Some(k) = key else { return shown };
            let leading = i < lead;
            let on_edge = leading || i >= n - trail;
            // Крапка ПЕРЕД словом — це не кінець речення, а шлях чи ключ команди: `/volume1`, `/all`.
            // І «;» перед словом — це `$`: `$HOME`.
            let by_keys_anyway = leading && matches!((k.vk, k.shift), (0xBF, false) | (0x34, true));
            if from == Lang::Uk && on_edge && uk_sentence_punct(k) && !by_keys_anyway {
                shown
            } else {
                char_for(k, to)
            }
        })
        .collect()
}

/// Як має виглядати слово, набране в розкладці `from`, після перенесення в іншу розкладку.
///
/// Літери переносимо «по клавішах». А розділові знаки по краях слова читаємо так, як в УКРАЇНСЬКІЙ розкладці,
/// в обох напрямках:
/// - українське слово, набране в англійській (`Ghbdsn?`): людина думає, що розкладка українська, і тисне
///   «українську» кому — тому по клавішах: «Привіт,»;
/// - англійський термін, набраний посеред українського речення (`іір?`, `іаез:`): знак належить реченню —
///   лишаємо його як є: `ssh?`, `sftp:` (по клавішах вийшло б `ssh&`, `sftp^`).
/// Знаки всередині слова завжди по клавішах: `гіук"фвьшт` -> `user@admin`.
pub fn convert(keys: &[Key], from: Lang) -> String {
    let items: Vec<(Option<Key>, char)> = keys.iter().map(|&k| (Some(k), char_for(k, from))).collect();
    convert_token(&items, from)
}

/// Яка клавіша (з Shift чи без) дає символ `c` у розкладці `lang`.
pub fn key_of(c: char, lang: Lang) -> Option<Key> {
    (0x30u32..=0xE2)
        .filter(|&vk| is_tracked_key(vk))
        .flat_map(|vk| [false, true].map(move |shift| Key::new(vk, shift)))
        .find(|&k| char_for(k, lang) == c)
}

/// Обернена задача: які клавіші треба натиснути, щоб у розкладці `lang` вийшов `text`.
pub fn keys_for(text: &str, lang: Lang) -> Option<Vec<Key>> {
    text.chars().map(|c| key_of(c, lang)).collect()
}

/// Результат перекладу довільного тексту в іншу розкладку.
pub struct Converted {
    pub text: String,
    /// Розкладка, в якій текст було набрано (вгадана за літерами).
    pub from: Lang,
}

/// Переклад довільного тексту (наприклад, виділеного фрагмента) «по клавішах» в іншу розкладку.
/// Розкладку-джерело визначаємо за переважними літерами; символи, яких нема в цій розкладці
/// (пробіли, переноси рядків, літери іншого алфавіту, емодзі), лишаються як є.
/// За рівної кількості літер обираємо `hint` (поточну розкладку), інакше англійську.
/// `None` — у тексті нема літер обох розкладок, конвертувати нічого.
pub fn convert_text(text: &str, hint: Option<Lang>) -> Option<Converted> {
    let (mut uk, mut en) = (0usize, 0usize);
    for c in text.chars().filter(|c| c.is_alphabetic()) {
        if key_of(c, Lang::Uk).is_some() {
            uk += 1;
        } else if key_of(c, Lang::En).is_some() {
            en += 1;
        }
    }
    let from = match uk.cmp(&en) {
        std::cmp::Ordering::Greater => Lang::Uk,
        std::cmp::Ordering::Less => Lang::En,
        std::cmp::Ordering::Equal if uk == 0 => return None,
        std::cmp::Ordering::Equal => hint.unwrap_or(Lang::En),
    };
    // По словах: розділові знаки по краях слова підкоряються тому самому правилу, що й у `convert`.
    let mut out = String::with_capacity(text.len());
    let mut token: Vec<(Option<Key>, char)> = Vec::new();
    for c in text.chars() {
        if c.is_whitespace() {
            out.push_str(&convert_token(&token, from));
            token.clear();
            out.push(c);
        } else {
            token.push((key_of(c, from), c));
        }
    }
    out.push_str(&convert_token(&token, from));
    let text = out;
    Some(Converted { text, from })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn privit() {
        let keys = keys_for("ghbdsn", Lang::En).unwrap();
        assert_eq!(render(&keys, Lang::Uk), "привіт");
    }

    #[test]
    fn caps_and_punct() {
        let keys = keys_for("Ghbdsn/", Lang::En).unwrap();
        assert_eq!(render(&keys, Lang::Uk), "Привіт.");
        let keys = keys_for("[ks,", Lang::En).unwrap();
        assert_eq!(render(&keys, Lang::Uk), "хліб");
    }

    /// Звірено з KLC розкладки «Ukrainian (Enhanced)» (kbdur1.dll) — перші три ряди й OEM-клавіші.
    #[test]
    fn matches_windows_enhanced_layout_klc() {
        let rows = [
            ("qwertyuiop[]", "йцукенгшщзхї"),
            ("asdfghjkl;'", "фівапролджє"),
            ("zxcvbnm,.", "ячсмитьбю"),
            ("/", "."),
            ("`", "'"),
        ];
        for (en, uk) in rows {
            let keys = keys_for(en, Lang::En).unwrap();
            assert_eq!(render(&keys, Lang::Uk), uk, "ряд {en}");
        }
        // Shift-стани за KLC: ₴, коми, цифровий ряд.
        let sh = |vk| Key::new(vk, true);
        assert_eq!(char_for(sh(0xC0), Lang::Uk), '₴');
        assert_eq!(char_for(sh(0xBF), Lang::Uk), ',');
        assert_eq!(render(&"1234567890".chars().map(|c| sh(c as u32)).collect::<Vec<_>>(), Lang::Uk), "!\"№;%:?*()");
        // `\` — це `\` і `/`, а ґ — на ISO-клавіші 0xE2.
        assert_eq!(char_for(Key::new(0xDC, false), Lang::Uk), '\\');
        assert_eq!(char_for(sh(0xDC), Lang::Uk), '/');
        assert_eq!(char_for(Key::new(0xE2, false), Lang::Uk), 'ґ');
        assert_eq!(char_for(sh(0xE2), Lang::Uk), 'Ґ');
    }

    #[test]
    fn roundtrip_all_ukrainian_letters() {
        for c in "абвгґдеєжзиіїйклмнопрстуфхцчшщьюя'.,АБВГҐДЕЄЖЗИІЇЙКЛМНОПРСТУФХЦЧШЩЬЮЯ".chars() {
            let keys = keys_for(&c.to_string(), Lang::Uk).unwrap_or_else(|| panic!("нема клавіші для {c}"));
            assert_eq!(render(&keys, Lang::Uk).chars().next(), Some(c));
        }
    }

    #[test]
    fn convert_text_detects_direction_by_letters() {
        let c = convert_text("ghbdsn", None).unwrap();
        assert_eq!((c.text.as_str(), c.from), ("привіт", Lang::En));
        let c = convert_text("Привіт", None).unwrap();
        assert_eq!((c.text.as_str(), c.from), ("Ghbdsn", Lang::Uk));
    }

    #[test]
    fn convert_text_keeps_spaces_newlines_and_digits() {
        let c = convert_text("ghbdsn 123\nvbh", None).unwrap();
        assert_eq!(c.text, "привіт 123\nмир");
    }

    #[test]
    fn convert_text_leaves_the_other_script_alone() {
        // 6 лат. + 6 кирил. літер — нічия: бере hint
        let c = convert_text("ghbdsn привіт", Some(Lang::En)).unwrap();
        assert_eq!(c.text, "привіт привіт");
        let c = convert_text("ghbdsn привіт", Some(Lang::Uk)).unwrap();
        assert_eq!(c.text, "ghbdsn ghbdsn");
    }

    #[test]
    fn convert_text_reads_punctuation_the_ukrainian_way() {
        // з англійської в українську — по клавішах: у UK-розкладці `,` — це Shift+`/`, а `.` — клавіша `/`
        let c = convert_text("Ghbdsn? cdsnt/", Some(Lang::En)).unwrap();
        assert_eq!(c.text, "Привіт, світе.");
        let c = convert_text("ghbdsn? zr cghfdb&", None).unwrap();
        assert_eq!(c.text, "привіт, як справи?");
        // з української в англійську знаки речення лишаються: у лозі «скь???» перетворили на `crm&&&`
        assert_eq!(convert_text("скь???", None).unwrap().text, "crm???");
        assert_eq!(convert_text("іаез: іір, фвьшт.", None).unwrap().text, "sftp: ssh, admin.");
        // а літери на «знакових» клавішах (б, ю) — по клавішах
        assert_eq!(convert_text("руддщб цщкдвю", None).unwrap().text, "hello, world.");
    }

    /// Випадки з логу: `іір?` -> `ssh&`, `іір,` -> `ssh?`, `іаез:` -> `sftp^` — і те, що працювало правильно.
    #[test]
    fn convert_keeps_sentence_punctuation_when_going_to_english() {
        let uk = |s: &str| convert(&keys_for(s, Lang::Uk).unwrap(), Lang::Uk);
        assert_eq!(uk("іір?"), "ssh?");
        assert_eq!(uk("іір,"), "ssh,");
        assert_eq!(uk("іаез:"), "sftp:");
        assert_eq!(uk("іаез."), "sftp.");
        assert_eq!(uk("\"іір\";"), "\"ssh\";");
        assert_eq!(uk("іір?!"), "ssh?!");
        // «ю» і «б» — літери, тож по клавішах: це англійські крапка й кома
        assert_eq!(uk("щзею"), "opt.");
        assert_eq!(uk("іірб"), "ssh,");
        // знак усередині слова — по клавішах; крапка перед словом — це `/`
        assert_eq!(uk("гіук\"фвьшт"), "user@admin");
        assert_eq!(uk(".мщдгьу1"), "/volume1");
        // `№` і «;» перед словом — по клавішах
        assert_eq!(uk("С№"), "C#");
        assert_eq!(uk(";РЩЬУ"), "$HOME");
        // цифри й дефіс однакові в обох розкладках
        assert_eq!(uk("ЛШЕ005"), "KIT005");
    }

    #[test]
    fn convert_to_ukrainian_goes_by_keys() {
        let en = |s: &str| convert(&keys_for(s, Lang::En).unwrap(), Lang::En);
        assert_eq!(en("Ghbdsn?"), "Привіт,");
        assert_eq!(en("pvj;tvj&"), "зможемо?");
        assert_eq!(en("gthtdsh?"), "перевір,");
        assert_eq!(en("[ks,"), "хліб");
        assert_eq!(en("g`znm"), "п'ять");
    }

    #[test]
    fn convert_text_without_letters_is_none() {
        assert!(convert_text("123 .,", None).is_none());
        assert!(convert_text("", None).is_none());
    }

    /// Клавіші слова, набраного з увімкненим CapsLock (без Shift).
    fn with_caps(text_en_lower: &str) -> Vec<Key> {
        keys_for(text_en_lower, Lang::En).unwrap().into_iter().map(|k| Key { caps: true, ..k }).collect()
    }

    #[test]
    fn capslock_affects_ukrainian_letters_on_punctuation_keys() {
        // «ХЛІБ» із CapsLock: клавіші [ k s ,  — в англійській це `[KS,`, в українській усі чотири літери великі
        let keys = with_caps("[ks,");
        assert_eq!(render(&keys, Lang::En), "[KS,");
        assert_eq!(render(&keys, Lang::Uk), "ХЛІБ");
        // те саме для ж є ї ю ґ
        let keys = with_caps(";'].");
        assert_eq!(render(&keys, Lang::En), ";'].");
        assert_eq!(render(&keys, Lang::Uk), "ЖЄЇЮ");
        assert_eq!(char_for(Key { vk: 0xE2, shift: false, caps: true }, Lang::Uk), 'Ґ');
    }

    #[test]
    fn capslock_with_shift_gives_lowercase_and_does_not_touch_digits_or_plain_punctuation() {
        let k = |vk, shift| Key { vk, shift, caps: true };
        assert_eq!(char_for(k(0x4B, true), Lang::En), 'k'); // Shift+K при CapsLock
        assert_eq!(char_for(k(0x4B, true), Lang::Uk), 'л');
        assert_eq!(char_for(k(0xDB, true), Lang::Uk), 'х'); // Shift+Х при CapsLock — мала
        assert_eq!(char_for(k(0xDB, true), Lang::En), '{'); // а в англійській CapsLock на `[` не діє
        assert_eq!(char_for(k(0x35, false), Lang::Uk), '5');
        assert_eq!(char_for(k(0x35, true), Lang::Uk), '%');
        assert_eq!(char_for(k(0xBF, false), Lang::Uk), '.'); // `/` — не літера в жодній розкладці
        assert_eq!(char_for(k(0xC0, false), Lang::Uk), '\''); // апостроф від CapsLock не залежить
    }
}
