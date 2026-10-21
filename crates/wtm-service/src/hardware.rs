//! Dedicated sensor/control worker. No hardware call runs on the governor or
//! IPC threads. Profiles are session-only; startup always keeps hardware defaults.
use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Sender};
use std::sync::{Arc, Mutex, PoisonError, RwLock};
use std::time::{Duration, Instant};
use wtm_core::hardware::{HardwareReader, HardwareReport, unix_ms};
use wtm_policy::{CoolingMode, CoolingProfile, smooth_duty};

enum Message {
    Set(String, CoolingProfile, Instant, Sender<Result<(), String>>),
    Stop(Sender<()>),
}

#[derive(Debug)]
pub struct HardwareHub {
    report: Arc<RwLock<HardwareReport>>,
    dry_run: Arc<AtomicBool>,
    worker: Mutex<Option<Sender<Message>>>,
    /// When a client last asked for sensors or a fan setting.
    last_request: Mutex<Instant>,
    /// The worker is holding a fan profile, which must keep being renewed.
    controlling: Arc<AtomicBool>,
}
impl Default for HardwareHub {
    fn default() -> Self {
        Self {
            report: Arc::new(RwLock::new(HardwareReport::default())),
            dry_run: Arc::new(AtomicBool::new(true)),
            worker: Mutex::new(None),
            last_request: Mutex::new(Instant::now()),
            controlling: Arc::new(AtomicBool::new(false)),
        }
    }
}

/// The helper may stop: nobody asked for `after` and no fan is controlled.
fn idle(last_request: Instant, controlling: bool, after: Duration) -> bool {
    !controlling && last_request.elapsed() >= after
}

