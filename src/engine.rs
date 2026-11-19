//! Рушій: буфер поточного слова, рішення «виправляти чи ні», ручне виправлення.
//!
//! Увесь стан живе в потоці, який встановив хуки (хуки викликаються саме в ньому), тому `thread_local`.
//! Правило для всього файлу: у потоці хука — нічого повільного. Рішення ухвалюємо під позикою стану,
//! а «натискаємо клавіші» вже після того, як позику відпущено.

use std::cell::RefCell;
use std::collections::HashSet;
use std::sync::atomic::{AtomicBool, Ordering};
use std::ptr::null_mut;
use std::time::{Duration, Instant};

use layout_switcher::apps::{is_sensitive, AppMode, AppRules, OffReason};
use layout_switcher::detect::{single_letter_fix, Dicts, Verdict};
use layout_switcher::keymap::is_key_run;
use layout_switcher::hotkey::DoubleShift;
use layout_switcher::inject::InjectedPolicy;
use layout_switcher::keymap::{convert, is_tracked_key, render, Key, Lang};
use layout_switcher::privacy::looks_secret;
use layout_switcher::session::{Embed, Outcome, Session};

use windows_sys::Win32::Foundation::HWND;
use windows_sys::Win32::UI::Input::KeyboardAndMouse::{
    GetKeyState, GetKeyboardLayout, SendInput, INPUT, KEYEVENTF_KEYUP, KEYEVENTF_UNICODE, VK_BACK, VK_CAPITAL,
    VK_CONTROL, VK_LWIN, VK_MENU, VK_PAUSE, VK_RETURN, VK_RWIN, VK_SHIFT, VK_SPACE, VK_TAB,
};
use windows_sys::Win32::UI::WindowsAndMessaging::{GetWindowThreadProcessId, KillTimer, SetTimer};

use crate::learned;
use crate::{appsoff, password};
use crate::logger::{log_enabled, log_event, say, with_log};
use crate::selection;
use crate::win::{self, down, hkl_hex, key_input, tap};

const SPACE: u32 = VK_SPACE as u32;
const RETURN: u32 = VK_RETURN as u32;
const TAB: u32 = VK_TAB as u32;
const BACK: u32 = VK_BACK as u32;
const PAUSE: u32 = VK_PAUSE as u32;
const CAPITAL: u32 = VK_CAPITAL as u32;
const SCROLL: u32 = 0x91;
const LSHIFT: u32 = 0xA0;
const VK_Z: u32 = 0x5A;
/// «Клавіша» тексту, який інша програма вводить символами Unicode: фізичної клавіші за нею нема.
const VK_PACKET: u32 = 0xE7;
/// Службовий маркер у буфері слова: «слово зіпсовано» (AltGr тощо) — не конвертувати.
const POISON: Key = Key::new(0x07, false);
/// Backspace/Ctrl+Z у цьому вікні після автовиправлення вважаємо «скасуванням» (сигнал хибного спрацювання).
const UNDO_WINDOW: Duration = Duration::from_secs(4);
/// Через скільки після виправлення перевіряємо, чи розкладка справді перемкнулась.
const LAYOUT_CHECK_MS: u32 = 300;

/// Автовиправлення увімкнено (значок у треї: «Призупинити» / «Відновити»). Пауза діє на все: і на автовиправлення,
/// і на подвійний Shift. Стан CapsLock при цьому відстежуємо й далі, інакше після паузи він «з'їхав» би.
static ENABLED: AtomicBool = AtomicBool::new(true);

pub(crate) fn enabled() -> bool {
    ENABLED.load(Ordering::Relaxed)
}

/// Увімкнути чи призупинити. Слово, яке набиралось, кидаємо: після паузи воно було б неповним.
pub(crate) fn set_enabled(on: bool) {
    ENABLED.store(on, Ordering::Relaxed);
    with_state(|st| {
        st.reset_word();
        st.dshift.on_other();
    });
    log_event("enabled", vec![("on", (on as u8).to_string())]);
}

