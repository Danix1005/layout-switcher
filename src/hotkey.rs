//! Подвійне натискання лівого Shift: чистий автомат без залежності від Windows (тестується на будь-якій ОС).
//!
//! «Натискання» = Shift натиснули й відпустили БЕЗ жодної іншої клавіші чи кліку між ними
//! (тому Shift+літера для великої літери, Shift+клік тощо не рахуються). «Подвійне» = два
//! таких натискання підряд, з паузою між відпусканнями не більше `DOUBLE_MAX_MS`.

/// Найдовше утримання Shift, яке ще вважається «натисканням» (довше — це вже утримання).
pub const TAP_MAX_MS: u64 = 350;
/// Найбільший інтервал між відпусканнями двох натискань.
pub const DOUBLE_MAX_MS: u64 = 500;

#[derive(Default)]
pub struct DoubleShift {
    down: bool,
    clean: bool,
    down_at: u64,
    tap_at: Option<u64>,
}

impl DoubleShift {
    /// Лівий Shift натиснули (автоповтор при утриманні ігноруємо).
    pub fn on_shift_down(&mut self, now_ms: u64) {
        if !self.down {
            self.down = true;
            self.clean = true;
            self.down_at = now_ms;
        }
    }

    /// Лівий Shift відпустили. `true` — щойно завершилось подвійне натискання.
    pub fn on_shift_up(&mut self, now_ms: u64) -> bool {
        let was_down = std::mem::replace(&mut self.down, false);
        let is_tap = was_down && self.clean && now_ms.saturating_sub(self.down_at) <= TAP_MAX_MS;
        if !is_tap {
            self.tap_at = None;
            return false;
        }
        match self.tap_at {
            Some(t) if now_ms.saturating_sub(t) <= DOUBLE_MAX_MS => {
                self.tap_at = None; // наступне натискання почне нову пару
                true
            }
            _ => {
                self.tap_at = Some(now_ms);
                false
            }
        }
    }

    /// Будь-яка інша клавіша або клік мишки: ламає і поточне, і попереднє «чисте» натискання.
    pub fn on_other(&mut self) {
        self.clean = false;
        self.tap_at = None;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tap(d: &mut DoubleShift, at: u64, hold: u64) -> bool {
        d.on_shift_down(at);
        d.on_shift_up(at + hold)
    }

    #[test]
    fn two_quick_taps_trigger() {
        let mut d = DoubleShift::default();
        assert!(!tap(&mut d, 1000, 60));
        assert!(tap(&mut d, 1200, 60));
    }

    #[test]
    fn single_tap_does_not_trigger() {
        let mut d = DoubleShift::default();
        assert!(!tap(&mut d, 1000, 60));
    }

    #[test]
    fn too_slow_second_tap_does_not_trigger_but_starts_a_new_pair() {
        let mut d = DoubleShift::default();
        assert!(!tap(&mut d, 1000, 60));
        assert!(!tap(&mut d, 2000, 60)); // пауза 900 мс — це вже нова «перша» дотик
        assert!(tap(&mut d, 2200, 60)); // а ось із нею пара склалась
    }

    #[test]
    fn shift_plus_letter_is_not_a_tap() {
        // Shift+літера для великої літери: Shift натиснули, друкуємо, відпускаємо — двічі підряд.
        let mut d = DoubleShift::default();
        d.on_shift_down(1000);
        d.on_other();
        assert!(!d.on_shift_up(1100));
        d.on_shift_down(1200);
        d.on_other();
        assert!(!d.on_shift_up(1300));
    }

    #[test]
    fn key_between_taps_breaks_the_pair() {
        let mut d = DoubleShift::default();
        assert!(!tap(&mut d, 1000, 60));
        d.on_other(); // між дотиками натиснули іншу клавішу
        assert!(!tap(&mut d, 1200, 60));
    }

    #[test]
    fn holding_shift_is_not_a_tap_and_autorepeat_is_ignored() {
        let mut d = DoubleShift::default();
        d.on_shift_down(1000);
        d.on_shift_down(1030); // автоповтор
        d.on_shift_down(1060);
        assert!(!d.on_shift_up(1600)); // тримали 600 мс
        assert!(!tap(&mut d, 1700, 60)); // і це лише «перше» натискання
    }

    #[test]
    fn click_during_shift_breaks_it_like_a_key() {
        let mut d = DoubleShift::default();
        assert!(!tap(&mut d, 1000, 60));
        d.on_shift_down(1200);
        d.on_other(); // Shift+клік (виділення)
        assert!(!d.on_shift_up(1250));
    }

    #[test]
    fn triple_tap_triggers_once_then_needs_a_new_pair() {
        let mut d = DoubleShift::default();
        assert!(!tap(&mut d, 1000, 50));
        assert!(tap(&mut d, 1150, 50));
        assert!(!tap(&mut d, 1300, 50)); // третє — початок нової пари
        assert!(tap(&mut d, 1450, 50));
    }
}
