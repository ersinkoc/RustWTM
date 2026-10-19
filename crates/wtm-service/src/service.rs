//! Service Control Manager integration.

use std::ffi::{OsStr, OsString};
use std::sync::mpsc;
use std::time::{Duration, Instant};

use anyhow::Context;
use tracing::error;
use windows_service::service::{
    ServiceAccess, ServiceAction, ServiceActionType, ServiceControl, ServiceControlAccept,
    ServiceErrorControl, ServiceExitCode, ServiceFailureActions, ServiceFailureResetPeriod,
    ServiceInfo, ServiceStartType, ServiceState, ServiceStatus, ServiceType,
};
use windows_service::service_control_handler::{self, ServiceControlHandlerResult};
use windows_service::service_manager::{ServiceManager, ServiceManagerAccess};
use windows_service::{define_windows_service, service_dispatcher};

use crate::engine::{self, Paths};
use crate::ipc_server::EngineMsg;
use crate::logging;

pub const SERVICE_NAME: &str = "RustWTM";
const DISPLAY_NAME: &str = "RustWTM Governor";
const DESCRIPTION: &str =
    "Keeps the system responsive by throttling processes that overload the CPU.";

define_windows_service!(ffi_service_main, service_main);

/// Entry point when started by the SCM.
pub fn dispatch() -> anyhow::Result<()> {
    service_dispatcher::start(SERVICE_NAME, ffi_service_main)
        .context("not started by the Service Control Manager; use `console` to run interactively")
}

/// SYSTEM and Administrators: full control; Users: read. Keeps the config
/// (which decides what gets throttled or killed) out of reach of standard users.
const DATA_DIR_SDDL: &str = "D:P(A;OICI;FA;;;SY)(A;OICI;FA;;;BA)(A;OICI;FRFX;;;BU)";

fn service_main(_args: Vec<OsString>) {
    let data_dir = engine::default_data_dir();
    let dacl = std::fs::create_dir_all(&data_dir)
        .map_err(|e| e.to_string())
        .and_then(|()| {
            wtm_sys::security::set_dacl(&data_dir, DATA_DIR_SDDL).map_err(|e| e.to_string())
        });
    let paths = Paths::under(&data_dir);
    let _guard = logging::init(&paths.logs, false);
    if let Err(e) = dacl {
        tracing::warn!(error = %e, "could not restrict data directory permissions");
    }
    if let Err(e) = run_service(&paths) {
        error!(error = %e, "service failed");
    }
}

fn status(state: ServiceState) -> ServiceStatus {
    ServiceStatus {
        service_type: ServiceType::OWN_PROCESS,
        current_state: state,
        controls_accepted: if state == ServiceState::Running {
            ServiceControlAccept::STOP | ServiceControlAccept::SHUTDOWN
        } else {
            ServiceControlAccept::empty()
        },
        exit_code: ServiceExitCode::Win32(0),
        checkpoint: 0,
        wait_hint: Duration::from_secs(10),
        process_id: None,
    }
}

fn run_service(paths: &Paths) -> anyhow::Result<()> {
    let (tx, rx) = mpsc::channel();
    let stop_tx = tx.clone();
    let handler = move |control| match control {
        ServiceControl::Stop | ServiceControl::Shutdown => {
            let _ = stop_tx.send(EngineMsg::Stop);
            ServiceControlHandlerResult::NoError
        }
        ServiceControl::Interrogate => ServiceControlHandlerResult::NoError,
        _ => ServiceControlHandlerResult::NotImplemented,
    };
    let handle = service_control_handler::register(SERVICE_NAME, handler)?;
    handle.set_service_status(status(ServiceState::Running))?;

    let result = engine::run(paths, tx, &rx);

    handle.set_service_status(status(ServiceState::Stopped))?;
    result
}