pub(crate) struct State {
    dicts: Dicts,
    rules: AppRules,
    inject: InjectedPolicy,
    keys: Vec<Key>,
    fg: usize,
    fg_exe: String,
    mode: AppMode,
    /// Розкладки, в які перемикаємось: ті, що реально встановлені (і востаннє бачені) у користувача.
    hkl_en: isize,
    hkl_uk: isize,
    last_fix_at: Option<Instant>,
    /// Подвійний лівий Shift — ручне виправлення.
    dshift: DoubleShift,
    start: Instant,
    /// В якій розкладці зараз ВИГЛЯДАЄ слово, що набирається (від неї рахуємо «в яку перемкнути»).
    shown: Option<Lang>,
    /// Щойно завершене слово (до першої наступної клавіші/кліку): його теж можна виправити вручну.
    last: Option<LastToken>,
    /// Перебіг набору в цьому вікні: мова останніх слів, чи перемикати розкладку (див. session.rs).
    session: Session,
    /// CapsLock: стежимо самі (див. `caps_now`).
    caps: bool,
    caps_reported: bool,
    caps_key_down: bool,
    pending_check: Option<LayoutCheck>,
    /// Застосунки, про які вже сказали в консолі (щоб не повторюватись).
    told_off: HashSet<String>,
    told_layout: HashSet<String>,
    last_key_at: Instant,
    /// Слово, яке користувач переніс подвійним Shift: запам'ятаємо його (learned.rs), коли він піде далі
    /// (почне нове слово), а не одразу — раптом він натисне Shift ще раз і поверне як було.
    pending_learn: Option<PendingLearn>,
    /// Слово, що набирається, вже переносили подвійним Shift — у яку розкладку.
    manual_mid: Option<Lang>,
    /// Попереднє слово — одна літера, а за нею пробіл (`d` у «d базу»): якщо наступне слово виправимо, її теж.
    prev1: Option<Prev1>,
    /// Слово вже перенесли в цю розкладку посеред набору (ми або подвійний Shift). Поки вікно не перемкнуло
    /// розкладку, наступні літери друкуємо самі — інакше вийшло б «приdsn».
    carry: Option<Lang>,
    /// У цьому слові спрацювало виправлення посеред слова.
    early_word: bool,
    /// Номер змін списку «не працювати в застосунку» (appsoff.rs), за яким складено `rules`.
    apps_gen: u32,
    /// Коли востаннє виправили слово автоматично (посеред слова або пробілом).
    auto_fix_at: Option<Instant>,
    /// Скасування виправлення посеред слова було надто швидким, щоб бути свідомим: не вчимо з нього.
    quick_revert: bool,
    /// У цьому слові вже записали `password_skip` (щоб не писати на кожну літеру).
    pw_logged: bool,
    /// Скан-код попередньої натиснутої клавіші і скільки разів поспіль він зростав рівно на 1.
    sc_prev: u32,
    sc_run: u32,
    /// Скільки натискань поспіль скан-коди йшли «звичайно» (не лічильником).
    sc_normal: u32,
    /// У цьому вікні натискання приховано: хук бачить не справжні клавіші, а лічильник (`йцукен…`, `фіва…`) —
    /// так антивіруси «захищають» браузер від кейлогерів (ESET «Захист банківських операцій і платежів»).
    hidden: bool,
    /// Чи вже казали про це користувачеві (раз за запуск).
    hidden_told: bool,
}

/// Стільки зростань скан-коду рівно на 1 поспіль (тобто 7 клавіш підряд по порядку на клавіатурі) —
/// і ми вважаємо, що натискання в цьому вікні підмінено. Людина так не друкує.
const HIDDEN_RUN: u32 = 6;
/// Стільки «звичайних» натискань поспіль — і підміни вже нема.
const VISIBLE_AGAIN: u32 = 12;

/// Подвійний Shift раніше, ніж за стільки після автоматичного виправлення, — це не «скасувати»: людина
/// помітила не ту розкладку й потягнулась виправляти, а ми встигли першими. Такий Shift нічого не робить.
const RACE_MS: u128 = 700;
/// Скасування посеред слова, швидше за це від виправлення, не вчить «не чіпати» (могла бути та сама гонка,
/// лише трохи повільніша). Після пробілу досить, щоб від виправлення й від пробілу минуло більше `RACE_MS`.
const REVERT_LEARN_MS: u128 = 1200;

struct PendingLearn {
    keys: Vec<Key>,
    /// Розкладка, в якій слово тепер на екрані.
    lang: Lang,
    /// Це скасування автоматичного виправлення (вчимо «не чіпати»), а не ручне виправлення.
    revert: bool,
}

#[derive(Clone, Copy)]
struct Prev1 {
    key: Key,
    lang: Lang,
}

#[derive(Clone)]
struct LastToken {
    keys: Vec<Key>,
    /// В якій розкладці слово виглядає на екрані (після автовиправлення — цільова).
    shown: Lang,
    /// Пробіл або Tab, який ми набрали після слова (Enter не запам'ятовуємо: у чаті повідомлення вже пішло).
    boundary: u32,
    /// Слово виправив автомат: подвійний Shift на ньому — це скасування.
    auto_fixed: bool,
    /// Коли слово закінчилось (пробіл).
    at: Instant,
}

/// Перевірка після виправлення: чи вікно з фокусом справді перейшло на потрібну розкладку.
struct LayoutCheck {
    focus: usize,
    want: Lang,
    exe: String,
}

/// Що треба зробити після того, як відпущено позику на стан.
struct Job {
    back: usize,
    text: String,
    boundary: Option<u32>,
    to: Lang,
    /// Розкладка, на яку перемкнутись. `None` — вкраплене слово: текст виправляємо, розкладку лишаємо.
    hkl: Option<isize>,
}

thread_local! {
    static ST: RefCell<Option<State>> = const { RefCell::new(None) };
}

fn with_state<R>(f: impl FnOnce(&mut State) -> R) -> Option<R> {
    ST.with(|cell| {
        let mut guard = cell.try_borrow_mut().ok()?;
        guard.as_mut().map(f)
    })
}

impl State {
    pub(crate) fn new(
        dicts: Dicts,
        rules: AppRules,
        inject: InjectedPolicy,
        embed: Embed,
        hkl_en: isize,
        hkl_uk: isize,
    ) -> State {
        let caps = unsafe { (GetKeyState(VK_CAPITAL as i32) & 1) != 0 };
        State {
            dicts,
            rules,
            inject,
            keys: Vec::new(),
            fg: 0,
            fg_exe: String::new(),
            mode: AppMode::Normal,
            hkl_en,
            hkl_uk,
            last_fix_at: None,
            dshift: DoubleShift::default(),
            start: Instant::now(),
            shown: None,
            last: None,
            session: Session::with_mode(embed),
            caps,
            caps_reported: caps,
            caps_key_down: false,
            pending_check: None,
            told_off: HashSet::new(),
            told_layout: HashSet::new(),
            last_key_at: Instant::now(),
            pending_learn: None,
            manual_mid: None,
            prev1: None,
            carry: None,
            early_word: false,
            apps_gen: u32::MAX,
            auto_fix_at: None,
            quick_revert: false,
            pw_logged: false,
            sc_prev: 0,
            sc_run: 0,
            sc_normal: 0,
            hidden: false,
            hidden_told: false,
        }
    }

