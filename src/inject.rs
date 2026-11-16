//! Що робити з клавішами, які ввів не користувач фізично, а інша програма («інжектований» ввід).
//!
//! Власні події ми мітимо й завжди пропускаємо — це вирішується раніше. Тут ідеться про ЧУЖІ програмні події.
//! На звичайному комп'ютері їх краще не чіпати (автонабір паролів, макроси). Але там, де клавіатура
//! «приїжджає» по мережі, — на машині, якою керують через AnyDesk/TeamViewer, а можливо й у сеансі RDP, —
//! увесь ввід може бути програмним, і тоді перемикач, що його ігнорує, просто нічого не робить.
//!
//! Режим `Auto`: якщо від запуску бачимо ЛИШЕ програмні натискання і їх уже чимало — починаємо їх обробляти.

/// Скільки програмних натискань поспіль (без жодного фізичного) вмикає обробку в режимі `Auto`.
pub const AUTO_THRESHOLD: u64 = 20;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum InjectedMode {
    /// Ніколи не обробляти чужий програмний ввід.
    Ignore,
    /// Завжди обробляти як звичайний набір.
    Accept,
    /// Вирішити самому (див. вище).
    Auto,
}

impl InjectedMode {
    pub fn parse(s: &str) -> Option<InjectedMode> {
        match s {
            "ignore" => Some(InjectedMode::Ignore),
            "accept" => Some(InjectedMode::Accept),
            "auto" => Some(InjectedMode::Auto),
            _ => None,
        }
    }

    pub fn tag(self) -> &'static str {
        match self {
            InjectedMode::Ignore => "ignore",
            InjectedMode::Accept => "accept",
            InjectedMode::Auto => "auto",
        }
    }
}

/// Рішення щодо одного натискання.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Decision {
    /// Обробляти натискання як набір тексту.
    pub process: bool,
    /// Саме цим натисканням режим `Auto` увімкнув обробку програмного вводу (сказати користувачеві, записати в лог).
    pub just_enabled: bool,
}

pub struct InjectedPolicy {
    mode: InjectedMode,
    physical: u64,
    injected: u64,
    auto_on: bool,
}

impl InjectedPolicy {
    pub fn new(mode: InjectedMode) -> Self {
        InjectedPolicy { mode, physical: 0, injected: 0, auto_on: false }
    }

    pub fn mode(&self) -> InjectedMode {
        self.mode
    }

    /// Чи обробляється зараз чужий програмний ввід.
    pub fn accepting(&self) -> bool {
        match self.mode {
            InjectedMode::Accept => true,
            InjectedMode::Ignore => false,
            InjectedMode::Auto => self.auto_on,
        }
    }

    /// (фізичних натискань, програмних натискань) від запуску.
    pub fn counts(&self) -> (u64, u64) {
        (self.physical, self.injected)
    }

    /// Чергове натискання клавіші (не наше власне). `injected` — подію ввела інша програма.
    pub fn on_key(&mut self, injected: bool) -> Decision {
        if !injected {
            self.physical += 1;
            return Decision { process: true, just_enabled: false };
        }
        self.injected += 1;
        let mut just_enabled = false;
        if self.mode == InjectedMode::Auto && !self.auto_on && self.physical == 0 && self.injected >= AUTO_THRESHOLD {
            self.auto_on = true;
            just_enabled = true;
        }
        Decision { process: self.accepting(), just_enabled }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn physical_keys_are_always_processed() {
        for mode in [InjectedMode::Ignore, InjectedMode::Accept, InjectedMode::Auto] {
            let mut p = InjectedPolicy::new(mode);
            assert!(p.on_key(false).process);
        }
    }

    #[test]
    fn ignore_and_accept_modes_are_fixed() {
        let mut p = InjectedPolicy::new(InjectedMode::Ignore);
        for _ in 0..100 {
            assert!(!p.on_key(true).process);
        }
        let mut p = InjectedPolicy::new(InjectedMode::Accept);
        assert!(p.on_key(true).process);
    }

    #[test]
    fn auto_turns_on_when_all_input_so_far_is_injected() {
        let mut p = InjectedPolicy::new(InjectedMode::Auto);
        for i in 1..AUTO_THRESHOLD {
            let d = p.on_key(true);
            assert!(!d.process && !d.just_enabled, "натискання {i}");
        }
        let d = p.on_key(true);
        assert!(d.process && d.just_enabled);
        // далі обробляємо без повторних сповіщень, фізичні клавіші теж
        let d = p.on_key(true);
        assert!(d.process && !d.just_enabled);
        assert!(p.on_key(false).process);
        assert!(p.accepting());
    }

    #[test]
    fn auto_stays_off_on_a_computer_with_a_real_keyboard() {
        // звичайний ПК: є фізичні клавіші, а програмний ввід — це автонабір чи макроси
        let mut p = InjectedPolicy::new(InjectedMode::Auto);
        p.on_key(false);
        for _ in 0..1000 {
            let d = p.on_key(true);
            assert!(!d.process && !d.just_enabled);
        }
        assert!(!p.accepting());
        assert_eq!(p.counts(), (1, 1000));
    }

    #[test]
    fn parse_and_tag_roundtrip() {
        for m in [InjectedMode::Ignore, InjectedMode::Accept, InjectedMode::Auto] {
            assert_eq!(InjectedMode::parse(m.tag()), Some(m));
        }
        assert_eq!(InjectedMode::parse("yes"), None);
    }
}
