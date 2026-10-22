//! Minimal HTTPS / HTTP GET and POST through WinHTTP (system proxy,
//! redirects followed), and SHA-256 through CNG, for update checks,
//! downloads and the Telegram bot.

use core::ffi::c_void;

use windows::Win32::Networking::WinHttp::{
    URL_COMPONENTS, WINHTTP_ACCESS_TYPE_AUTOMATIC_PROXY, WINHTTP_FLAG_SECURE,
    WINHTTP_OPEN_REQUEST_FLAGS, WINHTTP_QUERY_FLAG_NUMBER, WINHTTP_QUERY_STATUS_CODE,
    WinHttpCloseHandle, WinHttpConnect, WinHttpCrackUrl, WinHttpOpen, WinHttpOpenRequest,
    WinHttpQueryHeaders, WinHttpReadData, WinHttpReceiveResponse, WinHttpSendRequest,
    WinHttpSetTimeouts,
};
use windows::Win32::Security::Cryptography::{BCRYPT_SHA256_ALG_HANDLE, BCryptHash};
use windows::core::PCWSTR;

use crate::{Error, Result};

/// `INTERNET_SCHEME_HTTPS`.
const SCHEME_HTTPS: i32 = 2;

struct Handle(*mut c_void);

impl Handle {
    fn new(raw: *mut c_void) -> Result<Self> {
        if raw.is_null() {
            Err(windows::core::Error::from_thread().into())
        } else {
            Ok(Self(raw))
        }
    }
}

impl Drop for Handle {
    fn drop(&mut self) {
        // SAFETY: a WinHTTP handle we opened.
        unsafe {
            let _ = WinHttpCloseHandle(self.0);
        }
    }
}

fn wide_z(units: &[u16]) -> Vec<u16> {
    units.iter().copied().chain([0]).collect()
}

/// What one answer may take, in bytes and in all: each receive has its own
/// timeout, so a server trickling bytes (or sending without end) would
/// otherwise hold the caller, the service among them, forever.
#[derive(Debug, Clone, Copy)]
struct Limit {
    bytes: usize,
    time: std::time::Duration,
}

/// Downloads `url` (an installer, at most 512 MiB in 15 minutes); returns
/// the HTTP status and the body.
pub fn get(url: &str, user_agent: &str) -> Result<(u16, Vec<u8>)> {
    let limit = Limit {
        bytes: 512 << 20,
        time: std::time::Duration::from_secs(15 * 60),
    };
    request("GET", url, None, user_agent, 30_000, limit)
}

/// Sends `body` (of `content_type`) to `url`, waiting up to `timeout_ms`
/// for the answer; returns the HTTP status and the body.
pub fn post(
    url: &str,
    content_type: &str,
    body: &[u8],
    user_agent: &str,
    timeout_ms: i32,
) -> Result<(u16, Vec<u8>)> {
    // An API answer: a few MiB at most, within the wait plus a minute.
    let limit = Limit {
        bytes: 8 << 20,
        time: std::time::Duration::from_millis(timeout_ms.max(0) as u64 + 60_000),
    };
    request(
        "POST",
        url,
        Some((content_type, body)),
        user_agent,
        timeout_ms,
        limit,
    )
}

