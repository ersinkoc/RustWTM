//! Named-pipe server with an explicit DACL and client impersonation.

use std::fs::File;
use std::io::{self, Read, Write};
use std::os::windows::io::{AsRawHandle, FromRawHandle, RawHandle};

use windows::Win32::Foundation::{ERROR_PIPE_CONNECTED, HANDLE, HLOCAL, LocalFree};
use windows::Win32::Security::Authorization::{
    ConvertStringSecurityDescriptorToSecurityDescriptorW, SDDL_REVISION_1,
};
use windows::Win32::Security::{
    CheckTokenMembership, CreateWellKnownSid, PSECURITY_DESCRIPTOR, PSID, RevertToSelf,
    SECURITY_ATTRIBUTES, SECURITY_MAX_SID_SIZE, WinBuiltinAdministratorsSid,
};
use windows::Win32::Storage::FileSystem::{FILE_FLAG_FIRST_PIPE_INSTANCE, PIPE_ACCESS_DUPLEX};
use windows::Win32::System::Pipes::{
    ConnectNamedPipe, CreateNamedPipeW, GetNamedPipeClientProcessId, GetNamedPipeClientSessionId,
    GetNamedPipeServerProcessId, ImpersonateNamedPipeClient, PIPE_READMODE_BYTE,
    PIPE_REJECT_REMOTE_CLIENTS, PIPE_TYPE_BYTE, PIPE_UNLIMITED_INSTANCES, PIPE_WAIT,
};
use windows::core::PCWSTR;

use crate::Result;

const BUFFER_SIZE: u32 = 64 * 1024;

struct SecurityDescriptor(PSECURITY_DESCRIPTOR);

// SAFETY: the descriptor is an immutable heap block owned by this wrapper.
unsafe impl Send for SecurityDescriptor {}

impl Drop for SecurityDescriptor {
    fn drop(&mut self) {
        // SAFETY: allocated by ConvertStringSecurityDescriptorToSecurityDescriptorW.
        unsafe { LocalFree(Some(HLOCAL(self.0.0))) };
    }
}

/// Accepts connections on a local-only named pipe.
///
/// One unconnected instance is always kept open so another process cannot
/// grab the name between connections; the very first instance is created
/// with `FILE_FLAG_FIRST_PIPE_INSTANCE` so binding fails if someone else
/// already owns the name.
pub struct PipeListener {
    name: Vec<u16>,
    security: SecurityDescriptor,
    pending: Option<File>,
}

impl PipeListener {
    /// `sddl` is the pipe's security descriptor, e.g. `D:P(A;;GA;;;SY)`.
    pub fn bind(name: &str, sddl: &str) -> Result<Self> {
        let sddl: Vec<u16> = sddl.encode_utf16().chain([0]).collect();
        let mut sd = PSECURITY_DESCRIPTOR::default();
        // SAFETY: `sddl` is NUL-terminated; `sd` receives a LocalAlloc'd block.
        unsafe {
            ConvertStringSecurityDescriptorToSecurityDescriptorW(
                PCWSTR(sddl.as_ptr()),
                SDDL_REVISION_1,
                &mut sd,
                None,
            )?
        };
        let mut listener = Self {
            name: name.encode_utf16().chain([0]).collect(),
            security: SecurityDescriptor(sd),
            pending: None,
        };
        listener.pending = Some(listener.create_instance(true)?);
        Ok(listener)
    }

    fn create_instance(&self, first: bool) -> Result<File> {
        let sa = SECURITY_ATTRIBUTES {
            nLength: size_of::<SECURITY_ATTRIBUTES>() as u32,
            lpSecurityDescriptor: self.security.0.0,
            bInheritHandle: false.into(),
        };
        let mut open_mode = PIPE_ACCESS_DUPLEX;
        if first {
            open_mode |= FILE_FLAG_FIRST_PIPE_INSTANCE;
        }
        // SAFETY: `name` is NUL-terminated and `sa` outlives the call.
        let handle = unsafe {
            CreateNamedPipeW(
                PCWSTR(self.name.as_ptr()),
                open_mode,
                PIPE_TYPE_BYTE | PIPE_READMODE_BYTE | PIPE_WAIT | PIPE_REJECT_REMOTE_CLIENTS,
                PIPE_UNLIMITED_INSTANCES,
                BUFFER_SIZE,
                BUFFER_SIZE,
                0,
                Some(&sa),
            )
        };
        if handle.is_invalid() {
            return Err(windows::core::Error::from_thread().into());
        }
        // SAFETY: we exclusively own the freshly created handle.
        Ok(unsafe { File::from_raw_handle(handle.0 as RawHandle) })
    }