    fn now_ms(&self) -> u64 {
        self.start.elapsed().as_millis() as u64
    }

    fn hkl(&self, lang: Lang) -> isize {
        match lang {
            Lang::En => self.hkl_en,
            Lang::Uk => self.hkl_uk,
        }
    }

    /// Запам'ятати точний HKL, яким користувач насправді друкує (напр. «Українська (розширена)»).
    fn remember(&mut self, lang: Lang, hkl: isize) {
        match lang {
            Lang::En => self.hkl_en = hkl,
            Lang::Uk => self.hkl_uk = hkl,
        }
    }

    /// Забути слово, що набирається, і «останнє слово» (курсор перемістився або змінилось вікно).
    fn reset_word(&mut self) {
        self.keys.clear();
        self.last = None;
        self.shown = None;
        self.last_fix_at = None;
        self.pending_learn = None;
        self.manual_mid = None;
        self.prev1 = None;
        self.carry = None;
        self.early_word = false;
        self.quick_revert = false;
        self.pw_logged = false;
    }

    /// Стежимо, чи не підмінено натискання: справжні клавіші хук бачить як лічильник скан-кодів
    /// (0x15, 0x16, 0x17… — `yuiop[]` замість набраного `welcome`), а застосунок отримує правильні літери.
    fn watch_scancodes(&mut self, sc: u32) {
        if sc == self.sc_prev + 1 {
            self.sc_run += 1;
            self.sc_normal = 0;
        } else {
            self.sc_run = 0;
            self.sc_normal += 1;
        }
        self.sc_prev = sc;
        if !self.hidden && self.sc_run >= HIDDEN_RUN {
            self.hidden = true;
            self.keys.clear();
            self.last = None;
            log_event("input_hidden", vec![("exe", self.fg_exe.clone()), ("state", "on".into())]);
            if !std::mem::replace(&mut self.hidden_told, true) {
                let exe = self.fg_exe.clone();
                std::thread::spawn(move || {
                    crate::tray::notify(
                        &format!("{exe}: натискання приховано"),
                        "Схоже, антивірус захищає цей браузер від перехоплення клавіатури (ESET: «Захист банківських \
                         операцій і платежів»). Автовиправлення тут не працює; подвійний Shift виправляє слово перед \
                         курсором через виділення.",
                    )
                });
            }
        } else if self.hidden && self.sc_normal >= VISIBLE_AGAIN {
            self.hidden = false;
            log_event("input_hidden", vec![("exe", self.fg_exe.clone()), ("state", "off".into())]);
        }
    }

    /// Скільки мс минуло від останнього автоматичного виправлення.
    fn since_auto_fix(&self) -> u128 {
        self.auto_fix_at.map_or(u128::MAX, |t| t.elapsed().as_millis())
    }

    /// Виправлення посеред слова (`Dicts::early_fix`): викликається після кожної літери, з третьої.
    unsafe fn try_early(&mut self) -> Option<Job> {
        if self.keys.len() < 3 || self.manual_mid.is_some() || is_sensitive(&self.fg_exe) {
            return None;
        }
        let (lang, _) = win::layout_now();
        let cur = self.shown.or(lang)?;
        let fix = self.dicts.early_fix(&self.keys, cur, self.session.ctx())?;
        if !password::safe_to_fix(self.fg as HWND) {
            if !std::mem::replace(&mut self.pw_logged, true) {
                let why = password::why(self.fg as HWND);
                log_event("password_skip", vec![("exe", self.fg_exe.clone()), ("at", "early".into()), ("why", why.into())]);
            }
            return None;
        }
        let mut back = self.keys.len() - 1; // останню літеру ми «з'їли» — її на екрані ще нема
        let mut text = fix.text.clone();
        if let Some(letter) = self.prev1.take().filter(|p| p.lang == cur).and_then(|p| single_letter_fix(p.key, cur)) {
            back += 2;
            text = format!("{letter} {text}");
            log_event("prev_letter", vec![("exe", self.fg_exe.clone()), ("to", format!("{:?}", fix.to))]);
        }
        if log_enabled() {
            let exe = self.fg_exe.clone();
            let ctx = self.session.ctx().map(|l| format!("{l:?}")).unwrap_or_else(|| "-".into());
            let typed = render(&self.keys, cur);
            let n = self.keys.len();
            with_log(|lg| {
                let mut f: Vec<(&str, String)> = vec![
                    ("exe", exe),
                    ("ctx", ctx),
                    ("cur", format!("{cur:?}")),
                    ("to", format!("{:?}", fix.to)),
                    ("len", n.to_string()),
                ];
                if lg.words && !looks_secret(&typed) {
                    f.push(("typed", typed));
                    f.push(("other", fix.text.clone()));
                }
                lg.line("early", &f);
            });
        }
        self.carry = Some(fix.to);
        self.shown = Some(fix.to);
        self.early_word = true;
        self.auto_fix_at = Some(Instant::now());
        self.session.expect(fix.to); // розкладку перемкнули ми, а не людина: це не «sw=1»
        Some(Job { back, text, boundary: None, to: fix.to, hkl: Some(self.hkl(fix.to)) })
    }