fn request(
    method: &str,
    url: &str,
    body: Option<(&str, &[u8])>,
    user_agent: &str,
    timeout_ms: i32,
    limit: Limit,
) -> Result<(u16, Vec<u8>)> {
    let deadline = std::time::Instant::now() + limit.time;
    let url_w: Vec<u16> = url.encode_utf16().collect();
    // Null pointers with non-zero lengths: WinHTTP points into `url_w`.
    let mut parts = URL_COMPONENTS {
        dwStructSize: size_of::<URL_COMPONENTS>() as u32,
        dwHostNameLength: 1,
        dwUrlPathLength: 1,
        dwExtraInfoLength: 1,
        ..Default::default()
    };
    // SAFETY: `parts` is initialised; the results point into `url_w`.
    unsafe { WinHttpCrackUrl(&url_w, 0, &mut parts)? };
    let slice = |ptr: *mut u16, len: u32| -> Vec<u16> {
        if ptr.is_null() {
            return Vec::new();
        }
        // SAFETY: WinHTTP returned a pointer into `url_w` with this length.
        unsafe { std::slice::from_raw_parts(ptr, len as usize).to_vec() }
    };
    let host = wide_z(&slice(parts.lpszHostName.0, parts.dwHostNameLength));
    let mut object = slice(parts.lpszUrlPath.0, parts.dwUrlPathLength);
    object.extend(slice(parts.lpszExtraInfo.0, parts.dwExtraInfoLength));
    if object.is_empty() {
        object.push(u16::from(b'/'));
    }
    let object = wide_z(&object);
    let agent: Vec<u16> = user_agent.encode_utf16().chain([0]).collect();
    let verb: Vec<u16> = method.encode_utf16().chain([0]).collect();
    let headers: Vec<u16> = body
        .map(|(t, _)| format!("Content-Type: {t}\r\n"))
        .unwrap_or_default()
        .encode_utf16()
        .collect();
    let payload = body.map_or(&[][..], |(_, b)| b);
    let flags = if parts.nScheme.0 == SCHEME_HTTPS {
        WINHTTP_FLAG_SECURE
    } else {
        WINHTTP_OPEN_REQUEST_FLAGS(0)
    };

    // SAFETY: every string is NUL-terminated and outlives its call; the
    // handles are closed by `Handle` in reverse order.
    unsafe {
        let session = Handle::new(WinHttpOpen(
            PCWSTR(agent.as_ptr()),
            WINHTTP_ACCESS_TYPE_AUTOMATIC_PROXY,
            PCWSTR::null(),
            PCWSTR::null(),
            0,
        ))?;
        WinHttpSetTimeouts(session.0, 10_000, 10_000, 30_000, timeout_ms)?;
        let connection = Handle::new(WinHttpConnect(
            session.0,
            PCWSTR(host.as_ptr()),
            parts.nPort,
            0,
        ))?;
        let request = Handle::new(WinHttpOpenRequest(
            connection.0,
            PCWSTR(verb.as_ptr()),
            PCWSTR(object.as_ptr()),
            PCWSTR::null(),
            PCWSTR::null(),
            std::ptr::null(),
            flags,
        ))?;
        WinHttpSendRequest(
            request.0,
            (!headers.is_empty()).then_some(headers.as_slice()),
            (!payload.is_empty()).then_some(payload.as_ptr().cast()),
            payload.len() as u32,
            payload.len() as u32,
            0,
        )?;
        WinHttpReceiveResponse(request.0, std::ptr::null_mut())?;
        let mut status = 0u32;
        let mut len = size_of::<u32>() as u32;
        WinHttpQueryHeaders(
            request.0,
            WINHTTP_QUERY_STATUS_CODE | WINHTTP_QUERY_FLAG_NUMBER,
            PCWSTR::null(),
            Some((&raw mut status).cast()),
            &mut len,
            std::ptr::null_mut(),
        )?;
        let mut body = Vec::new();
        let mut buf = vec![0u8; 64 * 1024];
        loop {
            let mut read = 0u32;
            WinHttpReadData(
                request.0,
                buf.as_mut_ptr().cast(),
                buf.len() as u32,
                &mut read,
            )?;
            if read == 0 {
                break;
            }
            if body.len() + read as usize > limit.bytes {
                return Err(windows::core::Error::new(
                    windows::Win32::Foundation::ERROR_FILE_TOO_LARGE.to_hresult(),
                    format!("the answer is larger than {} bytes", limit.bytes),
                )
                .into());
            }
            body.extend_from_slice(&buf[..read as usize]);
            if std::time::Instant::now() >= deadline {
                return Err(windows::core::Error::new(
                    windows::Win32::Foundation::ERROR_TIMEOUT.to_hresult(),
                    format!("the answer took longer than {} s", limit.time.as_secs()),
                )
                .into());
            }
        }
        Ok((status as u16, body))
    }
}

