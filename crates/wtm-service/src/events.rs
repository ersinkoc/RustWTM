//! In-memory event log shared with IPC clients, mirrored to `tracing`.

use std::collections::VecDeque;
use std::sync::{Arc, Mutex, PoisonError};
use std::time::{SystemTime, UNIX_EPOCH};

use tracing::{error, info, warn};
use wtm_core::control::ProcessRef;
use wtm_ipc::{Event, EventLevel};

const CAPACITY: usize = 2000;

#[derive(Debug, Clone, Default)]
pub struct EventLog(Arc<Mutex<Inner>>);

#[derive(Debug, Default)]
struct Inner {
    next_id: u64,
    items: VecDeque<Event>,
}

impl EventLog {
    pub fn push(&self, level: EventLevel, target: Option<&ProcessRef>, message: impl Into<String>) {
        self.record(level, target, message.into(), false);
    }

    /// A warning clients should surface as a desktop notification.
    pub fn alert(&self, target: Option<&ProcessRef>, message: impl Into<String>) {
        self.record(EventLevel::Warn, target, message.into(), true);
    }

    /// An event clients should surface as a desktop notification.
    pub fn notable(&self, target: Option<&ProcessRef>, message: impl Into<String>) {
        self.record(EventLevel::Info, target, message.into(), true);
    }

    fn record(
        &self,
        level: EventLevel,
        target: Option<&ProcessRef>,
        message: String,
        notify: bool,
    ) {
        let pid = target.map(|t| t.pid);
        let name = target.map(|t| t.name.as_str());
        match level {
            EventLevel::Info => info!(pid, name, "{message}"),
            EventLevel::Warn => warn!(pid, name, "{message}"),
            EventLevel::Error => error!(pid, name, "{message}"),
        }

        let unix_ms = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0, |d| d.as_millis() as u64);
        let mut inner = self.0.lock().unwrap_or_else(PoisonError::into_inner);
        inner.next_id += 1;
        let event = Event {
            id: inner.next_id,
            unix_ms,
            level,
            pid,
            name: name.map(Into::into),
            message,
            notify,
        };
        inner.items.push_back(event);
        if inner.items.len() > CAPACITY {
            inner.items.pop_front();
        }
    }

    pub fn info(&self, target: Option<&ProcessRef>, message: impl Into<String>) {
        self.push(EventLevel::Info, target, message);
    }

    pub fn warn(&self, target: Option<&ProcessRef>, message: impl Into<String>) {
        self.push(EventLevel::Warn, target, message);
    }

    /// Events newer than `id`, oldest first.
    pub fn after(&self, id: u64) -> Vec<Event> {
        let inner = self.0.lock().unwrap_or_else(PoisonError::into_inner);
        inner.items.iter().filter(|e| e.id > id).cloned().collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keeps_order_ids_and_capacity() {
        let log = EventLog::default();
        for i in 0..CAPACITY + 5 {
            log.info(None, format!("e{i}"));
        }
        let all = log.after(0);
        assert_eq!(all.len(), CAPACITY);
        assert_eq!(all[0].id, 6);
        assert_eq!(log.after(all.last().unwrap().id - 2).len(), 2);
    }

    #[test]
    fn records_level_target_and_notification_flag() {
        let log = EventLog::default();
        let target = ProcessRef {
            pid: 42,
            create_time: 7,
            name: "app.exe".into(),
        };
        log.push(EventLevel::Error, Some(&target), "broken");
        log.warn(None, "careful");
        log.notable(Some(&target), "look");
        let all = log.after(0);
        assert_eq!(
            all.iter().map(|e| e.level).collect::<Vec<_>>(),
            [EventLevel::Error, EventLevel::Warn, EventLevel::Info]
        );
        assert_eq!(all[0].pid, Some(42));
        assert_eq!(all[0].name.as_deref(), Some("app.exe"));
        assert_eq!(all[1].pid, None);
        assert!(!all[0].notify && all[2].notify);
        assert!(all[0].unix_ms > 0);
    }
}