    /// Запам'ятати слово в особистому словнику, якщо воно того варте (вирішує `Dicts::learnable*`).
    fn learn(&mut self, keys: &[Key], lang: Lang, how: &'static str) {
        if is_sensitive(&self.fg_exe) || password::is_password(self.fg as HWND) {
            return; // термінали, менеджери паролів, VPN, поля паролів: звідти нічого не вчимо
        }
        let word = if how == "revert" { self.dicts.learnable_revert(keys, lang) } else { self.dicts.learnable(keys, lang) };
        let Some(word) = word else { return };
        self.dicts.add_word(lang, &word);
        if log_enabled() {
            let exe = self.fg_exe.clone();
            with_log(|lg| {
                let mut f: Vec<(&str, String)> =
                    vec![("how", how.to_string()), ("lang", format!("{lang:?}")), ("exe", exe)];
                if lg.words {
                    f.push(("word", word.clone()));
                }
                lg.line("learned", &f);
            });
        }
        learned::save(lang, word);
    }

    /// Користувач пішов далі після ручного перенесення слова — тепер його можна запам'ятати.
    fn commit_learn(&mut self) {
        if let Some(p) = self.pending_learn.take() {
            self.learn(&p.keys, p.lang, if p.revert { "revert" } else { "manual" });
        }
    }

    /// Стан CapsLock.
    ///
    /// `GetKeyState` показує стан клавіатури таким, яким його бачить черга повідомлень НАШОГО потоку, а він
    /// клавіатурних повідомлень не отримує — тож значення може застаріти. Тому рахуємо натискання CapsLock
    /// самі, а `GetKeyState` довіряємо лише тоді, коли його відповідь ЗМІНИЛАСЬ (отже, він живий).
    unsafe fn caps_now(&mut self) -> bool {
        let reported = (GetKeyState(VK_CAPITAL as i32) & 1) != 0;
        if reported != self.caps_reported {
            self.caps_reported = reported;
            self.caps = reported;
        }
        self.caps
    }

    /// Рядок `word` у лог. `out` — рішення сесії (нема, якщо розкладка не англійська й не українська).
    fn log_word(&mut self, keys: &[Key], cur: Option<Lang>, hkl: isize, out: Option<&Outcome>) {
        if !log_enabled() {
            return;
        }
        let exe = self.fg_exe.clone();
        let masked = is_sensitive(&exe) || !password::known_not_password(self.fg as HWND);
        let tag = out.map_or("no_lang", |o| o.verdict.tag());
        let to = out.and_then(|o| match &o.verdict {
            Verdict::Fix(f) => Some(f.to),
            _ => None,
        });
        let switched = out.map_or(false, |o| o.switched_by_user);
        let embedded = out.map_or(false, |o| o.embedded);
        let ctx = out.and_then(|o| o.ctx).map(|l| format!("{l:?}")).unwrap_or_else(|| "-".into());
        with_log(|lg| {
            let mut f: Vec<(&str, String)> = vec![
                ("exe", exe),
                ("ctx", ctx),
                ("cur", cur.map(|l| format!("{l:?}")).unwrap_or_else(|| "other".into())),
                ("hkl", hkl_hex(hkl)),
                ("len", keys.len().to_string()),
                ("verdict", tag.to_string()),
            ];
            if let Some(t) = to {
                f.push(("to", format!("{t:?}")));
            }
            if switched {
                f.push(("sw", "1".into())); // користувач сам перемкнув розкладку перед цим словом
            }
            if embedded {
                f.push(("emb", "1".into())); // вкраплене слово: текст виправлено, розкладку не чіпали
            }
            if lg.words && masked {
                f.push(("masked", "1".into())); // процес, де вводять облікові дані: слів у лог не пишемо
            } else if lg.words {
                let (a, b) = match cur {
                    Some(c) => (("typed", render(keys, c)), ("other", convert(keys, c))),
                    None => (("as_en", render(keys, Lang::En)), ("as_uk", render(keys, Lang::Uk))),
                };
                if looks_secret(&a.1) || looks_secret(&b.1) {
                    f.push(("masked", "secret".into())); // схоже на пароль чи ключ (див. privacy.rs)
                } else {
                    f.push(a);
                    f.push(b);
                }
            }
            lg.line("word", &f);
        });
    }

    fn log_undo(&mut self, via: &str) {
        if let Some(t) = self.last_fix_at.take() {
            if t.elapsed() < UNDO_WINDOW {
                let ms = t.elapsed().as_millis().to_string();
                log_event("undo", vec![("via", via.to_string()), ("ms", ms)]);
            }
        }
    }

    fn log_noop(&self, src: &str, reason: &str) {
        log_event(
            "manual_noop",
            vec![("src", src.to_string()), ("exe", self.fg_exe.clone()), ("reason", reason.to_string())],
        );
    }

