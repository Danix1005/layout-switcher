//! Що не можна писати в лог, навіть коли користувач сам увімкнув запис слів (`--log-words`).
//!
//! Поля паролів програма не розпізнає, тож набраний пароль виглядає для неї як звичайне слово. Тут — остання
//! лінія захисту: токен, схожий на пароль чи ключ, у лог не потрапляє (пишемо лише `masked=secret`).
//! Це евристика: пароль із самих літер вона не впізнає. Тому попередження «не набирай паролі під час
//! логування» лишається чинним.

/// Спецсимволи, типові для паролів. Розділові знаки речення (`. , ? : ;`) сюди не входять.
const PASSWORD_SYMBOLS: &str = "!@#$%^&*№";

/// Чи схожий токен на пароль або ключ: є цифра, є щонайменше дві літери поспіль, і при цьому або
/// літер із цифрами разом не менше восьми, або є спецсимвол.
///
/// Не спрацьовує на `KIT005`, `volume1`, `3dlviv` (короткі), `start-config` (без цифр),
/// `192ю168ю1ю10` (літери поодинці між цифрами).
pub fn looks_secret(token: &str) -> bool {
    let mut has_digit = false;
    let mut alnum = 0usize;
    let (mut run, mut max_run) = (0usize, 0usize);
    for c in token.chars() {
        if c.is_alphabetic() {
            run += 1;
            max_run = max_run.max(run);
            alnum += 1;
        } else {
            run = 0;
            if c.is_numeric() {
                has_digit = true;
                alnum += 1;
            }
        }
    }
    has_digit && max_run >= 2 && (alnum >= 8 || token.chars().any(|c| PASSWORD_SYMBOLS.contains(c)))
}

/// Текст для логу: слова, схожі на пароль, замінено на `•••`; пробіли й решта слів лишаються.
pub fn redact(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut token = String::new();
    let flush = |token: &mut String, out: &mut String| {
        if looks_secret(token) {
            out.push_str("•••");
        } else {
            out.push_str(token);
        }
        token.clear();
    };
    for c in text.chars() {
        if c.is_whitespace() {
            flush(&mut token, &mut out);
            out.push(c);
        } else {
            token.push(c);
        }
    }
    flush(&mut token, &mut out);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn password_like_tokens_are_detected() {
        for t in ["Qwerty12!", "Йцукен12!", "admin123", "hunter2!", "P@ssw0rd", "ABCD-1234-EFGH", "пароль2024", "x9$abc"] {
            assert!(looks_secret(t), "{t}");
        }
    }

    #[test]
    fn ordinary_tokens_are_not() {
        for t in [
            "привіт", "hello", "KIT005", "ЛШЕ005", "volume1", "volume1?", "3dlviv", "3D", "wifi6", "start-config",
            "192.168.1.10", "192ю168ю1ю10", "12:00", "2024", "5шт", "ssh?", "sftp:", "user@admin", "C#", "100%",
            "a1b2c3d4", "", "!!!",
        ] {
            assert!(!looks_secret(t), "{t}");
        }
    }

    #[test]
    fn redact_replaces_only_the_secret_words() {
        assert_eq!(redact("user admin pass Qwerty12! ok"), "user admin pass ••• ok");
        assert_eq!(redact("привіт, як справи?"), "привіт, як справи?");
        assert_eq!(redact("Qwerty12!"), "•••");
        assert_eq!(redact("  a\tQwerty12!\n"), "  a\t•••\n");
    }
}
