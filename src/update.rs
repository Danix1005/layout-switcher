//! Оновлення з GitHub: уся логіка без Windows і без мережі (версії, розбір відповіді про реліз, перевірка
//! адрес, контрольні суми, розклад перевірок). Саму мережу робить `http.rs`, заміну файлу — `updater.rs`.
//!
//! Довіра: програма слухає клавіатуру, тому завантажений файл ніколи не запускається без перевірки.
//! Перевіряємо: адреса файлу лежить у релізах саме цього репозиторію (https), розмір збігається з тим, що
//! заявлено у відповіді GitHub, а SHA-256 — зі списком `SHA256SUMS` з того самого релізу.

use crate::json::{self, Value};
use crate::sha256;

/// Ім'я exe серед файлів релізу.
pub const EXE_ASSET: &str = "layout-switcher.exe";
/// Ім'я файлу з контрольними сумами серед файлів релізу.
pub const SUMS_ASSET: &str = "SHA256SUMS";
/// Якщо ця позначка є в описі релізу, оновити одним exe не можна (змінились словники): програма лише відкриє
/// сторінку релізу, де лежить повний пакет.
pub const FULL_PACKAGE_MARK: &str = "[full-package]";
/// Як часто перевіряти оновлення.
pub const CHECK_INTERVAL_SECS: u64 = 24 * 3600;
/// Верхня межа розміру exe, який погоджуємось завантажити.
pub const MAX_EXE_BYTES: usize = 32 * 1024 * 1024;

// ───────────────────────────── версії ─────────────────────────────

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct Version(pub u32, pub u32, pub u32);

impl Version {
    /// `0.9.1` або `v0.9.1`. Усе інше (`0.9`, `0.9.1-rc1`, `1.2.3.4`) — не наша версія, оновлюватись на неї не будемо.
    pub fn parse(s: &str) -> Option<Version> {
        let s = s.trim();
        let s = s.strip_prefix('v').or_else(|| s.strip_prefix('V')).unwrap_or(s);
        let mut it = s.split('.');
        let mut next = || {
            let p = it.next()?;
            if p.is_empty() || p.len() > 6 || !p.bytes().all(|b| b.is_ascii_digit()) {
                return None;
            }
            p.parse::<u32>().ok()
        };
        let v = Version(next()?, next()?, next()?);
        if it.next().is_some() {
            return None;
        }
        Some(v)
    }
}

impl std::fmt::Display for Version {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}.{}.{}", self.0, self.1, self.2)
    }
}

// ───────────────────────────── джерело ─────────────────────────────

/// Звідки беремо оновлення і яким адресам довіряємо.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Source {
    /// Адреса «останнього релізу» (JSON).
    pub api_url: String,
    /// Усе, що завантажуємо, має починатися з цього (файли релізів репозиторію).
    pub download_prefix: String,
    /// Сторінки репозиторію (для кнопки «відкрити сторінку релізу»).
    pub page_prefix: String,
    /// Дозволити http (лише для перевірки на локальному сервері; у справжній збірці завжди `false`).
    pub allow_http: bool,
}

impl Source {
    pub fn github(owner: &str, repo: &str) -> Source {
        Source {
            api_url: format!("https://api.github.com/repos/{owner}/{repo}/releases/latest"),
            download_prefix: format!("https://github.com/{owner}/{repo}/releases/download/"),
            page_prefix: format!("https://github.com/{owner}/{repo}/"),
            allow_http: false,
        }
    }

    /// Адреса схожа на дозволену схему (https, а http — лише коли дозволено).
    pub fn scheme_ok(&self, url: &str) -> bool {
        url.starts_with("https://") || (self.allow_http && url.starts_with("http://"))
    }

    /// Чи можна завантажувати файл за цією адресою.
    pub fn download_allowed(&self, url: &str) -> bool {
        self.scheme_ok(url) && starts_with_ci(url, &self.download_prefix) && !url.contains(['\r', '\n', ' ', '"'])
    }
}

