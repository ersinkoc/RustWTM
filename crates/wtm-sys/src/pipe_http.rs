//! Bounded, read-only HTTP requests over a local Windows named pipe.
//! Overlapped operations are cancelled and drained before their buffers leave scope.
use crate::handle::OwnedHandle;
use std::io;
use std::os::windows::{fs::OpenOptionsExt, io::AsRawHandle};
use std::time::{Duration, Instant};
use windows::Win32::Foundation::{
    ERROR_BROKEN_PIPE, ERROR_IO_INCOMPLETE, ERROR_IO_PENDING, HANDLE, WAIT_TIMEOUT,
};
use windows::Win32::Storage::FileSystem::{
    FILE_FLAG_OVERLAPPED, ReadFile, SECURITY_IDENTIFICATION, WriteFile,
};
use windows::Win32::System::IO::{
    CancelIoEx, GetOverlappedResult, GetOverlappedResultEx, OVERLAPPED,
};
use windows::Win32::System::Threading::{CreateEventW, ResetEvent};

fn timed_out(error: &windows::core::Error, deadline: Instant) -> bool {
    // Windows waits use whole milliseconds and may expire before Instant reaches
    // the original deadline. A zero-millisecond poll returns IO_INCOMPLETE.
    error.code() == windows::core::HRESULT::from_win32(WAIT_TIMEOUT.0)
        || error.code() == ERROR_IO_INCOMPLETE.to_hresult()
        || Instant::now() >= deadline
}

fn complete(
    handle: HANDLE,
    operation: &mut OVERLAPPED,
    started: windows::core::Result<()>,
    deadline: Instant,
) -> io::Result<usize> {
    if let Err(error) = started {
        if error.code() == ERROR_BROKEN_PIPE.to_hresult() {
            return Ok(0);
        }
        if error.code() != ERROR_IO_PENDING.to_hresult() {
            return Err(io::Error::other(error));
        }
    }
    let mut transferred = 0;
    let remaining = deadline.saturating_duration_since(Instant::now());
    // SAFETY: the handle, event, OVERLAPPED and backing buffer remain live until
    // completion (including cancellation and draining on every failure below).
    let result = unsafe {
        GetOverlappedResultEx(
            handle,
            operation,
            &mut transferred,
            remaining.as_millis().min(u128::from(u32::MAX - 1)) as u32,
            false,
        )
    };
    if let Err(error) = result {
        // SAFETY: this exact operation belongs to handle. Drain even if it
        // completed concurrently with cancellation; never free pending buffers.
        unsafe {
            let _ = CancelIoEx(handle, Some(operation));
            let _ = GetOverlappedResult(handle, operation, &mut transferred, true);
        }
        if timed_out(&error, deadline) {
            return Err(io::Error::new(
                io::ErrorKind::TimedOut,
                "Docker request timed out",
            ));
        }
        if error.code() == ERROR_BROKEN_PIPE.to_hresult() {
            return Ok(0);
        }
        return Err(io::Error::other(error));
    }
    Ok(transferred as usize)
}

