//! Автозапуск з Windows: значення в `HKCU\Software\Microsoft\Windows\CurrentVersion\Run`.
//!
//! Це запис лише для поточного користувача, прав адміністратора не потрібно. Вмикає й вимикає його сам
//! користувач пунктом меню значка в треї; без цього програма ніде нічого не прописує.

use std::ptr::{null, null_mut};

use windows_sys::Win32::System::Registry::{
    RegCloseKey, RegCreateKeyExW, RegDeleteValueW, RegOpenKeyExW, RegQueryValueExW, RegSetValueExW, HKEY,
    HKEY_CURRENT_USER, KEY_QUERY_VALUE, KEY_SET_VALUE, REG_OPTION_NON_VOLATILE, REG_SZ,
};

use crate::win::wide;

const RUN_KEY: &str = "Software\\Microsoft\\Windows\\CurrentVersion\\Run";
const VALUE: &str = "layout-switcher";
const OK: u32 = 0; // ERROR_SUCCESS

/// Що зараз записано в автозапуску (рядок запуску), якщо записано.
pub(crate) fn current() -> Option<String> {
    unsafe {
        let mut key: HKEY = null_mut();
        if RegOpenKeyExW(HKEY_CURRENT_USER, wide(RUN_KEY).as_ptr(), 0, KEY_QUERY_VALUE, &mut key) != OK {
            return None;
        }
        let name = wide(VALUE);
        let mut kind = 0u32;
        let mut bytes = 0u32;
        let mut result = None;
        if RegQueryValueExW(key, name.as_ptr(), null(), &mut kind, null_mut(), &mut bytes) == OK
            && kind == REG_SZ
            && bytes >= 2
        {
            let mut buf = vec![0u16; (bytes as usize + 1) / 2];
            if RegQueryValueExW(key, name.as_ptr(), null(), &mut kind, buf.as_mut_ptr() as *mut u8, &mut bytes) == OK {
                let n = (bytes as usize / 2).min(buf.len());
                result = Some(String::from_utf16_lossy(&buf[..n]).trim_end_matches('\0').to_string());
            }
        }
        RegCloseKey(key);
        result
    }
}

/// Записати (або оновити) автозапуск.
pub(crate) fn enable(command: &str) -> bool {
    unsafe {
        let mut key: HKEY = null_mut();
        let rc = RegCreateKeyExW(
            HKEY_CURRENT_USER,
            wide(RUN_KEY).as_ptr(),
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
        let data = wide(command);
        let rc = RegSetValueExW(key, wide(VALUE).as_ptr(), 0, REG_SZ, data.as_ptr() as *const u8, (data.len() * 2) as u32);
        RegCloseKey(key);
        rc == OK
    }
}

/// Прибрати автозапуск. Якщо запису й не було — теж успіх.
pub(crate) fn disable() -> bool {
    unsafe {
        let mut key: HKEY = null_mut();
        if RegOpenKeyExW(HKEY_CURRENT_USER, wide(RUN_KEY).as_ptr(), 0, KEY_SET_VALUE, &mut key) != OK {
            return true;
        }
        let rc = RegDeleteValueW(key, wide(VALUE).as_ptr());
        RegCloseKey(key);
        rc == OK || rc == 2 // 2 = ERROR_FILE_NOT_FOUND
    }
}