    /// Blocks until a client connects.
    pub fn accept(&mut self) -> Result<PipeConnection> {
        let file = match self.pending.take() {
            Some(f) => f,
            None => self.create_instance(false)?,
        };
        // SAFETY: `file` owns a valid pipe handle.
        match unsafe { ConnectNamedPipe(raw(&file), None) } {
            Ok(()) => {}
            Err(e) if e.code() == ERROR_PIPE_CONNECTED.to_hresult() => {}
            Err(e) => return Err(e.into()),
        }
        self.pending = Some(self.create_instance(false)?);
        Ok(PipeConnection { file })
    }
}

/// PID of the process that created the server end of a client pipe.
pub fn server_pid(client: &File) -> Result<u32> {
    let mut pid = 0;
    // SAFETY: client owns a valid pipe handle; pid is writable.
    unsafe { GetNamedPipeServerProcessId(raw(client), &mut pid)? };
    Ok(pid)
}

fn raw(file: &File) -> HANDLE {
    HANDLE(file.as_raw_handle())
}

/// A connected server-side pipe instance.
pub struct PipeConnection {
    file: File,
}

impl PipeConnection {
    pub fn client_pid(&self) -> Result<u32> {
        let mut pid = 0;
        // SAFETY: valid pipe handle; `pid` is writable.
        unsafe { GetNamedPipeClientProcessId(raw(&self.file), &mut pid)? };
        Ok(pid)
    }

    /// The Terminal Services session of the connected client.
    pub fn client_session_id(&self) -> Result<u32> {
        let mut session = 0u32;
        // SAFETY: valid pipe handle; `session` is a writable u32.
        unsafe { GetNamedPipeClientSessionId(raw(&self.file), &mut session)? };
        Ok(session)
    }

    /// Runs `f` on this thread while impersonating the client, so access
    /// checks inside `f` happen with the client's rights.
    ///
    /// The client must have sent data before impersonation is possible.
    pub fn as_client<T>(&self, f: impl FnOnce() -> T) -> Result<T> {
        // SAFETY: valid, connected pipe handle.
        unsafe { ImpersonateNamedPipeClient(raw(&self.file))? };
        let out = f();
        // SAFETY: undoes the impersonation above. Continuing with the
        // client's token would be a privilege bug, so failure is fatal.
        unsafe { RevertToSelf() }.expect("RevertToSelf failed");
        Ok(out)
    }

    /// Whether the client's (elevated) token is in BUILTIN\Administrators.
    pub fn client_is_admin(&self) -> Result<bool> {
        self.as_client(is_current_token_admin)?
    }
}

fn is_current_token_admin() -> Result<bool> {
    let mut sid = [0u8; SECURITY_MAX_SID_SIZE as usize];
    let mut size = sid.len() as u32;
    let psid = PSID(sid.as_mut_ptr().cast());
    // SAFETY: `sid` is large enough for any SID.
    unsafe { CreateWellKnownSid(WinBuiltinAdministratorsSid, None, Some(psid), &mut size)? };
    let mut member = windows::core::BOOL::default();
    // SAFETY: None = the calling thread's (impersonation) token.
    unsafe { CheckTokenMembership(None, psid, &mut member)? };
    Ok(member.as_bool())
}

impl Read for PipeConnection {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        self.file.read(buf)
    }
}