/// Початок адреси збігається з `prefix`, не зважаючи на регістр літер: GitHub не розрізняє регістр у `власник/репозиторій`
/// і віддає ім'я так, як воно написане в налаштуваннях репозиторію (а в `Cargo.toml` його могли написати інакше).
fn starts_with_ci(url: &str, prefix: &str) -> bool {
    url.len() >= prefix.len() && url.as_bytes()[..prefix.len()].eq_ignore_ascii_case(prefix.as_bytes())
}

/// `https://github.com/власник/назва[.git][/]` -> (власник, назва). `OWNER` — це заповнювач у Cargo.toml,
/// доки справжню адресу не вписано, оновлення вимкнені.
pub fn repo_from_url(url: &str) -> Option<(String, String)> {
    let rest = url.trim().strip_prefix("https://github.com/")?;
    let mut parts = rest.trim_end_matches('/').split('/');
    let owner = parts.next()?;
    let repo = parts.next()?.trim_end_matches(".git");
    if parts.next().is_some() {
        return None;
    }
    let ok = |s: &str| {
        !s.is_empty() && s.len() <= 100 && s != "." && s != ".." && s.bytes().all(|b| b.is_ascii_alphanumeric() || b"-_.".contains(&b))
    };
    if !ok(owner) || !ok(repo) || owner == "OWNER" {
        return None;
    }
    Some((owner.to_string(), repo.to_string()))
}

// ───────────────────────────── реліз ─────────────────────────────

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Asset {
    pub name: String,
    pub url: String,
    pub size: u64,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Release {
    pub version: Version,
    pub tag: String,
    pub page_url: String,
    pub notes: String,
    pub assets: Vec<Asset>,
}

/// Розібрати відповідь `GET /repos/{власник}/{репо}/releases/latest`.
pub fn parse_release(text: &str) -> Result<Release, String> {
    let v = json::parse(text)?;
    if v.get("draft").and_then(Value::as_bool) == Some(true) || v.get("prerelease").and_then(Value::as_bool) == Some(true) {
        return Err("останній реліз — чернетка або пробна версія".into());
    }
    let tag = v.get("tag_name").and_then(Value::as_str).ok_or("у відповіді нема tag_name")?;
    let version = Version::parse(tag).ok_or_else(|| format!("тег релізу «{tag}» не схожий на версію X.Y.Z"))?;
    let page_url = v.get("html_url").and_then(Value::as_str).unwrap_or("").to_string();
    let notes = v.get("body").and_then(Value::as_str).unwrap_or("").to_string();
    let mut assets = Vec::new();
    for a in v.get("assets").and_then(Value::as_array).unwrap_or(&[]) {
        let (Some(name), Some(url)) = (a.get("name").and_then(Value::as_str), a.get("browser_download_url").and_then(Value::as_str))
        else {
            continue;
        };
        assets.push(Asset { name: name.to_string(), url: url.to_string(), size: a.get("size").and_then(Value::as_u64).unwrap_or(0) });
    }
    Ok(Release { version, tag: tag.to_string(), page_url, notes, assets })
}

/// Те, що програма пропонує встановити.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Update {
    pub version: Version,
    pub tag: String,
    pub page_url: String,
    pub notes: String,
    pub exe_url: String,
    pub exe_size: u64,
    pub sums_url: String,
    /// Оновити одним exe не можна — тільки повним пакетом зі сторінки релізу.
    pub full_package: bool,
}

