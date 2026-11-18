//! Правила для застосунків: де перемикач не втручається взагалі і де слова не можна писати в лог.
//! Чиста логіка над іменем exe (без Windows), тому тестується на будь-якій ОС.

/// Що робити у вікні застосунку.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum AppMode {
    /// Звичайна робота: автовиправлення й ручне виправлення.
    Normal,
    /// Не втручаємось: ні автовиправлення, ні подвійного Shift, ні виділеного.
    Off(OffReason),
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum OffReason {
    /// Клієнт віддаленого доступу або віртуальна машина: клавіші йдуть на ІНШИЙ комп'ютер зі своєю розкладкою.
    RemoteClient,
    /// Користувач сам вимкнув цей застосунок у `apps-off.txt` поруч із програмою.
    UserList,
    /// Користувач вимкнув застосунок пунктом меню (`%APPDATA%\layout-switcher\apps-off.txt`).
    Menu,
}

impl OffReason {
    pub fn tag(self) -> &'static str {
        match self {
            OffReason::RemoteClient => "remote_client",
            OffReason::UserList => "user_list",
            OffReason::Menu => "menu",
        }
    }
}

/// Клієнти віддаленого доступу й віртуальні машини (частина імені exe, малими літерами).
///
/// У таких вікнах локальний перемикач нічого вдіяти не може: розкладка віддаленої системи нам невідома,
/// перемкнути її повідомленням вікну неможливо, а виправлений текст, надісланий як Unicode, клієнт може
/// просто не передати. Тому тут ми пасивні — виправляти має екземпляр, запущений на віддаленій стороні.
const REMOTE_CLIENTS: &[&str] = &[
    "mstsc",                // Підключення до віддаленого робочого стола (RDP)
    "msrdc",                // Remote Desktop / Windows App / AVD
    "rdcman",               // Remote Desktop Connection Manager
    "mremoteng",            // mRemoteNG
    "royalts",              // Royal TS
    "remotedesktopmanager", // Devolutions RDM
    "anydesk",
    "teamviewer",
    "rustdesk",
    "radmin",
    "rutview",   // Remote Utilities
    "romviewer", // LiteManager
    "aeroadmin",
    "supremo",
    "parsec",
    "screenconnect",
    "splashtop",
    "strwinclt", // Splashtop
    "dwrcc",     // DameWare
    "cdviewer",  // Citrix
    "wfica32",   // Citrix
    "nxplayer",  // NoMachine
    "vncviewer",
    "tvnviewer", // TightVNC
    "vmconnect", // Hyper-V
    "vmware",
    "vmplayer",
    "virtualboxvm",
    "qemu-system",
    "remote-viewer",
    "virt-viewer",
    "scrcpy",
];

/// Чи є застосунок клієнтом віддаленого доступу / віртуальною машиною.
pub fn is_remote_client(exe: &str) -> bool {
    let n = exe.to_lowercase();
    REMOTE_CLIENTS.iter().any(|k| n.contains(k))
}

/// Процеси, де зазвичай вводять логіни/паролі: для них слова в лог не пишемо ніколи (навіть із `--log-words`),
/// а виділений фрагмент не чіпаємо (у терміналі вставка — це виконання команди).
pub fn is_sensitive(exe: &str) -> bool {
    let n = exe.to_lowercase();
    n.starts_with('?')
        || is_remote_client(&n)
        || [
            "putty", "winbox", "keepass", "bitwarden", "1password", "lastpass", "winscp", "filezilla", "ssh", "vpn",
            "terminal", "cmd.exe", "powershell", "pwsh", "conhost", "mintty", "bash", "rdp", "remote", "login",
            "credential", "password", "authenticator",
        ]
        .iter()
        .any(|k| n.contains(k))
}

/// Правила користувача + вбудовані.
#[derive(Default)]
pub struct AppRules {
    /// Імена exe з `apps-off.txt` поруч із програмою (малими літерами, з `.exe`).
    off: Vec<String>,
    /// Вимкнені з меню значка (файл у %APPDATA%).
    menu_off: Vec<String>,
}

/// Розбір списку застосунків: одне ім'я exe на рядок (`Code.exe` або просто `code`), `#` — коментар.
fn parse_list(text: &str) -> Vec<String> {
    text.trim_start_matches('\u{feff}') // Блокнот може зберегти файл із позначкою BOM на початку
        .lines()
        .filter_map(list_entry)
        .collect()
}

fn list_entry(line: &str) -> Option<String> {
    let l = line.split('#').next().unwrap_or("").trim().to_lowercase();
    if l.is_empty() {
        None
    } else if l.ends_with(".exe") {
        Some(l)
    } else {
        Some(format!("{l}.exe"))
    }
}

