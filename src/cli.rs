//! Параметри командного рядка. Чиста логіка без Windows — тестується на будь-якій ОС.
//!
//! Програма не має вікна консолі, тож розбір окремо від `main`: він лише повертає, що користувач просив,
//! а як показати довідку чи помилку (вікно повідомлення), вирішує `main`.

use crate::inject::InjectedMode;
use crate::session::Embed;

/// Параметри звичайного запуску.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Options {
    /// Писати діагностичний лог.
    pub log: bool,
    /// Писати в лог і набрані слова.
    pub log_words: bool,
    pub injected: InjectedMode,
    pub switch: Embed,
    /// Відкрити вікно консолі з повідомленнями (для налагодження).
    pub console: bool,
    /// Не показувати підказку при запуску (так запускає автозапуск з Windows).
    pub quiet: bool,
    /// Службовий параметр: так програма запускає себе після оновлення. Число — номер процесу старої версії,
    /// якого треба дочекатись (він ще тримає значок і «один екземпляр на сеанс»).
    pub after_update: Option<u32>,
}

impl Default for Options {
    fn default() -> Self {
        Options {
            log: false,
            log_words: false,
            injected: InjectedMode::Auto,
            switch: Embed::default(),
            console: false,
            quiet: false,
            after_update: None,
        }
    }
}

impl Options {
    /// Параметри, які треба пам'ятати в записі автозапуску: режими роботи. Лог і консоль — разові, їх не беремо.
    /// `--quiet` додаємо завжди: при вході в Windows підказка про значок не потрібна.
    pub fn autostart_args(&self) -> Vec<String> {
        let mut v = Vec::new();
        if self.injected != InjectedMode::Auto {
            v.push(format!("--injected={}", self.injected.tag()));
        }
        if self.switch != Embed::default() {
            v.push(format!("--switch={}", self.switch.tag()));
        }
        v.push("--quiet".to_string());
        v
    }
}

/// Команда для вже запущеного екземпляра.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Control {
    Exit,
    Pause,
    Resume,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Command {
    Run(Options),
    Help,
    Version,
    Control(Control),
}

/// Розібрати аргументи (без імені програми). `Err` — текст помилки.
pub fn parse<I: IntoIterator<Item = String>>(args: I) -> Result<Command, String> {
    let mut o = Options::default();
    for a in args {
        match a.as_str() {
            "--log" => o.log = true,
            "--log-words" => {
                o.log = true;
                o.log_words = true;
            }
            "--accept-injected" => o.injected = InjectedMode::Accept,
            "--console" => o.console = true,
            "--quiet" => o.quiet = true,
            "--exit" => return Ok(Command::Control(Control::Exit)),
            "--pause" => return Ok(Command::Control(Control::Pause)),
            "--resume" => return Ok(Command::Control(Control::Resume)),
            "--version" => return Ok(Command::Version),
            "--help" | "-h" | "/?" => return Ok(Command::Help),
            other => {
                if let Some(m) = other.strip_prefix("--injected=").and_then(InjectedMode::parse) {
                    o.injected = m;
                } else if let Some(m) = other.strip_prefix("--switch=").and_then(Embed::parse) {
                    o.switch = m;
                } else if let Some(pid) = other.strip_prefix("--after-update=").and_then(|p| p.parse::<u32>().ok()) {
                    o.after_update = Some(pid);
                } else {
                    return Err(format!("Невідомий параметр: {other}"));
                }
            }
        }
    }
    Ok(Command::Run(o))
}

/// Текст довідки. Показується у вікні повідомлення з пропорційним шрифтом, тож без вирівнювання стовпчиків
/// і з короткими рядками.
pub fn help_text(version: &str) -> String {
    format!(
        "layout-switcher {version} — автоперемикач розкладки EN <-> UK\n\
         \n\
         Працює без вікна: значок біля годинника, меню — права кнопка миші.\n\
         \n\
         Параметри запуску:\n\
         --log — діагностичний лог без набраних слів\n\
         --log-words — лог із набраними словами (обережно з паролями)\n\
         --switch=smart — англійське слово посеред українського тексту виправляти без перемикання розкладки (типово)\n\
         --switch=always — перемикати розкладку після кожного виправлення\n\
         --switch=smart-both — те саме, що smart, але в обидва боки\n\
         --injected=auto | accept | ignore — чужий програмний ввід (віддалене керування)\n\
         --console — показати вікно консолі з повідомленнями (для налагодження)\n\
         --quiet — не показувати підказку при запуску\n\
         --after-update=N — службовий: так програма запускає себе після оновлення\n\
         \n\
         Керування вже запущеною програмою:\n\
         --pause — призупинити автовиправлення\n\
         --resume — відновити\n\
         --exit — завершити роботу\n\
         \n\
         Оновлення: меню значка — «Оновлення» (перевірка раз на добу з GitHub, установка лише за вашим кліком).\n\
         Поруч з exe: тека dict\\ (словники), apps-off.txt (застосунки, де не втручатись)."
    )
}