    /// Ручне перемикання слова: у лог іде і «що було», і «що вирішив би автомат» (`auto`) —
    /// якщо автомат мовчав, видно чому (unknown = прогалина словника, known = хибне рішення).
    fn manual_token(
        &mut self,
        src: &'static str,
        kind: &'static str,
        keys: &[Key],
        shown: Lang,
        boundary: Option<u32>,
        hkl_now: isize,
    ) -> Job {
        let to = shown.other();
        let ctx = self.session.ctx();
        let auto = self.dicts.verdict(keys, shown, ctx).tag();
        let text = convert(keys, shown);
        if log_enabled() {
            let exe = self.fg_exe.clone();
            let masked = is_sensitive(&exe);
            let ctx_s = ctx.map(|l| format!("{l:?}")).unwrap_or_else(|| "-".into());
            with_log(|lg| {
                let mut f: Vec<(&str, String)> = vec![
                    ("src", src.to_string()),
                    ("kind", kind.to_string()),
                    ("exe", exe),
                    ("ctx", ctx_s),
                    ("shown", format!("{shown:?}")),
                    ("to", format!("{to:?}")),
                    ("hkl", hkl_hex(hkl_now)),
                    ("len", keys.len().to_string()),
                    ("auto", auto.to_string()),
                ];
                if lg.words && masked {
                    f.push(("masked", "1".into()));
                } else if lg.words {
                    let typed = render(keys, shown);
                    if looks_secret(&typed) || looks_secret(&text) {
                        f.push(("masked", "secret".into()));
                    } else {
                        f.push(("typed", typed));
                        f.push(("other", text.clone()));
                    }
                }
                lg.line("manual", &f);
            });
        }
        self.session.on_manual(to); // ручний вибір — найпевніший сигнал, якою мовою пише користувач
        Job { back: keys.len() + boundary.is_some() as usize, text, boundary, to, hkl: Some(self.hkl(to)) }
    }
}

// ───────────────────────────── вхід із хуків ─────────────────────────────

pub(crate) fn init(state: State) {
    ST.with(|c| *c.borrow_mut() = Some(state));
}

/// Подія клавіатури (не наша власна). Повертає true, якщо клавішу треба «з'їсти»:
/// ми самі надрукуємо замість неї що треба.
/// Shift, Ctrl, Alt, Win: антивірус їх не підмінює, і в «лічильник» вони не входять.
fn is_modifier(vk: u32) -> bool {
    matches!(vk, 0x10 | 0x11 | 0x12 | 0xA0..=0xA5 | 0x5B | 0x5C | 0x14)
}

pub(crate) fn on_key(vk: u32, sc: u32, is_down: bool, is_up: bool, injected: bool) -> bool {
    let job = with_state(|st| {
        st.last_key_at = Instant::now();
        if is_down && !injected && !is_modifier(vk) {
            st.watch_scancodes(sc);
        }

        // Чужий програмний ввід: обробляти чи ні — вирішує політика (див. inject.rs).
        if is_down && vk != VK_PACKET {
            let d = st.inject.on_key(injected);
            if d.just_enabled {
                let (phys, inj) = st.inject.counts();
                log_event(
                    "injected",
                    vec![("mode", "auto_on".into()), ("physical", phys.to_string()), ("injected", inj.to_string())],
                );
                say("Бачу лише програмний ввід (віддалене керування?) — відтепер обробляю його як звичайний набір.");
            }
            if !d.process {
                return None;
            }
        } else if injected && !st.inject.accepting() {
            return None;
        }

        if vk == CAPITAL {
            // рахуємо перемикання CapsLock самі; автоповтор при утриманні не рахуємо
            if is_down && !st.caps_key_down {
                st.caps = !st.caps;
            }
            st.caps_key_down = is_down;
        }

        if !enabled() {
            return None;
        }

        if is_down {
            unsafe { step(st, vk) }
        } else if is_up && vk == LSHIFT && st.mode == AppMode::Normal {
            let now = st.now_ms();
            if st.dshift.on_shift_up(now) {
                unsafe { manual(st, "dshift") }
            } else {
                None
            }
        } else {
            None
        }
    })
    .flatten();

    match job {
        Some(j) => {
            if let Some(focus) = unsafe { run(&j) } {
                with_state(|st| {
                    st.pending_check = Some(LayoutCheck { focus: focus as usize, want: j.to, exe: st.fg_exe.clone() });
                });
                unsafe { SetTimer(null_mut(), 0, LAYOUT_CHECK_MS, Some(check_layout)) };
            }
            is_down
        }
        None => false,
    }
}

/// Клік мишкою: курсор міг переміститись — слово «розірване».
pub(crate) fn on_click() {
    if !enabled() {
        return;
    }
    with_state(|st| {
        st.reset_word();
        st.dshift.on_other(); // Shift+клік — це виділення, а не подвійне натискання
    });
}

/// Скільки часу не було клавіатурних подій (для безпечного перевстановлення хуків).
pub(crate) fn idle() -> Duration {
    with_state(|st| st.last_key_at.elapsed()).unwrap_or(Duration::ZERO)
}

/// Періодичний рядок у лог: скільки натискань було фізичних, скільки програмних.
pub(crate) fn log_stats(rehooks: u64) {
    if !log_enabled() {
        return;
    }
    if let Some((phys, inj, accepting)) = with_state(|st| {
        let (p, i) = st.inject.counts();
        (p, i, st.inject.accepting())
    }) {
        log_event(
            "stats",
            vec![
                ("physical", phys.to_string()),
                ("injected", inj.to_string()),
                ("accepting", (accepting as u8).to_string()),
                ("rehooks", rehooks.to_string()),
            ],
        );
    }
}