/// Only local pipes and GET requests are accepted. Reads at most `limit` bytes
/// (including headers), under one wall-clock deadline for the entire exchange.
pub fn get(pipe: &str, path: &str, timeout: Duration, limit: usize) -> io::Result<Vec<u8>> {
    let name = pipe
        .strip_prefix(r"\\.\pipe\")
        .ok_or_else(|| io::Error::other("Only local Docker pipes are supported"))?;
    if name.is_empty()
        || name.len() > 128
        || !name
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"-_.".contains(&b))
        || !path.starts_with('/')
        || !path.bytes().all(|b| b.is_ascii_graphic())
        || limit == 0
        || limit > 4 * 1024 * 1024
        || timeout.is_zero()
        || timeout > Duration::from_secs(3)
    {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "Invalid Docker request bounds",
        ));
    }
    let deadline = Instant::now() + timeout;
    let file = std::fs::OpenOptions::new()
        .access_mode(crate::pipe::CLIENT_ACCESS)
        .custom_flags(FILE_FLAG_OVERLAPPED.0)
        .security_qos_flags(SECURITY_IDENTIFICATION.0)
        .open(pipe)?;
    let handle = HANDLE(file.as_raw_handle());
    // SAFETY: creates a fresh manual-reset event with no borrowed security data.
    let event = OwnedHandle::new(
        unsafe { CreateEventW(None, true, false, None) }.map_err(io::Error::other)?,
    );
    let request = format!(
        "GET {path} HTTP/1.1\r\nHost: docker\r\nConnection: close\r\nAccept: application/json\r\n\r\n"
    );
    let mut sent = 0;
    while sent < request.len() {
        // SAFETY: this request owns the event and the preceding I/O is complete.
        unsafe { ResetEvent(event.raw()) }.map_err(io::Error::other)?;
        let mut operation = OVERLAPPED {
            hEvent: event.raw(),
            ..Default::default()
        };
        // SAFETY: immutable request buffer stays live until complete drains I/O.
        let started = unsafe {
            WriteFile(
                handle,
                Some(&request.as_bytes()[sent..]),
                None,
                Some(&mut operation),
            )
        };
        let n = complete(handle, &mut operation, started, deadline)?;
        if n == 0 {
            return Err(io::Error::new(
                io::ErrorKind::WriteZero,
                "Docker pipe closed",
            ));
        }
        sent += n;
    }
    let mut response = Vec::new();
    loop {
        if Instant::now() >= deadline {
            return Err(io::Error::new(
                io::ErrorKind::TimedOut,
                "Docker request timed out",
            ));
        }
        let mut buffer = [0u8; 8192];
        // SAFETY: this request owns the event and the preceding I/O is complete.
        unsafe { ResetEvent(event.raw()) }.map_err(io::Error::other)?;
        let mut operation = OVERLAPPED {
            hEvent: event.raw(),
            ..Default::default()
        };
        // SAFETY: buffer and OVERLAPPED remain live through completion/drain.
        let started = unsafe { ReadFile(handle, Some(&mut buffer), None, Some(&mut operation)) };
        let n = complete(handle, &mut operation, started, deadline)?;
        if n == 0 {
            return Ok(response);
        }
        if response.len() + n > limit {
            return Err(io::Error::other("Docker response exceeds its size limit"));
        }
        response.extend_from_slice(&buffer[..n]);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{Read, Write};
    #[test]
    fn native_wait_timeout_does_not_depend_on_clock_rounding() {
        use windows::Win32::Foundation::{ERROR_ACCESS_DENIED, ERROR_IO_INCOMPLETE, WAIT_TIMEOUT};
        let future = Instant::now() + Duration::from_secs(1);
        for code in [WAIT_TIMEOUT.0, ERROR_IO_INCOMPLETE.0] {
            let error =
                windows::core::Error::from_hresult(windows::core::HRESULT::from_win32(code));
            assert!(timed_out(&error, future));
        }
        let other = windows::core::Error::from_hresult(ERROR_ACCESS_DENIED.to_hresult());
        assert!(!timed_out(&other, future));
        assert!(timed_out(&other, Instant::now()));
    }

    #[test]
    fn pipe_http_reads_and_bounds_an_exchange() {
        let pipe = format!(r"\\.\pipe\wtm-docker-http-{}", std::process::id());
        let mut listener =
            crate::pipe::PipeListener::bind(&pipe, "D:P(A;;GA;;;SY)(A;;GA;;;BA)(A;;GA;;;OW)")
                .unwrap();
        let worker = std::thread::spawn(move || {
            for _ in 0..2 {
                let mut connection = listener.accept().unwrap();
                let mut buf = [0; 512];
                let read = connection.read(&mut buf).unwrap();
                assert!(buf[..read].starts_with(b"GET /version HTTP/1.1\r\n"));
                let _ = connection.write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\n\r\n{}");
            }
        });
        assert!(
            get(&pipe, "/version", Duration::from_secs(1), 4096)
                .unwrap()
                .ends_with(b"{}")
        );
        assert!(get(&pipe, "/version", Duration::from_secs(1), 8).is_err());
        worker.join().unwrap();
        assert!(
            get(
                r"\\remote\pipe\engine",
                "/version",
                Duration::from_secs(1),
                100
            )
            .is_err()
        );
        assert!(get(&pipe, "/version\r\nPOST /bad", Duration::from_secs(1), 100).is_err());
    }
    #[test]
    fn a_silent_pipe_times_out_without_leaving_pending_io() {
        let pipe = format!(r"\\.\pipe\wtm-docker-silent-{}", std::process::id());
        let mut listener =
            crate::pipe::PipeListener::bind(&pipe, "D:P(A;;GA;;;SY)(A;;GA;;;BA)(A;;GA;;;OW)")
                .unwrap();
        let (release, wait) = std::sync::mpsc::channel();
        let server = std::thread::spawn(move || {
            let _connection = listener.accept().unwrap();
            wait.recv_timeout(Duration::from_secs(3)).unwrap();
        });
        let start = Instant::now();
        let error = get(&pipe, "/version", Duration::from_millis(80), 4096).unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::TimedOut);
        assert!(start.elapsed() < Duration::from_secs(2));
        release.send(()).unwrap();
        server.join().unwrap();
    }
}