/// Чи є реліз новішим за поточну версію і чи його можна безпечно встановити.
/// `Ok(None)` — оновлень нема. `Err` — реліз новіший, але зіпсований (нема файлів або адреси чужі).
pub fn evaluate(current: &str, rel: &Release, src: &Source) -> Result<Option<Update>, String> {
    let cur = Version::parse(current).ok_or_else(|| format!("власна версія «{current}» не схожа на X.Y.Z"))?;
    if rel.version <= cur {
        return Ok(None);
    }
    let page_url = if starts_with_ci(&rel.page_url, &src.page_prefix) && src.scheme_ok(&rel.page_url) && !rel.page_url.contains(['\r', '\n', ' ', '"']) {
        rel.page_url.clone()
    } else {
        format!("{}releases/latest", src.page_prefix)
    };
    let full_package = rel.notes.contains(FULL_PACKAGE_MARK);
    let find = |name: &str| rel.assets.iter().find(|a| a.name == name);
    let (exe, sums) = match (find(EXE_ASSET), find(SUMS_ASSET)) {
        (Some(e), Some(s)) => (e, s),
        _ if full_package => {
            // повний пакет: файл не завантажуємо, потрібна лише сторінка
            return Ok(Some(Update {
                version: rel.version,
                tag: rel.tag.clone(),
                page_url,
                notes: rel.notes.clone(),
                exe_url: String::new(),
                exe_size: 0,
                sums_url: String::new(),
                full_package: true,
            }));
        }
        _ => return Err(format!("у релізі {} нема файлів {EXE_ASSET} і {SUMS_ASSET}", rel.tag)),
    };
    if !src.download_allowed(&exe.url) || !src.download_allowed(&sums.url) {
        return Err(format!("адреси файлів релізу {} не ведуть у релізи цього репозиторію", rel.tag));
    }
    if exe.size > MAX_EXE_BYTES as u64 {
        return Err(format!("{EXE_ASSET} у релізі {} надто великий ({} байтів)", rel.tag, exe.size));
    }
    Ok(Some(Update {
        version: rel.version,
        tag: rel.tag.clone(),
        page_url,
        notes: rel.notes.clone(),
        exe_url: exe.url.clone(),
        exe_size: exe.size,
        sums_url: sums.url.clone(),
        full_package,
    }))
}

// ───────────────────────────── контрольні суми ─────────────────────────────

/// Сума для файлу з вмісту `SHA256SUMS` (формат `sha256sum`: `<64 цифри>  <ім'я>` або `<64 цифри> *<ім'я>`).
/// Якщо файл згаданий кілька разів із різними сумами — це підозріло, повертаємо `None`.
pub fn sum_for(sums: &str, name: &str) -> Option<[u8; 32]> {
    let mut found: Option<[u8; 32]> = None;
    for line in sums.lines() {
        let line = line.trim_end_matches('\r');
        let Some((hash, rest)) = line.split_once(char::is_whitespace) else { continue };
        let file = rest.trim_start().trim_start_matches('*');
        // ім'я може мати префікс шляху (`dist/release/layout-switcher.exe`)
        let base = file.rsplit(['/', '\\']).next().unwrap_or(file);
        if base != name {
            continue;
        }
        let h = sha256::parse_hex(hash)?;
        if found.is_some_and(|f| f != h) {
            return None;
        }
        found = Some(h);
    }
    found
}

/// Перевірка завантаженого файлу: розмір, заголовок exe, SHA-256.
pub fn verify_exe(data: &[u8], expected_size: u64, expected: &[u8; 32]) -> Result<(), String> {
    if expected_size != 0 && data.len() as u64 != expected_size {
        return Err(format!("розмір файлу {} байтів, а в релізі заявлено {expected_size}", data.len()));
    }
    if data.len() < 1024 || &data[..2] != b"MZ" {
        return Err("завантажений файл не схожий на програму Windows".into());
    }
    let got = sha256::digest(data);
    if &got != expected {
        return Err(format!(
            "контрольна сума SHA-256 не збіглася (отримано {}, у релізі {})",
            sha256::to_hex(&got),
            sha256::to_hex(expected)
        ));
    }
    Ok(())
}

// ───────────────────────────── розклад ─────────────────────────────

/// Чи настав час перевіряти. `last` — коли востаннє перевіряли (секунди Unix). Якщо годинник пішов назад
/// (`last` у майбутньому більш ніж на добу), теж перевіряємо.
pub fn due(now: u64, last: Option<u64>) -> bool {
    match last {
        None => true,
        Some(l) if l > now + CHECK_INTERVAL_SECS => true,
        Some(l) => now.saturating_sub(l) >= CHECK_INTERVAL_SECS,
    }
}