/// Таймер після виправлення: чи розкладка справді перемкнулась? Результат — у лог (подія `layout`).
unsafe extern "system" fn check_layout(_hwnd: HWND, _msg: u32, id: usize, _time: u32) {
    KillTimer(null_mut(), id);
    let _ = std::panic::catch_unwind(|| {
        with_state(|st| {
            let Some(chk) = st.pending_check.take() else { return };
            let tid = unsafe { GetWindowThreadProcessId(chk.focus as HWND, null_mut()) };
            if tid == 0 {
                return; // вікна вже нема
            }
            let hkl = unsafe { GetKeyboardLayout(tid) } as isize;
            let got = match (hkl as usize) & 0x3FF {
                0x22 => Some(Lang::Uk),
                0x09 => Some(Lang::En),
                _ => None,
            };
            let ok = got == Some(chk.want);
            log_event(
                "layout",
                vec![
                    ("exe", chk.exe.clone()),
                    ("want", format!("{:?}", chk.want)),
                    ("got", got.map(|l| format!("{l:?}")).unwrap_or_else(|| "other".into())),
                    ("hkl", hkl_hex(hkl)),
                    ("ok", (ok as u8).to_string()),
                ],
            );
            if !ok && st.told_layout.insert(chk.exe.clone()) {
                say(format!(
                    "{}: текст виправлено, але розкладка не перемкнулась (запиши лог і надішли — подія layout).",
                    chk.exe
                ));
            }
        });
    });
}

// ───────────────────────────── рішення ─────────────────────────────

/// Ручне виправлення: слово, що набирається → щойно набране слово → виділений фрагмент.
unsafe fn manual(st: &mut State, src: &'static str) -> Option<Job> {
    let (lang_now, hkl_now) = win::layout_now();
    if let Some(l) = lang_now {
        st.remember(l, hkl_now);
    }

    // Натискання підмінено або буфер схожий на лічильник (`yuiop[]` замість `welcome`): перетворювати те, що ми
    // «бачили», не можна — це дало б нісенітницю. Беремо справжній текст з екрана: слово перед курсором.
    if st.hidden || is_key_run(&st.keys, 4) || st.last.as_ref().is_some_and(|l| is_key_run(&l.keys, 4)) {
        st.keys.clear();
        st.last = None;
    }

    // 1) слово, яке зараз набирається (ще без пробілу)
    if !st.keys.is_empty() {
        if !st.keys.iter().all(|k| is_tracked_key(k.vk)) {
            st.log_noop(src, "poisoned_token");
            return None;
        }
        let Some(shown) = st.shown.or(lang_now) else {
            st.log_noop(src, "no_lang");
            return None;
        };
        // Ми щойно виправили це слово посеред набору, а людина саме тиснула Shift, щоб виправити його сама.
        let since = st.since_auto_fix();
        if st.early_word && st.manual_mid.is_none() && since < RACE_MS {
            st.log_noop(src, "just_fixed");
            return None;
        }
        if st.early_word && st.manual_mid.is_none() {
            st.quick_revert = since < REVERT_LEARN_MS;
        }
        let keys = st.keys.clone();
        let job = st.manual_token(src, "word", &keys, shown, None, hkl_now);
        st.shown = Some(job.to); // слово лишається в буфері; наступні клавіші друкуються вже в новій розкладці
        st.carry = Some(job.to);
        // Повторний Shift посеред того самого слова повертає як було — тоді вчити нічого.
        st.manual_mid = if st.manual_mid.is_some() { None } else { Some(job.to) };
        st.prev1 = None;
        return Some(job);
    }

    // 2) щойно набране слово (після пробілу/Tab) — після автовиправлення це «скасувати»
    if let Some(last) = st.last.clone() {
        let since = st.since_auto_fix();
        if last.auto_fixed && since < RACE_MS {
            st.log_noop(src, "just_fixed"); // те саме, що й вище, але виправив пробіл
            return None;
        }
        let job = st.manual_token(src, "last", &last.keys, last.shown, Some(last.boundary), hkl_now);
        if let Some(l) = st.last.as_mut() {
            l.shown = job.to; // повторне натискання перемкне назад
            l.auto_fixed = false;
        }
        // Повторне натискання на тому самому слові повертає як було: користувач передумав — нічого не вчимо.
        st.pending_learn = match st.pending_learn.take() {
            Some(_) => None,
            // Швидке «скасування» не вчимо: це могла бути та сама гонка з автоматом.
            None if last.auto_fixed && since.min(last.at.elapsed().as_millis()) < RACE_MS => None,
            None => Some(PendingLearn { keys: last.keys.clone(), lang: job.to, revert: last.auto_fixed }),
        };
        st.prev1 = None;
        return Some(job);
    }

    // 3) виділений фрагмент — у фоновому потоці (чекає на застосунок, хук блокувати не можна).
    //    Не в терміналах і не там, де вводять облікові дані: вставка в терміналі — це виконання команди.
    if is_sensitive(&st.fg_exe) {
        st.log_noop(src, "unsafe_app");
        return None;
    }
    selection::spawn(selection::SelReq {
        src,
        exe: st.fg_exe.clone(),
        hint: lang_now,
        hkl_en: st.hkl_en,
        hkl_uk: st.hkl_uk,
    });
    None
}