impl HardwareHub {
    pub fn dry_run(&self, dry: bool) {
        self.dry_run.store(dry, Ordering::Relaxed);
    }
    fn sender(&self) -> Sender<Message> {
        let mut worker = self.worker.lock().unwrap_or_else(PoisonError::into_inner);
        worker.get_or_insert_with(|| {
            let (tx, rx) = mpsc::channel();
            let (report, dry_run) = (self.report.clone(), self.dry_run.clone());
            let controlling = self.controlling.clone();
            std::thread::spawn(move || {
                let mut reader = HardwareReader::new(true);
                let mut profiles: HashMap<String, CoolingProfile> = HashMap::new();
                let mut applied: HashMap<String, (f32, Instant)> = HashMap::new();
                loop {
                    match rx.recv_timeout(Duration::from_secs(2)) {
                        Ok(Message::Stop(reply)) => { for id in profiles.keys() {let _=reader.set(id,None);} drop(reader); let _=reply.send(()); return; }
                        Err(mpsc::RecvTimeoutError::Disconnected) => break,
                        Ok(Message::Set(id, profile, deadline, reply)) => {
                            if Instant::now() >= deadline {let _=reply.send(Err("Cooling request expired before execution".into()));continue;}
                            let r = if profile.mode == CoolingMode::Automatic {
                                // Remove the request even when restoring fails; future leases
                                // must not re-apply a setting the user just disabled.
                                profiles.remove(&id); applied.remove(&id);
                                reader.set(&id, None)
                            } else if dry_run.load(Ordering::Relaxed) {
                                Err("Switch the service from monitoring to enforcing before controlling fans".into())
                            } else {
                                let snapshot = report.read().unwrap_or_else(PoisonError::into_inner).clone();
                                match snapshot.channels.iter().find(|c| c.id == id) {
                                    None => Err("Cooling channel is no longer available".into()),
                                    Some(channel) => profile.validate(channel).and_then(|()| {
                                        let duty = profile.duty(&snapshot, unix_ms()).ok_or("Temperature sensor is missing or stale")?;
                                        reader.set(&id, Some(duty))?;
                                        applied.insert(id.clone(), (duty, Instant::now()));
                                        profiles.insert(id.clone(), profile);
                                        Ok(())
                                    }),
                                }
                            };
                            if reply.send(r).is_err() {
                                // Do not keep renewing an operation whose caller timed out.
                                profiles.remove(&id); applied.remove(&id);
                                let _ = reader.set(&id, None);
                            }
                        }
                        Err(mpsc::RecvTimeoutError::Timeout) => {}
                    }
                    controlling.store(!profiles.is_empty(), Ordering::Relaxed);
                    match reader.sample() {
                        Ok(mut snapshot) => {
                            let now = unix_ms();
                            for (id, profile) in &profiles {
                                let requested = if dry_run.load(Ordering::Relaxed) { None } else { profile.duty(&snapshot, now) };
                                let duty = requested.map(|v| applied.get(id).map_or(v, |(old, at)| smooth_duty(*old, v, at.elapsed().as_secs_f32())));
                                let result = reader.set(id, duty);
                                let label = match (result, duty) {
                                    (Ok(()), Some(v)) => { applied.insert(id.clone(), (v, Instant::now())); format!("Software · {v:.0}%") },
                                    (Ok(()), None) => { applied.remove(id); "Hardware automatic · sensor unavailable or monitoring only".into() },
                                    (Err(e), _) => format!("Control failed: {e}"),
                                };
                                snapshot.cooling.push((id.clone(),label));
                            }
                            if dry_run.load(Ordering::Relaxed) {profiles.clear();applied.clear();}
                            controlling.store(!profiles.is_empty(), Ordering::Relaxed);
                            *report.write().unwrap_or_else(PoisonError::into_inner) = snapshot;
                        }
                        Err(e) => {
                            // Restore immediately on failed collection, and allow the helper's
                            // lease deadline to retry if communication itself is broken.
                            for id in profiles.keys() { let _ = reader.set(id,None); }
                            applied.clear();
                            report.write().unwrap_or_else(PoisonError::into_inner).notice = format!("Sensor collection failed: {e}");
                        }
                    }
                }
                for id in profiles.keys() { let _ = reader.set(id,None); }
            });
            tx
        }).clone()
    }
    fn requested(&self) {
        *self
            .last_request
            .lock()
            .unwrap_or_else(PoisonError::into_inner) = Instant::now();
    }
    pub fn snapshot(&self) -> HardwareReport {
        self.requested();
        self.sender();
        self.report
            .read()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
    }
    pub fn set(&self, id: String, profile: CoolingProfile) -> Result<(), String> {
        if profile.mode != CoolingMode::Automatic && self.dry_run.load(Ordering::Relaxed) {
            return Err(
                "Switch the service from monitoring to enforcing before controlling fans".into(),
            );
        }
        self.requested();
        let (reply, rx) = mpsc::channel();
        self.sender()
            .send(Message::Set(
                id,
                profile,
                Instant::now() + Duration::from_secs(6),
                reply,
            ))
            .map_err(|e| e.to_string())?;
        // Six seconds to start plus the provider's four-second control timeout.
        rx.recv_timeout(Duration::from_secs(12))
            .map_err(|e| format!("Hardware control did not acknowledge: {e}"))?
    }
    /// Stops the helper (about 100 MB and a steady trickle of CPU) once no
    /// client asked for `after` and no fan profile is active. The next
    /// request starts it again. Does not wait for the helper to exit.
    pub fn stop_if_idle(&self, after: Duration) {
        let mut worker = self.worker.lock().unwrap_or_else(PoisonError::into_inner);
        let last = *self
            .last_request
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        if worker.is_none() || !idle(last, self.controlling.load(Ordering::Relaxed), after) {
            return;
        }
        if let Some(tx) = worker.take() {
            let (reply, _) = mpsc::channel();
            let _ = tx.send(Message::Stop(reply));
        }
        // A reading kept from before the pause would look current.
        *self.report.write().unwrap_or_else(PoisonError::into_inner) = HardwareReport::default();
    }
    pub fn stop(&self) {
        if let Some(tx) = self
            .worker
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .take()
        {
            let (reply, rx) = mpsc::channel();
            let _ = tx.send(Message::Stop(reply));
            let _ = rx.recv_timeout(Duration::from_secs(5));
        }
    }
}
impl Drop for HardwareHub {
    fn drop(&mut self) {
        self.stop();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn monitoring_mode_rejects_writes_without_even_starting_a_provider() {
        let hub = HardwareHub::default();
        let profile = CoolingProfile {
            mode: CoolingMode::Manual(50.0),
            sensor_id: "temp".into(),
            ..Default::default()
        };
        assert!(
            hub.set("fan".into(), profile)
                .unwrap_err()
                .contains("monitoring")
        );
        assert!(hub.worker.lock().unwrap().is_none());
    }

    #[test]
    fn idle_needs_quiet_clients_and_no_fan_control() {
        let long_ago = Instant::now() - Duration::from_secs(60);
        let after = Duration::from_secs(30);
        assert!(idle(long_ago, false, after));
        assert!(!idle(long_ago, true, after), "a fan profile keeps it");
        assert!(!idle(Instant::now(), false, after));
    }

    #[test]
    fn the_helper_stops_when_idle_and_restarts_on_request() {
        let hub = HardwareHub::default();
        // Never started: nothing to stop.
        hub.stop_if_idle(Duration::ZERO);
        assert!(hub.worker.lock().unwrap().is_none());

        let _ = hub.snapshot();
        assert!(hub.worker.lock().unwrap().is_some());
        // Asked just now.
        hub.stop_if_idle(Duration::from_secs(30));
        assert!(hub.worker.lock().unwrap().is_some());
        // Driving a fan keeps it running however long nobody asks.
        hub.controlling.store(true, Ordering::Relaxed);
        hub.stop_if_idle(Duration::ZERO);
        assert!(hub.worker.lock().unwrap().is_some());
        hub.controlling.store(false, Ordering::Relaxed);
        hub.stop_if_idle(Duration::ZERO);
        assert!(hub.worker.lock().unwrap().is_none());
        let _ = hub.snapshot();
        assert!(hub.worker.lock().unwrap().is_some(), "started again");
        hub.stop();
    }
}