/// Рядок запуску для запису автозапуску: шлях у лапках і параметри.
pub fn quote_command(exe: &str, args: &[String]) -> String {
    let mut s = format!("\"{exe}\"");
    for a in args {
        s.push(' ');
        s.push_str(a);
    }
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    fn p(args: &[&str]) -> Result<Command, String> {
        parse(args.iter().map(|s| s.to_string()))
    }

    #[test]
    fn no_arguments_is_a_plain_run() {
        assert_eq!(p(&[]), Ok(Command::Run(Options::default())));
    }

    #[test]
    fn options_are_parsed() {
        let Ok(Command::Run(o)) = p(&["--log-words", "--switch=always", "--injected=ignore", "--quiet", "--console"]) else {
            panic!("очікував Run");
        };
        assert!(o.log && o.log_words && o.quiet && o.console);
        assert_eq!(o.switch, Embed::Off);
        assert_eq!(o.injected, InjectedMode::Ignore);
        let Ok(Command::Run(o)) = p(&["--log"]) else { panic!() };
        assert!(o.log && !o.log_words);
    }

    #[test]
    fn controls_help_and_version_win_over_everything() {
        assert_eq!(p(&["--exit"]), Ok(Command::Control(Control::Exit)));
        assert_eq!(p(&["--log", "--pause"]), Ok(Command::Control(Control::Pause)));
        assert_eq!(p(&["--resume"]), Ok(Command::Control(Control::Resume)));
        assert_eq!(p(&["--help"]), Ok(Command::Help));
        assert_eq!(p(&["/?"]), Ok(Command::Help));
        assert_eq!(p(&["--version"]), Ok(Command::Version));
    }

    #[test]
    fn unknown_or_malformed_options_are_errors() {
        assert!(p(&["--bogus"]).unwrap_err().contains("--bogus"));
        assert!(p(&["--switch=sometimes"]).is_err());
        assert!(p(&["--injected="]).is_err());
    }

    #[test]
    fn autostart_keeps_modes_but_not_log_or_console() {
        let Ok(Command::Run(o)) = p(&["--log-words", "--console", "--switch=always", "--injected=accept"]) else { panic!() };
        assert_eq!(o.autostart_args(), vec!["--injected=accept", "--switch=always", "--quiet"]);
        assert_eq!(Options::default().autostart_args(), vec!["--quiet"]);
    }

    #[test]
    fn after_update_is_parsed_and_never_goes_to_autostart() {
        let Ok(Command::Run(o)) = p(&["--quiet", "--after-update=4242", "--switch=always"]) else { panic!() };
        assert_eq!(o.after_update, Some(4242));
        assert!(!o.autostart_args().iter().any(|a| a.contains("after-update")));
        assert!(p(&["--after-update=abc"]).is_err());
        assert!(p(&["--after-update="]).is_err());
        let Ok(Command::Run(o)) = p(&[]) else { panic!() };
        assert_eq!(o.after_update, None);
    }

    #[test]
    fn autostart_arguments_round_trip_through_the_parser() {
        let o = Options { switch: Embed::Both, injected: InjectedMode::Ignore, ..Options::default() };
        let Ok(Command::Run(back)) = parse(o.autostart_args()) else { panic!() };
        assert_eq!((back.switch, back.injected, back.quiet), (Embed::Both, InjectedMode::Ignore, true));
    }

    #[test]
    fn command_line_quotes_the_path_only() {
        let c = quote_command("C:\\Program Files\\ls\\layout-switcher.exe", &["--switch=always".into(), "--quiet".into()]);
        assert_eq!(c, "\"C:\\Program Files\\ls\\layout-switcher.exe\" --switch=always --quiet");
    }

    #[test]
    fn help_mentions_every_option() {
        let h = help_text("9.9.9");
        for k in [
            "--log", "--log-words", "--switch", "--injected", "--console", "--quiet", "--pause", "--resume", "--exit",
            "--after-update", "9.9.9",
        ] {
            assert!(h.contains(k), "{k}");
        }
    }
}