/// SHA-256 of `data`.
pub fn sha256(data: &[u8]) -> Result<[u8; 32]> {
    let mut out = [0u8; 32];
    // SAFETY: the pseudo-handle needs no opening; `out` is 32 bytes.
    let status = unsafe { BCryptHash(BCRYPT_SHA256_ALG_HANDLE, None, data, &mut out) };
    if status.is_err() {
        return Err(Error::Nt {
            call: "BCryptHash",
            status: status.0 as u32,
        });
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{Read, Write};
    use std::net::TcpListener;

    /// Answers one request per item of `replies` on a local port.
    fn serve(replies: Vec<&'static str>) -> (u16, std::thread::JoinHandle<Vec<String>>) {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let t = std::thread::spawn(move || {
            let mut requests = Vec::new();
            for reply in replies {
                let (mut s, _) = listener.accept().unwrap();
                // Headers, then as much body as they announce.
                let mut req = Vec::new();
                let mut buf = [0u8; 4096];
                loop {
                    let n = s.read(&mut buf).unwrap();
                    req.extend_from_slice(&buf[..n]);
                    let text = String::from_utf8_lossy(&req);
                    let Some(end) = text.find("\r\n\r\n") else {
                        continue;
                    };
                    let len = text
                        .lines()
                        .find_map(|l| l.strip_prefix("Content-Length: "))
                        .map_or(0, |v| v.trim().parse::<usize>().unwrap());
                    if n == 0 || req.len() >= end + 4 + len {
                        break;
                    }
                }
                requests.push(String::from_utf8_lossy(&req).into_owned());
                s.write_all(reply.as_bytes()).unwrap();
            }
            requests
        });
        (port, t)
    }

    #[test]
    fn an_answer_too_large_or_too_slow_is_cut_off() {
        let big: &'static str = Box::leak(
            format!(
                "HTTP/1.1 200 OK
Content-Length: 4096
Connection: close

{}",
                "x".repeat(4096)
            )
            .into_boxed_str(),
        );
        let (port, server) = serve(vec![big]);
        let limit = Limit {
            bytes: 1024,
            time: std::time::Duration::from_secs(60),
        };
        let url = format!("http://127.0.0.1:{port}/");
        let e = request("GET", &url, None, "wtm-test", 30_000, limit).unwrap_err();
        assert!(e.to_string().contains("larger than 1024 bytes"), "{e}");
        server.join().unwrap();

        // A byte at a time: every receive is quick, the whole is not.
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let trickle = std::thread::spawn(move || {
            let (mut s, _) = listener.accept().unwrap();
            let mut buf = [0u8; 4096];
            let _ = s.read(&mut buf);
            let _ = s.write_all(
                b"HTTP/1.1 200 OK
Content-Length: 100

",
            );
            for _ in 0..100 {
                if s.write_all(b"x").is_err() {
                    break;
                }
                std::thread::sleep(std::time::Duration::from_millis(20));
            }
        });
        let limit = Limit {
            bytes: 1 << 20,
            time: std::time::Duration::from_millis(300),
        };
        let url = format!("http://127.0.0.1:{port}/");
        let e = request("GET", &url, None, "wtm-test", 30_000, limit).unwrap_err();
        assert!(e.to_string().contains("took longer"), "{e}");
        trickle.join().unwrap();
    }

    #[test]
    fn gets_a_body_and_status_and_follows_redirects() {
        let (port, server) = serve(vec![
            "HTTP/1.1 302 Found\r\nLocation: /final\r\nContent-Length: 0\r\n\r\n",
            "HTTP/1.1 200 OK\r\nContent-Length: 5\r\nConnection: close\r\n\r\nhello",
            "HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
        ]);
        let (status, body) =
            get(&format!("http://127.0.0.1:{port}/start?x=1"), "wtm-test").unwrap();
        assert_eq!((status, body.as_slice()), (200, &b"hello"[..]));
        let (status, _) = get(&format!("http://127.0.0.1:{port}"), "wtm-test").unwrap();
        assert_eq!(status, 404);
        let requests = server.join().unwrap();
        assert!(
            requests[0].starts_with("GET /start?x=1 HTTP/1.1"),
            "{}",
            requests[0]
        );
        assert!(
            requests[0].contains("User-Agent: wtm-test"),
            "{}",
            requests[0]
        );
        assert!(requests[1].starts_with("GET /final "), "{}", requests[1]);
        assert!(requests[2].starts_with("GET / "), "{}", requests[2]);
    }

    #[test]
    fn posts_a_body_with_its_content_type() {
        let (port, server) = serve(vec![
            "HTTP/1.1 200 OK\r\nContent-Length: 2\r\nConnection: close\r\n\r\nok",
        ]);
        let (status, body) = post(
            &format!("http://127.0.0.1:{port}/bot/send"),
            "application/json",
            br#"{"a":1}"#,
            "wtm-test",
            5_000,
        )
        .unwrap();
        assert_eq!((status, body.as_slice()), (200, &b"ok"[..]));
        let request = &server.join().unwrap()[0];
        assert!(request.starts_with("POST /bot/send HTTP/1.1"), "{request}");
        assert!(
            request.contains("Content-Type: application/json"),
            "{request}"
        );
        assert!(request.ends_with(r#"{"a":1}"#), "{request}");
    }

    #[test]
    fn bad_urls_and_unreachable_hosts_fail() {
        assert!(get("not a url", "wtm-test").is_err());
        // Nothing listens on port 1.
        assert!(get("http://127.0.0.1:1/", "wtm-test").is_err());
    }

    #[test]
    fn hashes_with_sha256() {
        let hex = |b: [u8; 32]| b.iter().map(|x| format!("{x:02x}")).collect::<String>();
        assert_eq!(
            hex(sha256(b"abc").unwrap()),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
    }
}