/// Вимкнути або знову ввімкнути застосунок у списку (текст файлу): коментарі й інші рядки лишаються як були.
/// Повертає новий текст і чи вимкнено застосунок тепер.
pub fn toggle_in_list(text: &str, exe: &str) -> (String, bool) {
    let name = list_entry(exe).unwrap_or_default();
    let mut found = false;
    let mut out = String::new();
    for line in text.lines() {
        if list_entry(line).as_deref() == Some(name.as_str()) {
            found = true; // прибираємо — застосунок знову ввімкнено
            continue;
        }
        out.push_str(line);
        out.push('\n');
    }
    if !found {
        out.push_str(exe.trim());
        out.push('\n');
    }
    (out, !found)
}

impl AppRules {
    pub fn new() -> Self {
        Self::default()
    }

    /// Розбирає `apps-off.txt`: одне ім'я exe на рядок (`Code.exe` або просто `code`), `#` — коментар.
    pub fn from_user_list(text: &str) -> Self {
        AppRules { off: parse_list(text), menu_off: Vec::new() }
    }

    /// Список, вимкнений з меню (перечитується, коли користувач щось змінив).
    pub fn set_menu_list(&mut self, text: &str) {
        self.menu_off = parse_list(text);
    }

    /// Скільки застосунків вимкнув користувач.
    pub fn user_count(&self) -> usize {
        self.off.len()
    }

    pub fn mode(&self, exe: &str) -> AppMode {
        let n = exe.to_lowercase();
        if self.off.iter().any(|o| *o == n) {
            AppMode::Off(OffReason::UserList)
        } else if self.menu_off.iter().any(|o| *o == n) {
            AppMode::Off(OffReason::Menu)
        } else if is_remote_client(&n) {
            AppMode::Off(OffReason::RemoteClient)
        } else {
            AppMode::Normal
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn remote_clients_are_off_and_ordinary_apps_are_not() {
        let r = AppRules::new();
        for exe in ["mstsc.exe", "AnyDesk.exe", "TeamViewer.exe", "vmconnect.exe", "VirtualBoxVM.exe", "mRemoteNG.exe"] {
            assert_eq!(r.mode(exe), AppMode::Off(OffReason::RemoteClient), "{exe}");
        }
        for exe in ["chrome.exe", "ms-teams.exe", "ChatGPT.exe", "WINWORD.EXE", "WinBox.exe", "putty.exe", "explorer.exe"] {
            assert_eq!(r.mode(exe), AppMode::Normal, "{exe}");
        }
    }

    #[test]
    fn user_list_is_exact_case_insensitive_and_extension_is_optional() {
        let r = AppRules::from_user_list("# мої винятки\nCode.exe\n  idea64   # IntelliJ\n\nWindowsTerminal.exe\n");
        assert_eq!(r.user_count(), 3);
        assert_eq!(r.mode("code.exe"), AppMode::Off(OffReason::UserList));
        assert_eq!(r.mode("IDEA64.EXE"), AppMode::Off(OffReason::UserList));
        assert_eq!(r.mode("windowsterminal.exe"), AppMode::Off(OffReason::UserList));
        // файл, збережений Блокнотом із BOM: перший рядок не має зіпсуватись
        assert_eq!(AppRules::from_user_list("\u{feff}Code.exe\r\n").mode("code.exe"), AppMode::Off(OffReason::UserList));
        // точний збіг, а не частина імені
        assert_eq!(r.mode("vscode.exe"), AppMode::Normal);
        assert_eq!(r.mode("code-insiders.exe"), AppMode::Normal);
    }

    #[test]
    fn menu_toggle_adds_and_removes_keeping_the_rest() {
        let (t, off) = toggle_in_list("", "Viber.exe");
        assert!(off);
        assert_eq!(t, "Viber.exe\n");
        let (t, off) = toggle_in_list(&format!("# мої\n{t}code.exe\n"), "viber.EXE");
        assert!(!off);
        assert_eq!(t, "# мої\ncode.exe\n");
        let mut r = AppRules::from_user_list("");
        r.set_menu_list("Viber.exe\n");
        assert_eq!(r.mode("viber.exe"), AppMode::Off(OffReason::Menu));
        r.set_menu_list("");
        assert_eq!(r.mode("viber.exe"), AppMode::Normal);
    }

    #[test]
    fn sensitive_covers_remote_clients_terminals_and_password_tools() {
        for exe in ["putty.exe", "WinBox.exe", "mstsc.exe", "AnyDesk.exe", "WindowsTerminal.exe", "KeePass.exe", "?(no access)"] {
            assert!(is_sensitive(exe), "{exe}");
        }
        for exe in ["chrome.exe", "ms-teams.exe", "ChatGPT.exe", "Viber.exe", "notepad.exe"] {
            assert!(!is_sensitive(exe), "{exe}");
        }
    }
}
