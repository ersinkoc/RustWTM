//! Runs the real binary under the Service Control Manager: install, start,
//! a verified client connection, stop and uninstall. Needs elevation and
//! changes the machine, so it only runs where RUSTWTM_SCM_TESTS is set (CI).

use std::path::PathBuf;
use std::process::{Command, Output};
use std::time::{Duration, Instant};

const EXE: &str = env!("CARGO_BIN_EXE_wtm-service");
const SERVICE: &str = "RustWTM";

fn wtm(arg: &str) -> Output {
    Command::new(EXE).arg(arg).output().unwrap()
}

fn succeeds(arg: &str) {
    let out = wtm(arg);
    assert!(out.status.success(), "wtm-service {arg}: {out:?}");
}

/// Leaves the machine as it was, also when an assertion fails.
struct Cleanup {
    data_dir: Option<PathBuf>,
}

impl Drop for Cleanup {
    fn drop(&mut self) {
        let _ = wtm("uninstall");
        if let Some(dir) = &self.data_dir {
            let _ = std::fs::remove_dir_all(dir);
        }
    }
}

#[test]
fn installed_binary_runs_as_a_service() {
    if std::env::var_os("RUSTWTM_SCM_TESTS").is_some() {
        let data_dir = PathBuf::from(std::env::var_os("ProgramData").unwrap()).join("RustWTM");
        let _cleanup = Cleanup {
            data_dir: (!data_dir.exists()).then(|| data_dir.clone()),
        };
        succeeds("install");
        wtm_sys::services::set_start_type(SERVICE, wtm_sys::services::StartType::Manual).unwrap();
        // Under coverage, let the service process write its profile too.
        if let Some(profile) = std::env::var_os("LLVM_PROFILE_FILE") {
            let key = format!(r"HKLM\SYSTEM\CurrentControlSet\Services\{SERVICE}");
            let value = format!("LLVM_PROFILE_FILE={}", profile.to_string_lossy());
            let reg = Command::new("reg")
                .args(["add", &key, "/v", "Environment", "/t", "REG_MULTI_SZ"])
                .args(["/s", "|", "/d", &value, "/f"])
                .output()
                .unwrap();
            assert!(reg.status.success(), "{reg:?}");
        }

        succeeds("start");
        let pid = wtm_sys::services::service_pid(SERVICE).unwrap();
        // The pipe comes up once the engine has loaded its config.
        let deadline = Instant::now() + Duration::from_secs(30);
        let client = loop {
            match wtm_ipc::Client::connect() {
                Ok(c) => break c,
                Err(e) => assert!(Instant::now() < deadline, "service never served: {e}"),
            }
            std::thread::sleep(Duration::from_millis(100));
        };
        assert!(client.is_verified(), "served by service process {pid}");
        assert!(client.is_admin());
        drop(client);
        assert!(data_dir.join("config.toml").exists());

        succeeds("stop");
        assert!(wtm_sys::services::service_pid(SERVICE).is_none());
        succeeds("stop"); // already stopped

        // Uninstalling stops a running service first.
        succeeds("start");
        succeeds("uninstall");
        assert!(wtm_sys::services::service_pid(SERVICE).is_none());
        assert!(!wtm("start").status.success(), "no longer installed");
    }
}
