//! Мінімальний HTTP(S)-клієнт на WinHTTP (є в кожній Windows, користується системними сертифікатами й
//! налаштуваннями проксі). Потрібен лише для перевірки й завантаження оновлень; працює в окремому потоці.

use std::ffi::c_void;
use std::ptr::{null, null_mut};

use windows_sys::Win32::Foundation::GetLastError;
use windows_sys::Win32::Networking::WinHttp::{
    WinHttpCloseHandle, WinHttpConnect, WinHttpOpen, WinHttpOpenRequest, WinHttpQueryHeaders, WinHttpReadData,
    WinHttpReceiveResponse, WinHttpSendRequest, WinHttpSetTimeouts, WINHTTP_ACCESS_TYPE_AUTOMATIC_PROXY,
    WINHTTP_ACCESS_TYPE_DEFAULT_PROXY, WINHTTP_FLAG_SECURE, WINHTTP_QUERY_FLAG_NUMBER, WINHTTP_QUERY_STATUS_CODE,
};

use layout_switcher::update::{split_url, Source};

use crate::win::wide;

/// Помилка запиту: код відповіді сервера (0 — відповіді не було) і текст для користувача.
#[derive(Debug)]
pub(crate) struct HttpError {
    pub status: u32,
    pub message: String,
}

impl HttpError {
    fn new(status: u32, message: impl Into<String>) -> Self {
        HttpError { status, message: message.into() }
    }
}

/// Дескриптор WinHTTP, що закривається сам.
struct Handle(*mut c_void);

impl Drop for Handle {
    fn drop(&mut self) {
        if !self.0.is_null() {
            unsafe { WinHttpCloseHandle(self.0) };
        }
    }
}

/// Зрозумілий текст для типових помилок WinHTTP.
fn describe(code: u32) -> String {
    let what = match code {
        12002 => "час очікування вичерпано",
        12007 => "не вдалося знайти адресу сервера (немає інтернету чи DNS?)",
        12029 => "не вдалося з'єднатися з сервером",
        12030 => "з'єднання перервано",
        12038 | 12044 | 12045 | 12157 | 12175 => "помилка захищеного з'єднання (сертифікат чи TLS)",
        12163 => "немає з'єднання з інтернетом",
        _ => "помилка мережі",
    };
    format!("{what} (код WinHTTP {code})")
}

fn fail_last(stage: &str) -> HttpError {
    let code = unsafe { GetLastError() };
    HttpError::new(0, format!("{stage}: {}", describe(code)))
}

/// GET. Повертає тіло відповіді (не більше `max_bytes`), для кодів не 2xx — помилку з `status`.
pub(crate) fn get(src: &Source, url: &str, max_bytes: usize, accept: &str) -> Result<Vec<u8>, HttpError> {
    if !src.scheme_ok(url) {
        return Err(HttpError::new(0, "адреса має бути https"));
    }
    let parts = split_url(url).ok_or_else(|| HttpError::new(0, "некоректна адреса"))?;
    unsafe {
        let agent = wide(&format!("layout-switcher/{}", env!("CARGO_PKG_VERSION")));
        // Системний проксі з автоналаштуванням (Windows 8.1+); на старших — типовий.
        let mut session = Handle(WinHttpOpen(agent.as_ptr(), WINHTTP_ACCESS_TYPE_AUTOMATIC_PROXY, null(), null(), 0));
        if session.0.is_null() {
            session = Handle(WinHttpOpen(agent.as_ptr(), WINHTTP_ACCESS_TYPE_DEFAULT_PROXY, null(), null(), 0));
        }
        if session.0.is_null() {
            return Err(fail_last("WinHttpOpen"));
        }
        WinHttpSetTimeouts(session.0, 10_000, 10_000, 20_000, 30_000);

        let host = wide(&parts.host);
        let conn = Handle(WinHttpConnect(session.0, host.as_ptr(), parts.port, 0));
        if conn.0.is_null() {
            return Err(fail_last("з'єднання"));
        }
        let (verb, path) = (wide("GET"), wide(&parts.path));
        let flags = if parts.https { WINHTTP_FLAG_SECURE } else { 0 };
        let req = Handle(WinHttpOpenRequest(conn.0, verb.as_ptr(), path.as_ptr(), null(), null(), null(), flags));
        if req.0.is_null() {
            return Err(fail_last("запит"));
        }
        let headers = wide(&format!("Accept: {accept}\r\nX-GitHub-Api-Version: 2022-11-28\r\n"));
        if WinHttpSendRequest(req.0, headers.as_ptr(), u32::MAX, null(), 0, 0, 0) == 0 {
            return Err(fail_last("надсилання запиту"));
        }
        if WinHttpReceiveResponse(req.0, null_mut()) == 0 {
            return Err(fail_last("відповідь сервера"));
        }
        let mut status = 0u32;
        let mut len = 4u32;
        if WinHttpQueryHeaders(
            req.0,
            WINHTTP_QUERY_STATUS_CODE | WINHTTP_QUERY_FLAG_NUMBER,
            null(),
            &mut status as *mut u32 as *mut c_void,
            &mut len,
            null_mut(),
        ) == 0
        {
            return Err(fail_last("код відповіді"));
        }
        if !(200..300).contains(&status) {
            let why = match status {
                403 | 429 => " (ліміт запитів до GitHub — спробуй пізніше)",
                404 => " (не знайдено)",
                _ => "",
            };
            return Err(HttpError::new(status, format!("сервер відповів кодом {status}{why}")));
        }
        let mut body = Vec::new();
        let mut chunk = vec![0u8; 32 * 1024];
        loop {
            let mut n = 0u32;
            if WinHttpReadData(req.0, chunk.as_mut_ptr() as *mut c_void, chunk.len() as u32, &mut n) == 0 {
                return Err(fail_last("читання відповіді"));
            }
            if n == 0 {
                break;
            }
            if body.len() + n as usize > max_bytes {
                return Err(HttpError::new(status, format!("відповідь більша за {} КБ — відкидаю", max_bytes / 1024)));
            }
            body.extend_from_slice(&chunk[..n as usize]);
        }
        Ok(body)
    }
}
