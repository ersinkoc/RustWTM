//! One lazy Docker reader, independent of the process sampler and UI thread.
use crate::backend::Data;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, SyncSender};
use std::sync::{Arc, Mutex, PoisonError, Weak};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

struct Request {
    selected: Option<String>,
    until: Instant,
}
struct Session {
    signal: SyncSender<()>,
    latest: Arc<Mutex<Request>>,
    stop: Arc<AtomicBool>,
    worker: JoinHandle<()>,
}
struct Control {
    session: Option<Session>,
    retry: Instant,
}
pub struct DockerWorker {
    data: Weak<Mutex<Data>>,
    ctx: eframe::egui::Context,
    control: Mutex<Control>,
}
impl DockerWorker {
    pub fn new(data: &Arc<Mutex<Data>>, ctx: eframe::egui::Context) -> Self {
        Self {
            data: Arc::downgrade(data),
            ctx,
            control: Mutex::new(Control {
                session: None,
                retry: Instant::now(),
            }),
        }
    }
    #[cfg(test)]
    pub fn offline() -> Self {
        Self {
            data: Weak::new(),
            ctx: Default::default(),
            control: Mutex::new(Control {
                session: None,
                retry: Instant::now(),
            }),
        }
    }
    pub fn request(&self, selected: Option<&str>) {
        if self.data.strong_count() == 0 {
            return;
        }
        let mut control = self.control.lock().unwrap_or_else(PoisonError::into_inner);
        if control.session.is_none() && Instant::now() >= control.retry {
            let (signal, receiver) = mpsc::sync_channel(1);
            let latest = Arc::new(Mutex::new(Request {
                selected: None,
                until: Instant::now(),
            }));
            let stop = Arc::new(AtomicBool::new(false));
            let (requests, stopping, data, ctx) = (
                latest.clone(),
                stop.clone(),
                self.data.clone(),
                self.ctx.clone(),
            );
            match std::thread::Builder::new()
                .name("docker-monitor".into())
                .spawn(move || {
                    let mut sampler = wtm_core::docker::Sampler::default();
                    run(
                        receiver,
                        requests,
                        stopping,
                        data,
                        ctx,
                        move |selected, stop| sampler.sample(selected, stop),
                    );
                }) {
                Ok(worker) => {
                    control.session = Some(Session {
                        signal,
                        latest,
                        stop,
                        worker,
                    })
                }
                Err(error) => {
                    control.retry = Instant::now() + Duration::from_secs(30);
                    if let Some(data) = self.data.upgrade() {
                        data.lock().unwrap_or_else(PoisonError::into_inner).docker =
                            Arc::new(wtm_core::docker::State {
                                report: None,
                                error: Some(format!("Docker monitor could not start: {error}")),
                            });
                    }
                }
            }
        }
        if let Some(session) = &control.session {
            let mut request = session
                .latest
                .lock()
                .unwrap_or_else(PoisonError::into_inner);
            request.selected = selected.map(str::to_owned);
            request.until = Instant::now() + Duration::from_secs(6);
            drop(request);
            let _ = session.signal.try_send(());
        }
    }
}
fn run(
    receiver: mpsc::Receiver<()>,
    requests: Arc<Mutex<Request>>,
    stopping: Arc<AtomicBool>,
    data: Weak<Mutex<Data>>,
    ctx: eframe::egui::Context,
    mut sample: impl FnMut(Option<&str>, &AtomicBool) -> Result<wtm_core::docker::Report, String>,
) {
    let mut next = Instant::now();
    let mut selected = None;
    while !stopping.load(Ordering::Relaxed) {
        if matches!(
            receiver.recv_timeout(Duration::from_millis(200)),
            Err(mpsc::RecvTimeoutError::Disconnected)
        ) {
            break;
        }
        let (requested, until) = {
            let r = requests.lock().unwrap_or_else(PoisonError::into_inner);
            (r.selected.clone(), r.until)
        };
        if requested != selected {
            selected = requested;
            next = Instant::now();
        }
        if Instant::now() < next || Instant::now() >= until {
            continue;
        }
        let report = sample(selected.as_deref(), &stopping);
        if stopping.load(Ordering::Relaxed) {
            break;
        }
        let Some(data) = data.upgrade() else {
            break;
        };
        {
            let mut data = data.lock().unwrap_or_else(PoisonError::into_inner);
            let mut state = (*data.docker).clone();
            match report {
                Ok(report) => {
                    state.report = Some(report);
                    state.error = None;
                }
                Err(error) => state.error = Some(error),
            }
            data.docker = Arc::new(state);
        }
        ctx.request_repaint();
        next = Instant::now() + Duration::from_secs(2);
    }
}

impl Drop for DockerWorker {
    fn drop(&mut self) {
        let control = self
            .control
            .get_mut()
            .unwrap_or_else(PoisonError::into_inner);
        if let Some(session) = control.session.take() {
            session.stop.store(true, Ordering::Relaxed);
            let _ = session.signal.try_send(());
            let until = Instant::now() + Duration::from_secs(2);
            while !session.worker.is_finished() && Instant::now() < until {
                std::thread::sleep(Duration::from_millis(10));
            }
            if session.worker.is_finished() {
                let _ = session.worker.join();
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn slow_docker_does_not_lock_process_data_and_shutdown_discards_its_reply() {
        let data = Arc::new(Mutex::new(Data::default()));
        let requests = Arc::new(Mutex::new(Request {
            selected: Some("original".into()),
            until: Instant::now() + Duration::from_secs(10),
        }));
        let stop = Arc::new(AtomicBool::new(false));
        let (signal, receiver) = mpsc::sync_channel(1);
        let (entered, arrived) = mpsc::channel();
        let (release, gate) = mpsc::channel();
        let (weak, latest, stopping) = (Arc::downgrade(&data), requests.clone(), stop.clone());
        let worker = std::thread::spawn(move || {
            run(
                receiver,
                latest,
                stopping,
                weak,
                Default::default(),
                move |id, _| {
                    entered.send(id.map(str::to_owned)).unwrap();
                    gate.recv_timeout(Duration::from_secs(3)).unwrap();
                    Ok(crate::containers::fixture().report.unwrap())
                },
            )
        });
        signal.send(()).unwrap();
        assert_eq!(
            arrived
                .recv_timeout(Duration::from_secs(2))
                .unwrap()
                .as_deref(),
            Some("original")
        );
        assert!(
            data.try_lock().is_ok(),
            "process data remains accessible while Docker is blocked"
        );
        requests.lock().unwrap().selected = Some("replacement".into());
        stop.store(true, Ordering::Relaxed);
        release.send(()).unwrap();
        worker.join().unwrap();
        assert!(
            data.lock().unwrap().docker.report.is_none(),
            "a reply after shutdown is not published"
        );
    }
}