/// Нове переднє вікно: з'ясувати застосунок і режим роботи в ньому.
unsafe fn enter_window(st: &mut State, t: win::Target) {
    st.fg = t.fg as usize;
    st.reset_word();
    st.hidden = false; // у новому вікні перевіряємо заново
    st.sc_run = 0;
    st.session.reset();
    st.fg_exe = win::exe_of(t.fg);
    appsoff::set_current(&st.fg_exe);
    st.mode = st.rules.mode(&st.fg_exe);
    password::ask(t.fg, true);
    if log_enabled() {
        let mut f = vec![("exe", st.fg_exe.clone())];
        if t.focus != t.fg {
            let focus_exe = win::exe_of(t.focus);
            if focus_exe != st.fg_exe {
                f.push(("focus_exe", focus_exe)); // поле вводу належить іншому процесу (WebView2, UWP)
            }
        }
        if let AppMode::Off(reason) = st.mode {
            f.push(("mode", format!("off:{}", reason.tag())));
        }
        log_event("fg", f);
    }
    if let AppMode::Off(reason) = st.mode {
        if st.told_off.insert(st.fg_exe.clone()) {
            say(match reason {
                OffReason::RemoteClient => format!(
                    "{}: клієнт віддаленого доступу — тут не втручаюсь. Щоб виправляти набір на віддаленій машині, запусти layout-switcher там.",
                    st.fg_exe
                ),
                OffReason::UserList => format!("{}: вимкнено в apps-off.txt — тут не втручаюсь.", st.fg_exe),
                OffReason::Menu => format!("{}: вимкнено в меню значка — тут не втручаюсь.", st.fg_exe),
            });
        }
    }
}

unsafe fn step(st: &mut State, vk: u32) -> Option<Job> {
    let t = win::target();
    // Список «не працювати в застосунку» змінили з меню — перечитати й одразу застосувати до поточного вікна.
    let gen = appsoff::generation();
    if gen != st.apps_gen {
        st.apps_gen = gen;
        st.rules.set_menu_list(&appsoff::read());
        if t.fg as usize == st.fg {
            st.fg = 0; // примусово «увійти» у вікно заново: режим, лог, підказка
        }
    }
    if t.fg as usize != st.fg {
        enter_window(st, t);
    }
    // Слова з виправлення виділеного (фоновий потік) — перевірити словниками й, можливо, запам'ятати.
    for (keys, lang) in learned::take_offers() {
        st.learn(&keys, lang, "selection");
    }
    if st.mode != AppMode::Normal {
        st.dshift.on_other();
        return None;
    }

    // Лівий Shift: стежимо за «чистими» натисканнями (подвійне натискання = ручне виправлення).
    if vk == LSHIFT {
        let now = st.now_ms();
        st.dshift.on_shift_down(now);
        return None;
    }
    st.dshift.on_other(); // будь-яка інша клавіша ламає «чисте» натискання

    // Натискання підмінено (див. watch_scancodes): ми не знаємо, що насправді набрано — нічого не відстежуємо.
    if st.hidden {
        st.keys.clear();
        st.last = None;
        st.carry = None;
        return None;
    }

    // Самі модифікатори і CapsLock слово не рвуть.
    if matches!(vk, 0x10 | 0x11 | 0x12 | 0x14 | 0x5B | 0x5C | 0xA1..=0xA5) {
        return None;
    }

    // Scroll Lock — пауза/відновлення логу (запобіжник для --log-words).
    if vk == SCROLL {
        let msg = with_log(|lg| {
            if lg.paused {
                lg.paused = false;
                lg.line("resume", &[]);
                "лог: відновлено"
            } else {
                lg.line("pause", &[]);
                lg.paused = true;
                "лог: ПАУЗА"
            }
        });
        if let Some(m) = msg {
            say(m);
        }
        return None;
    }

    let ctrl = down(VK_CONTROL);
    let alt = down(VK_MENU);
    let win_key = down(VK_LWIN) || down(VK_RWIN);

    // AltGr (= Ctrl+Alt): наприклад ґ на AltGr+Г. Таке слово не чіпаємо до кінця.
    if ctrl && alt && !win_key {
        st.last = None;
        st.keys.push(POISON);
        return None;
    }
    // Ctrl/Alt/Win-комбінації — це не набір тексту.
    if ctrl || alt || win_key {
        if ctrl && !alt && vk == VK_Z {
            st.log_undo("ctrl+z");
        }
        st.keys.clear();
        st.last = None;
        st.last_fix_at = None;
        st.pending_learn = None;
        st.manual_mid = None;
        st.prev1 = None;
        return None;
    }

    match vk {
        PAUSE => manual(st, "pause"), // Pause/Break — те саме, що подвійний Shift (запасна клавіша)
        BACK => {
            st.pending_learn = None; // стирає щойно перенесене — значить, не те
            if st.keys.is_empty() {
                st.last = None; // стерли щось перед курсором — «останнє слово» більше не під курсором
                st.prev1 = None;
                st.log_undo("backspace");
            } else {
                st.keys.pop();
            }
            None
        }
        SPACE | RETURN | TAB => {
            st.commit_learn();
            let keys = std::mem::take(&mut st.keys);
            let mid = st.manual_mid.take();
            let prev = st.prev1.take();
            let carried = st.carry.take();
            let early = std::mem::take(&mut st.early_word);
            let quick_revert = std::mem::take(&mut st.quick_revert);
            if keys.is_empty() {
                return None;
            }
            st.shown = None;
            let (lang, hkl_now) = win::layout_now();
            // Якщо слово переносили посеред набору, на екрані воно в тій розкладці, навіть коли вікно її не ввімкнуло.
            let Some(cur) = carried.or(lang) else {
                st.last = None;
                st.log_word(&keys, None, hkl_now, None);
                return None;
            };
            if lang == Some(cur) {
                st.remember(cur, hkl_now);
            }
            let now = st.now_ms();
            let out = st.session.on_word(&st.dicts, &keys, cur, now);
            st.log_word(&keys, Some(cur), hkl_now, Some(&out));
            let mut to = match &out.verdict {
                Verdict::Fix(f) => Some(f.to),
                _ => None,
            };
            // Розкладку цього слова користувач обрав сам (подвійний Shift посеред слова) — не сперечаємось.
            if to.is_some() && mid == Some(cur) {
                to = None;
                log_event("kept_manual", vec![("exe", st.fg_exe.clone())]);
            }
            // Поле пароля: нічого не виправляємо.
            if to.is_some() && !password::safe_to_fix(st.fg as HWND) {
                to = None;
                log_event("password_skip", vec![("exe", st.fg_exe.clone()), ("at", "word".into()), ("why", password::why(st.fg as HWND).into())]);
            }
            // Запам'ятовуємо слово, щоб подвійний Shift міг виправити (або скасувати виправлення) вже після пробілу.
            st.last = if vk == RETURN {
                None
            } else {
                Some(LastToken {
                    keys: keys.clone(),
                    shown: to.unwrap_or(cur),
                    boundary: vk,
                    auto_fixed: to.is_some() || (early && mid.is_none()),
                    at: Instant::now(),
                })
            };
            // Слово, яке переносили вручну посеред набору, дописано: запам'ятаємо, коли користувач піде далі.
            // Якщо до того його переносив автомат — це скасування: вчимо «не чіпати».
            if mid == Some(cur) && to.is_none() && !(early && quick_revert) {
                st.pending_learn = Some(PendingLearn { keys: keys.clone(), lang: cur, revert: early });
            }
            // Одна літера й пробіл: може знадобитись, якщо наступне слово виправлятимемо («d базу» -> «в базу»).
            // Не тоді, коли перед нею розкладку перемкнули самі: це свідомо набрана літера («диск D»).
            if keys.len() == 1 && vk == SPACE && matches!(out.verdict, Verdict::Short) && !out.switched_by_user {
                st.prev1 = Some(Prev1 { key: keys[0], lang: cur });
            }
            match out.verdict {
                Verdict::Fix(fix) if to.is_some() => {
                    st.last_fix_at = Some(Instant::now());
                    st.auto_fix_at = Some(Instant::now());
                    let hkl = out.switch_to.map(|l| st.hkl(l));
                    let (mut back, mut text) = (keys.len(), fix.text);
                    if let Some(letter) = prev.filter(|p| p.lang == cur).and_then(|p| single_letter_fix(p.key, cur)) {
                        back += 2; // літера й пробіл після неї
                        text = format!("{letter} {text}");
                        st.last = None; // «останнє слово» тепер із двох — повторний Shift не вгадає, що повертати
                        log_event("prev_letter", vec![("exe", st.fg_exe.clone()), ("to", format!("{:?}", fix.to))]);
                    }
                    Some(Job { back, text, boundary: Some(vk), to: fix.to, hkl })
                }
                _ => None,
            }
        }
        v if is_tracked_key(v) => {
            if st.keys.is_empty() {
                st.commit_learn(); // попереднє перенесене слово так і лишилось — запам'ятовуємо
                st.last = None; // почалось нове слово
                st.shown = win::layout_now().0; // у якій розкладці воно набирається
                st.carry = None;
                st.early_word = false;
                st.pw_logged = false;
                password::ask(t.fg, false); // поки набирається слово, дізнаємось, чи це не поле пароля
            }
            let key = Key { vk: v, shift: down(VK_SHIFT), caps: st.caps_now() };
            st.keys.push(key);
            st.last_fix_at = None;
            // Розкладку посеред слова вже перемкнули, а вікно ще не встигло (або не вміє): літеру друкуємо самі.
            if let Some(to) = st.carry {
                if win::layout_now().0 != Some(to) {
                    return Some(Job { back: 0, text: render(&[key], to), boundary: None, to, hkl: None });
                }
                return None;
            }
            st.try_early()
        }
        _ => {
            // стрілки, Home/End, Esc, Delete, F-клавіші, numpad, VK_PACKET... — позиція змінилась
            st.keys.clear();
            st.last = None;
            st.last_fix_at = None;
            st.pending_learn = None;
            st.manual_mid = None;
            st.prev1 = None;
            None
        }
    }
}

