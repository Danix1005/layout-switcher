//! Логіка без залежності від Windows — тестується на будь-якій ОС (`cargo test --lib`):
//! таблиці розкладок, детектор «не тієї» розкладки, подвійний Shift, правила для застосунків,
//! політика щодо програмного вводу, пам'ять про перебіг набору (коли перемикати розкладку),
//! захист логу від паролів, розбір параметрів командного рядка, логіка оновлення з GitHub (версії, JSON, SHA-256).
pub mod apps;
pub mod cli;
pub mod detect;
pub mod hotkey;
pub mod inject;
pub mod json;
pub mod keymap;
pub mod privacy;
pub mod session;
pub mod sha256;
pub mod update;
