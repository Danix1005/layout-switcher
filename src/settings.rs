//! Налаштування програми в реєстрі: `HKCU\Software\layout-switcher` (лише поточний користувач, без прав
//! адміністратора). Зараз тут лише перевірка оновлень: чи вмикати її й коли була остання.

use std::ptr::{null, null_mut};
use std::time::{SystemTime, UNIX_EPOCH};

use windows_sys::Win32::System::Registry::{
    RegCloseKey, RegCreateKeyExW, RegOpenKeyExW, RegQueryValueExW, RegSetValueExW, HKEY, HKEY_CURRENT_USER,
    KEY_QUERY_VALUE, KEY_SET_VALUE, REG_DWORD, REG_OPTION_NON_VOLATILE,
};

use crate::win::wide;

const KEY: &str = "Software\\layout-switcher";
const AUTO_CHECK: &str = "UpdateCheck";
const LAST_CHECK: &str = "LastUpdateCheck";
const OK: u32 = 0; // ERROR_SUCCESS

fn get_u32(name: &str) -> Option<u32> {
    unsafe {
        let mut key: HKEY = null_mut();
        if RegOpenKeyExW(HKEY_CURRENT_USER, wide(KEY).as_ptr(), 0, KEY_QUERY_VALUE, &mut key) != OK {
            return None;
        }
        let (mut kind, mut value, mut bytes) = (0u32, 0u32, 4u32);
        let rc = RegQueryValueExW(key, wide(name).as_ptr(), null(), &mut kind, &mut value as *mut u32 as *mut u8, &mut bytes);
        RegCloseKey(key);
        (rc == OK && kind == REG_DWORD && bytes == 4).then_some(value)
    }
}

fn set_u32(name: &str, value: u32) -> bool {
    unsafe {
        let mut key: HKEY = null_mut();
        let rc = RegCreateKeyExW(
            HKEY_CURRENT_USER,
            wide(KEY).as_ptr(),
            0,
            null(),
            REG_OPTION_NON_VOLATILE,
            KEY_SET_VALUE,
            null(),
            &mut key,
            null_mut(),
        );
        if rc != OK {
            return false;
        }
        let rc = RegSetValueExW(key, wide(name).as_ptr(), 0, REG_DWORD, &value as *const u32 as *const u8, 4);
        RegCloseKey(key);
        rc == OK
    }
}

pub(crate) fn now() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0)
}

/// Перевіряти оновлення раз на добу? Типово так.
pub(crate) fn auto_check() -> bool {
    get_u32(AUTO_CHECK) != Some(0)
}

pub(crate) fn set_auto_check(on: bool) -> bool {
    set_u32(AUTO_CHECK, on as u32)
}

/// Коли востаннє вдало перевіряли (секунди Unix).
pub(crate) fn last_check() -> Option<u64> {
    get_u32(LAST_CHECK).map(u64::from)
}

pub(crate) fn set_last_check(t: u64) {
    set_u32(LAST_CHECK, t.min(u32::MAX as u64) as u32);
}