impl Write for PipeConnection {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.file.write(buf)
    }

    fn flush(&mut self) -> io::Result<()> {
        self.file.flush()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs::OpenOptions;

    #[test]
    fn round_trip_and_identity() {
        let name = format!(r"\\.\pipe\wtm-test-{}", std::process::id());
        let mut listener = PipeListener::bind(
            &name,
            "D:P(A;;GA;;;SY)(A;;GA;;;BA)(A;;GRGW;;;IU)(A;;GA;;;OW)",
        )
        .unwrap();

        // A second listener on the same name must fail (anti-squatting).
        assert!(PipeListener::bind(&name, "D:P(A;;GA;;;OW)").is_err());

        let client_name = name.clone();
        let client = std::thread::spawn(move || {
            let mut c = OpenOptions::new()
                .read(true)
                .write(true)
                .open(&client_name)
                .unwrap();
            c.write_all(b"ping").unwrap();
            let mut buf = [0u8; 4];
            c.read_exact(&mut buf).unwrap();
            assert_eq!(&buf, b"pong");
        });

        let mut conn = listener.accept().unwrap();
        let mut buf = [0u8; 4];
        conn.read_exact(&mut buf).unwrap();
        assert_eq!(&buf, b"ping");
        assert_eq!(conn.client_pid().unwrap(), std::process::id());
        let me = crate::process::ProcessSnapshotter::new()
            .snapshot()
            .unwrap()
            .into_iter()
            .find(|p| p.pid == std::process::id())
            .unwrap();
        assert_eq!(conn.client_session_id().unwrap(), me.session_id);
        // Same token on both ends, so impersonation must agree with a direct check.
        assert_eq!(
            conn.client_is_admin().unwrap(),
            is_current_token_admin().unwrap()
        );
        conn.write_all(b"pong").unwrap();
        client.join().unwrap();
    }

    const SDDL: &str = "D:P(A;;GA;;;SY)(A;;GA;;;BA)(A;;GA;;;OW)";

    fn open_client(name: &str) -> std::io::Result<File> {
        OpenOptions::new().read(true).write(true).open(name)
    }

    #[test]
    fn rejects_malformed_sddl() {
        let name = format!(r"\\.\pipe\wtm-test-sddl-{}", std::process::id());
        assert!(PipeListener::bind(&name, "not an sddl string").is_err());
    }

    #[test]
    fn client_connecting_before_accept_and_server_pid() {
        let name = format!(r"\\.\pipe\wtm-test-early-{}", std::process::id());
        let mut listener = PipeListener::bind(&name, SDDL).unwrap();
        // The pending instance accepts the client before accept() runs, so
        // ConnectNamedPipe reports ERROR_PIPE_CONNECTED.
        let mut client = open_client(&name).unwrap();
        assert_eq!(server_pid(&client).unwrap(), std::process::id());
        let mut conn = listener.accept().unwrap();

        conn.write_all(b"hi").unwrap();
        conn.flush().unwrap();
        let mut buf = [0u8; 2];
        client.read_exact(&mut buf).unwrap();
        assert_eq!(&buf, b"hi");
    }

    #[test]
    fn client_gone_before_accept_then_recovers() {
        let name = format!(r"\\.\pipe\wtm-test-gone-{}", std::process::id());
        let mut listener = PipeListener::bind(&name, SDDL).unwrap();
        // Connect and disconnect before accept(): ConnectNamedPipe fails.
        drop(open_client(&name).unwrap());
        assert!(listener.accept().is_err());

        // No pending instance is left, so the next accept() creates one; the
        // client retries until it exists.
        let client_name = name.clone();
        let client = std::thread::spawn(move || {
            let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
            loop {
                match open_client(&client_name) {
                    Ok(mut c) => return c.write_all(b"ok"),
                    Err(e) if std::time::Instant::now() >= deadline => return Err(e),
                    Err(_) => std::thread::sleep(std::time::Duration::from_millis(10)),
                }
            }
        });
        let mut conn = listener.accept().unwrap();
        let mut buf = [0u8; 2];
        conn.read_exact(&mut buf).unwrap();
        assert_eq!(&buf, b"ok");
        client.join().unwrap().unwrap();
    }
}