// ───────────────────────────── виведення ─────────────────────────────

/// Стерти набране, надрукувати правильне і, якщо треба, попросити вікно перемкнути розкладку.
/// Повертає вікно з фокусом, якому надіслано запит (щоб згодом перевірити результат).
unsafe fn run(job: &Job) -> Option<HWND> {
    let mut input: Vec<INPUT> = Vec::with_capacity((job.back + job.text.len() + 2) * 2);
    for _ in 0..job.back {
        tap(&mut input, VK_BACK);
    }
    for ch in job.text.chars() {
        let mut buf = [0u16; 2];
        for &u in ch.encode_utf16(&mut buf).iter() {
            input.push(key_input(0, u, KEYEVENTF_UNICODE));
            input.push(key_input(0, u, KEYEVENTF_UNICODE | KEYEVENTF_KEYUP));
        }
    }
    if let Some(vk) = job.boundary {
        tap(&mut input, vk as u16);
    }
    SendInput(input.len() as u32, input.as_ptr(), std::mem::size_of::<INPUT>() as i32);

    match job.hkl {
        Some(hkl) => {
            let focus = win::request_layout(hkl);
            say(format!("-> {:?}: виправлено {} симв.", job.to, job.back));
            Some(focus)
        }
        None => {
            say(format!("-> {:?}: виправлено {} симв. (вкраплене слово, розкладку не перемикаю)", job.to, job.back));
            None
        }
    }
}
