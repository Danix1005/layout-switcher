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
use layout_switcher::detect::{Dicts, Verdict};
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
}

#[derive(Clone)]
struct LastToken {
    keys: Vec<Key>,
    /// В якій розкладці слово виглядає на екрані (після автовиправлення — цільова).
    shown: Lang,
    /// Пробіл або Tab, який ми набрали після слова (Enter не запам'ятовуємо: у чаті повідомлення вже пішло).
    boundary: u32,
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
        let masked = is_sensitive(&exe);
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
pub(crate) fn on_key(vk: u32, is_down: bool, is_up: bool, injected: bool) -> bool {
    let job = with_state(|st| {
        st.last_key_at = Instant::now();

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
        let keys = st.keys.clone();
        let job = st.manual_token(src, "word", &keys, shown, None, hkl_now);
        st.shown = Some(job.to); // слово лишається в буфері; наступні клавіші друкуються вже в новій розкладці
        return Some(job);
    }

    // 2) щойно набране слово (після пробілу/Tab) — після автовиправлення це «скасувати»
    if let Some(last) = st.last.clone() {
        let job = st.manual_token(src, "last", &last.keys, last.shown, Some(last.boundary), hkl_now);
        if let Some(l) = st.last.as_mut() {
            l.shown = job.to; // повторне натискання перемкне назад
        }
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
    st.session.reset();
    st.fg_exe = win::exe_of(t.fg);
    st.mode = st.rules.mode(&st.fg_exe);
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
            });
        }
    }
}

unsafe fn step(st: &mut State, vk: u32) -> Option<Job> {
    let t = win::target();
    if t.fg as usize != st.fg {
        enter_window(st, t);
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
        return None;
    }

    match vk {
        PAUSE => manual(st, "pause"), // Pause/Break — те саме, що подвійний Shift (запасна клавіша)
        BACK => {
            if st.keys.is_empty() {
                st.last = None; // стерли щось перед курсором — «останнє слово» більше не під курсором
                st.log_undo("backspace");
            } else {
                st.keys.pop();
            }
            None
        }
        SPACE | RETURN | TAB => {
            let keys = std::mem::take(&mut st.keys);
            if keys.is_empty() {
                return None;
            }
            st.shown = None;
            let (lang, hkl_now) = win::layout_now();
            let Some(cur) = lang else {
                st.last = None;
                st.log_word(&keys, None, hkl_now, None);
                return None;
            };
            st.remember(cur, hkl_now);
            let now = st.now_ms();
            let out = st.session.on_word(&st.dicts, &keys, cur, now);
            st.log_word(&keys, Some(cur), hkl_now, Some(&out));
            let to = match &out.verdict {
                Verdict::Fix(f) => Some(f.to),
                _ => None,
            };
            // Запам'ятовуємо слово, щоб подвійний Shift міг виправити (або скасувати виправлення) вже після пробілу.
            st.last =
                if vk == RETURN { None } else { Some(LastToken { keys: keys.clone(), shown: to.unwrap_or(cur), boundary: vk }) };
            match out.verdict {
                Verdict::Fix(fix) => {
                    st.last_fix_at = Some(Instant::now());
                    let hkl = out.switch_to.map(|l| st.hkl(l));
                    Some(Job { back: keys.len(), text: fix.text, boundary: Some(vk), to: fix.to, hkl })
                }
                _ => None,
            }
        }
        v if is_tracked_key(v) => {
            if st.keys.is_empty() {
                st.last = None; // почалось нове слово
                st.shown = win::layout_now().0; // у якій розкладці воно набирається
            }
            let key = Key { vk: v, shift: down(VK_SHIFT), caps: st.caps_now() };
            st.keys.push(key);
            st.last_fix_at = None;
            None
        }
        _ => {
            // стрілки, Home/End, Esc, Delete, F-клавіші, numpad, VK_PACKET... — позиція змінилась
            st.keys.clear();
            st.last = None;
            st.last_fix_at = None;
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