pub fn install() -> anyhow::Result<()> {
    let manager = ServiceManager::local_computer(
        None::<&str>,
        ServiceManagerAccess::CONNECT | ServiceManagerAccess::CREATE_SERVICE,
    )
    .context("opening the Service Control Manager (run elevated)")?;
    let info = ServiceInfo {
        name: SERVICE_NAME.into(),
        display_name: DISPLAY_NAME.into(),
        service_type: ServiceType::OWN_PROCESS,
        start_type: ServiceStartType::AutoStart,
        error_control: ServiceErrorControl::Normal,
        executable_path: std::env::current_exe()?,
        launch_arguments: vec!["service".into()],
        dependencies: vec![],
        account_name: None, // LocalSystem
        account_password: None,
    };
    let service = manager
        .create_service(&info, ServiceAccess::CHANGE_CONFIG | ServiceAccess::START)
        .context("creating service")?;
    service.set_description(DESCRIPTION)?;
    let restart = |secs| ServiceAction {
        action_type: ServiceActionType::Restart,
        delay: Duration::from_secs(secs),
    };
    service
        .update_failure_actions(ServiceFailureActions {
            reset_period: ServiceFailureResetPeriod::After(Duration::from_secs(24 * 3600)),
            reboot_msg: None,
            command: None,
            actions: Some(vec![restart(5), restart(30), restart(120)]),
        })
        .context("setting recovery actions")?;
    println!("Installed service '{SERVICE_NAME}'. Start it with `wtm-service start`.");
    println!(
        "Config: {}",
        Paths::under(&engine::default_data_dir()).config.display()
    );
    Ok(())
}

fn open(access: ServiceAccess) -> anyhow::Result<windows_service::service::Service> {
    let manager = ServiceManager::local_computer(None::<&str>, ServiceManagerAccess::CONNECT)
        .context("opening the Service Control Manager")?;
    manager
        .open_service(SERVICE_NAME, access)
        .with_context(|| format!("opening service '{SERVICE_NAME}' (installed? elevated?)"))
}

fn wait_for(
    service: &windows_service::service::Service,
    state: ServiceState,
) -> anyhow::Result<()> {
    let deadline = Instant::now() + Duration::from_secs(20);
    while service.query_status()?.current_state != state {
        anyhow::ensure!(Instant::now() < deadline, "timed out waiting for {state:?}");
        std::thread::sleep(Duration::from_millis(200));
    }
    Ok(())
}

pub fn start() -> anyhow::Result<()> {
    let service = open(ServiceAccess::START | ServiceAccess::QUERY_STATUS)?;
    service.start::<&OsStr>(&[])?;
    wait_for(&service, ServiceState::Running)?;
    println!("Service started.");
    Ok(())
}

pub fn stop() -> anyhow::Result<()> {
    let service = open(ServiceAccess::STOP | ServiceAccess::QUERY_STATUS)?;
    if service.query_status()?.current_state != ServiceState::Stopped {
        service.stop()?;
        wait_for(&service, ServiceState::Stopped)?;
    }
    println!("Service stopped.");
    Ok(())
}

pub fn uninstall() -> anyhow::Result<()> {
    let service = open(ServiceAccess::STOP | ServiceAccess::QUERY_STATUS | ServiceAccess::DELETE)?;
    if service.query_status()?.current_state != ServiceState::Stopped {
        service.stop()?;
        wait_for(&service, ServiceState::Stopped)?;
    }
    service.delete()?;
    println!("Service '{SERVICE_NAME}' removed.");
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dispatch_fails_outside_the_scm() {
        assert!(dispatch().is_err());
    }

    #[test]
    fn status_accepts_stop_only_while_running() {
        assert!(
            status(ServiceState::Running)
                .controls_accepted
                .contains(ServiceControlAccept::STOP)
        );
        assert!(status(ServiceState::Stopped).controls_accepted.is_empty());
    }

    /// Installs and removes the real service. Needs elevation and changes
    /// the machine, so it only runs where RUSTWTM_SCM_TESTS is set (CI).
    #[test]
    fn install_stop_and_uninstall() {
        if std::env::var_os("RUSTWTM_SCM_TESTS").is_none() {
            return;
        }
        install().unwrap();
        assert!(install().is_err(), "second install must fail");
        wtm_sys::services::set_start_type(SERVICE_NAME, wtm_sys::services::StartType::Manual)
            .unwrap();
        assert!(wtm_sys::services::service_pid(SERVICE_NAME).is_none());
        stop().unwrap(); // already stopped: no-op
        uninstall().unwrap();
        assert!(open(ServiceAccess::QUERY_STATUS).is_err());
    }
}