// ───────────────────────────── текст ─────────────────────────────

/// Початок опису релізу для вікна підтвердження: без розмітки заголовків, не довший за `max` символів.
pub fn short_notes(notes: &str, max: usize) -> String {
    let cleaned: Vec<String> = notes
        .lines()
        .map(|l| l.trim_end().trim_start_matches('#').trim_start().replace("**", "").replace('`', ""))
        .filter(|l| !l.contains(FULL_PACKAGE_MARK))
        .collect();
    let joined = cleaned.join("\n");
    let joined = joined.trim();
    if joined.chars().count() <= max {
        return joined.to_string();
    }
    let cut: String = joined.chars().take(max).collect();
    // не рвемо посеред рядка, якщо є куди відступити
    let cut = match cut.rfind('\n') {
        Some(i) if i > max / 2 => cut[..i].to_string(),
        _ => cut,
    };
    format!("{}\n…", cut.trim_end())
}

/// Частини адреси для WinHTTP.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct UrlParts {
    pub https: bool,
    pub host: String,
    pub port: u16,
    /// Шлях із запитом, завжди починається з `/`.
    pub path: String,
}

pub fn split_url(url: &str) -> Option<UrlParts> {
    let (https, rest) = if let Some(r) = url.strip_prefix("https://") {
        (true, r)
    } else {
        (false, url.strip_prefix("http://")?)
    };
    let (authority, path) = match rest.find(['/', '?', '#']) {
        Some(i) => (&rest[..i], &rest[i..]),
        None => (rest, "/"),
    };
    let path = path.split('#').next().unwrap_or("/");
    let path = if path.starts_with('/') { path.to_string() } else { format!("/{path}") };
    if authority.is_empty() || authority.contains(['@', ' ', '\\']) {
        return None;
    }
    let (host, port) = match authority.rsplit_once(':') {
        Some((h, p)) => (h, p.parse::<u16>().ok()?),
        None => (authority, if https { 443 } else { 80 }),
    };
    if host.is_empty() || !host.bytes().all(|b| b.is_ascii_alphanumeric() || b"-._".contains(&b)) {
        return None;
    }
    Some(UrlParts { https, host: host.to_string(), port, path })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn src() -> Source {
        Source::github("acme", "layout-switcher")
    }

    fn release_json(tag: &str, notes: &str, exe_url: &str, sums_url: &str) -> String {
        format!(
            r#"{{"tag_name":"{tag}","html_url":"https://github.com/acme/layout-switcher/releases/tag/{tag}","draft":false,
            "prerelease":false,"body":"{notes}","assets":[
            {{"name":"layout-switcher.exe","size":2048,"browser_download_url":"{exe_url}"}},
            {{"name":"SHA256SUMS","size":200,"browser_download_url":"{sums_url}"}},
            {{"name":"layout-switcher-windows.zip","size":99,"browser_download_url":"https://github.com/acme/layout-switcher/releases/download/{tag}/z.zip"}}]}}"#
        )
    }

    fn good_json(tag: &str) -> String {
        let base = format!("https://github.com/acme/layout-switcher/releases/download/{tag}");
        release_json(tag, "Що нового", &format!("{base}/layout-switcher.exe"), &format!("{base}/SHA256SUMS"))
    }

    #[test]
    fn versions_parse_and_compare() {
        assert_eq!(Version::parse("0.9.1"), Some(Version(0, 9, 1)));
        assert_eq!(Version::parse("v1.10.0"), Some(Version(1, 10, 0)));
        assert!(Version::parse("v0.10.0") > Version::parse("v0.9.9"));
        assert!(Version::parse("1.0.0") > Version::parse("0.99.99"));
        for bad in ["", "1", "1.2", "1.2.3.4", "1.2.3-rc1", "v", "a.b.c", "1..3", "1.2.-3", "1.2.3 ", "99999999.0.0"] {
            if bad == "1.2.3 " {
                assert_eq!(Version::parse(bad), Some(Version(1, 2, 3))); // пробіли по краях відкидаємо
            } else {
                assert_eq!(Version::parse(bad), None, "{bad:?}");
            }
        }
        assert_eq!(Version(0, 9, 1).to_string(), "0.9.1");
    }

    #[test]
    fn repository_url_parsing() {
        assert_eq!(repo_from_url("https://github.com/acme/layout-switcher"), Some(("acme".into(), "layout-switcher".into())));
        assert_eq!(repo_from_url("https://github.com/acme/layout-switcher.git/"), Some(("acme".into(), "layout-switcher".into())));
        assert_eq!(repo_from_url("https://github.com/OWNER/layout-switcher"), None); // заповнювач
        assert_eq!(repo_from_url(""), None);
        assert_eq!(repo_from_url("http://github.com/a/b"), None);
        assert_eq!(repo_from_url("https://evil.example/a/b"), None);
        assert_eq!(repo_from_url("https://github.com/a"), None);
        assert_eq!(repo_from_url("https://github.com/a/b/c"), None);
        assert_eq!(repo_from_url("https://github.com/a/../b"), None);
        assert_eq!(repo_from_url("https://github.com/a b/c"), None);
    }

    #[test]
    fn newer_release_is_offered_with_trusted_urls() {
        let rel = parse_release(&good_json("v0.9.1")).unwrap();
        let u = evaluate("0.9.0", &rel, &src()).unwrap().unwrap();
        assert_eq!(u.version, Version(0, 9, 1));
        assert_eq!(u.exe_url, "https://github.com/acme/layout-switcher/releases/download/v0.9.1/layout-switcher.exe");
        assert_eq!(u.sums_url, "https://github.com/acme/layout-switcher/releases/download/v0.9.1/SHA256SUMS");
        assert_eq!(u.exe_size, 2048);
        assert!(!u.full_package);
        assert!(u.page_url.ends_with("/releases/tag/v0.9.1"));
    }

    #[test]
    fn same_or_older_release_means_up_to_date() {
        for tag in ["v0.9.0", "v0.8.9", "v0.1.0"] {
            let rel = parse_release(&good_json(tag)).unwrap();
            assert_eq!(evaluate("0.9.0", &rel, &src()), Ok(None), "{tag}");
        }
    }

    #[test]
    fn foreign_download_addresses_are_refused() {
        let ok = "https://github.com/acme/layout-switcher/releases/download/v0.9.1/SHA256SUMS";
        for exe in [
            "https://evil.example/layout-switcher.exe",
            "https://github.com/other/layout-switcher/releases/download/v0.9.1/layout-switcher.exe",
            "https://github.com/acme/layout-switcher/raw/main/layout-switcher.exe",
            "http://github.com/acme/layout-switcher/releases/download/v0.9.1/layout-switcher.exe",
            "https://github.com/acme/layout-switcher/releases/download/v0.9.1/a b.exe",
            "https://github.com/acme/layout-switcher-evil/releases/download/v0.9.1/layout-switcher.exe",
        ] {
            let rel = parse_release(&release_json("v0.9.1", "", exe, ok)).unwrap();
            assert!(evaluate("0.9.0", &rel, &src()).is_err(), "{exe}");
        }
    }

    #[test]
    fn missing_assets_is_an_error_unless_full_package_is_required() {
        let rel = parse_release(r#"{"tag_name":"v0.9.1","body":"x","assets":[]}"#).unwrap();
        assert!(evaluate("0.9.0", &rel, &src()).is_err());
        let rel = parse_release(r#"{"tag_name":"v0.9.1","html_url":"https://github.com/acme/layout-switcher/releases/tag/v0.9.1","body":"Нові словники [full-package]","assets":[]}"#).unwrap();
        let u = evaluate("0.9.0", &rel, &src()).unwrap().unwrap();
        assert!(u.full_package && u.exe_url.is_empty());
    }

    #[test]
    fn full_package_mark_wins_even_when_exe_is_present() {
        let base = "https://github.com/acme/layout-switcher/releases/download/v0.9.1";
        let rel = parse_release(&release_json("v0.9.1", "Нові словники [full-package]", &format!("{base}/layout-switcher.exe"), &format!("{base}/SHA256SUMS"))).unwrap();
        assert!(evaluate("0.9.0", &rel, &src()).unwrap().unwrap().full_package);
    }

    #[test]
    fn foreign_page_address_falls_back_to_the_releases_page() {
        let base = "https://github.com/acme/layout-switcher/releases/download/v0.9.1";
        let mut rel = parse_release(&release_json("v0.9.1", "", &format!("{base}/layout-switcher.exe"), &format!("{base}/SHA256SUMS"))).unwrap();
        rel.page_url = "https://evil.example/phish".into();
        let u = evaluate("0.9.0", &rel, &src()).unwrap().unwrap();
        assert_eq!(u.page_url, "https://github.com/acme/layout-switcher/releases/latest");
    }

    #[test]
    fn drafts_prereleases_and_odd_tags_are_not_offered() {
        assert!(parse_release(r#"{"tag_name":"v0.9.1","draft":true}"#).is_err());
        assert!(parse_release(r#"{"tag_name":"v0.9.1","prerelease":true}"#).is_err());
        assert!(parse_release(r#"{"tag_name":"nightly"}"#).is_err());
        assert!(parse_release(r#"{"tag_name":"v0.9.1-rc1"}"#).is_err());
        assert!(parse_release("не json").is_err());
        assert!(parse_release("{}").is_err());
    }

    #[test]
    fn sums_file_is_read_in_sha256sum_format() {
        let a = "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad";
        let b = "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855";
        let text = format!("{a}  layout-switcher.exe\r\n{b} *layout-switcher-windows.zip\n\n");
        assert_eq!(sum_for(&text, "layout-switcher.exe"), sha256::parse_hex(a));
        assert_eq!(sum_for(&text, "layout-switcher-windows.zip"), sha256::parse_hex(b));
        assert_eq!(sum_for(&text, "other.exe"), None);
        // шлях перед ім'ям
        assert_eq!(sum_for(&format!("{a}  dist/release/layout-switcher.exe\n"), "layout-switcher.exe"), sha256::parse_hex(a));
        // ім'я лише схоже
        assert_eq!(sum_for(&format!("{a}  my-layout-switcher.exe\n"), "layout-switcher.exe"), None);
        // зіпсований хеш
        assert_eq!(sum_for("xyz  layout-switcher.exe\n", "layout-switcher.exe"), None);
        // два різні записи для одного файлу — підозріло
        assert_eq!(sum_for(&format!("{a}  layout-switcher.exe\n{b}  layout-switcher.exe\n"), "layout-switcher.exe"), None);
        // однакові — нічого страшного
        assert_eq!(sum_for(&format!("{a}  layout-switcher.exe\n{a}  layout-switcher.exe\n"), "layout-switcher.exe"), sha256::parse_hex(a));
    }

    #[test]
    fn downloaded_file_is_verified() {
        let mut data = b"MZ".to_vec();
        data.extend((0..4000u32).map(|i| (i % 251) as u8));
        let sum = sha256::digest(&data);
        assert!(verify_exe(&data, data.len() as u64, &sum).is_ok());
        assert!(verify_exe(&data, 0, &sum).is_ok()); // розмір у релізі невідомий
        assert!(verify_exe(&data, data.len() as u64 + 1, &sum).unwrap_err().contains("розмір"));
        let mut bad = data.clone();
        bad[2000] ^= 1;
        assert!(verify_exe(&bad, data.len() as u64, &sum).unwrap_err().contains("SHA-256"));
        let mut not_pe = data.clone();
        not_pe[0] = b'X';
        let s2 = sha256::digest(&not_pe);
        assert!(verify_exe(&not_pe, 0, &s2).unwrap_err().contains("не схожий"));
        assert!(verify_exe(b"MZ", 0, &sha256::digest(b"MZ")).is_err()); // надто малий
    }

    #[test]
    fn check_schedule() {
        let day = CHECK_INTERVAL_SECS;
        assert!(due(1_000_000, None));
        assert!(!due(1_000_000, Some(1_000_000 - day + 1)));
        assert!(due(1_000_000, Some(1_000_000 - day)));
        assert!(!due(1_000_000, Some(1_000_000)));
        assert!(!due(1_000_000, Some(1_000_000 + 3600))); // трохи наперед — ще ні
        assert!(due(1_000_000, Some(1_000_000 + day + 1))); // годинник пішов назад
    }

    #[test]
    fn notes_are_shortened_for_the_dialog() {
        assert_eq!(short_notes("## 0.9.1\n\n- **Нове**: `оновлення`\n", 500), "0.9.1\n\n- Нове: оновлення");
        let long = (1..=100).map(|i| format!("- рядок номер {i}")).collect::<Vec<_>>().join("\n");
        let s = short_notes(&long, 200);
        assert!(s.chars().count() <= 205 && s.ends_with('…'));
        assert!(!s.contains("рядок номер 100"));
        assert!(!short_notes("Нові словники [full-package]\nрядок", 100).contains("full-package"));
        assert_eq!(short_notes("", 50), "");
    }

    #[test]
    fn urls_are_split_for_winhttp() {
        let u = split_url("https://api.github.com/repos/a/b/releases/latest").unwrap();
        assert_eq!((u.https, u.host.as_str(), u.port, u.path.as_str()), (true, "api.github.com", 443, "/repos/a/b/releases/latest"));
        let u = split_url("http://127.0.0.1:8080/x?y=1#frag").unwrap();
        assert_eq!((u.https, u.host.as_str(), u.port, u.path.as_str()), (false, "127.0.0.1", 8080, "/x?y=1"));
        assert_eq!(split_url("https://example.com").unwrap().path, "/");
        assert_eq!(split_url("https://example.com?q=1").unwrap().path, "/?q=1");
        for bad in ["ftp://x/y", "https://", "https://user@host/x", "https://ho st/x", "https://host:99999/x", "https://host:/x", "//host/x", "https://ho\\st/"] {
            assert_eq!(split_url(bad), None, "{bad:?}");
        }
    }

    #[test]
    fn repository_name_case_does_not_matter_but_the_name_itself_does() {
        // у Cargo.toml написано Danix1005/layout-switcher, а GitHub віддав danix1005/Layout-Switcher
        let s = Source::github("Danix1005", "layout-switcher");
        assert!(s.download_allowed("https://github.com/danix1005/Layout-Switcher/releases/download/v1.0.0/layout-switcher.exe"));
        assert!(s.download_allowed("https://github.com/Danix1005/layout-switcher/releases/download/v1.0.0/SHA256SUMS"));
        assert!(!s.download_allowed("https://github.com/Danix1005/layout-switcher2/releases/download/v1.0.0/layout-switcher.exe"));
        assert!(!s.download_allowed("https://github.com/Danix10050/layout-switcher/releases/download/v1.0.0/layout-switcher.exe"));
        assert!(!s.download_allowed("https://github.com/Danix1005/layout-switcher/releases/download"));
        assert!(!s.download_allowed("https://GITHUB.com.evil.example/Danix1005/layout-switcher/releases/download/x"));
        assert!(!s.download_allowed("")); // коротша за префікс
    }

    #[test]
    fn http_is_refused_unless_explicitly_allowed() {
        let s = src();
        assert!(!s.scheme_ok("http://api.github.com/x"));
        let t = Source { allow_http: true, ..s };
        assert!(t.scheme_ok("http://127.0.0.1/x"));
    }
}
