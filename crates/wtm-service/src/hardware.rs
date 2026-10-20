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
}
impl Default for HardwareHub {
    fn default() -> Self {
        Self {
            report: Arc::new(RwLock::new(HardwareReport::default())),
            dry_run: Arc::new(AtomicBool::new(true)),
            worker: Mutex::new(None),
        }
    }
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
    pub fn snapshot(&self) -> HardwareReport {
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
}
