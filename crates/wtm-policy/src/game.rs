//! Game mode: while a full-screen app is in front (a game, a video, a
//! presentation), busy background apps are calmed down so the game gets the
//! machine, and everything is handed back when it ends.

use std::collections::{HashMap, HashSet};

use serde::Deserialize;
use wtm_core::control::ProcessRef;
use wtm_core::{ProcessInfo, protection_reason};

use crate::{Action, Decision, Key, PriorityLevel, Source, glob_match};

/// RustWTM's own programs, never calmed.
const OWN_IMAGES: [&str; 3] = ["rustwtm.exe", "wtm.exe", "wtm-service.exe"];

#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct GameModeConfig {
    pub enabled: bool,
    /// Background apps using at least this share of the machine are calmed.
    pub background_cpu_percent: f32,
    /// What calming means; rolled back when the game ends.
    pub actions: Vec<Action>,
    /// Switch to the High performance power plan while a game runs, and
    /// back to the previous plan afterwards.
    pub power_plan: bool,
    /// Image-name wildcards never calmed (streaming, voice chat).
    pub exempt: Vec<String>,
}

impl Default for GameModeConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            background_cpu_percent: 2.0,
            actions: vec![Action::Priority(PriorityLevel::BelowNormal), Action::EcoQos],
            power_plan: false,
            exempt: vec![
                "obs*.exe".into(),
                "discord.exe".into(),
                "audiodg.exe".into(),
                "explorer.exe".into(),
            ],
        }
    }
}

impl GameModeConfig {
    pub(crate) fn validate(&self) -> Result<(), String> {
        if !(0.0..=100.0).contains(&self.background_cpu_percent) {
            return Err("background_cpu_percent must be within 0..=100".into());
        }
        for a in &self.actions {
            if matches!(a, Action::CpuCapRelative(_)) {
                return Err("cpu_cap_relative is not supported for game mode; use cpu_cap".into());
            }
            a.validate()?;
        }
        Ok(())
    }
}

/// A change of game, for side effects such as the power plan.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GameChange {
    Started(String),
    Ended(String),
}

#[derive(Debug, Default)]
pub struct GameMode {
    cfg: GameModeConfig,
    /// The game's name and its processes.
    game: Option<(String, HashSet<Key>)>,
    calmed: HashMap<Key, ProcessRef>,
}

impl GameMode {
    pub fn new(cfg: GameModeConfig) -> Self {
        Self {
            cfg,
            ..Self::default()
        }
    }

    pub fn config(&self) -> &GameModeConfig {
        &self.cfg
    }

    pub fn set_config(&mut self, cfg: GameModeConfig) {
        self.cfg = cfg;
    }

    /// The game in front, if game mode is on.
    pub fn game(&self) -> Option<&str> {
        self.game.as_ref().map(|(n, _)| n.as_str())
    }

    pub fn calmed(&self) -> usize {
        self.calmed.len()
    }

    /// `game`: the full-screen foreground app and its processes, if any.
    /// Returns what to apply or roll back, and whether a game started or ended.
    pub fn tick(
        &mut self,
        game: Option<(String, HashSet<Key>)>,
        procs: &[ProcessInfo],
    ) -> (Vec<Decision>, Option<GameChange>) {
        let game = game.filter(|_| self.cfg.enabled);
        let before = self.game().map(str::to_owned);
        let now = game.as_ref().map(|(n, _)| n.clone());
        let change = match (&before, &now) {
            (None, Some(n)) => Some(GameChange::Started(n.clone())),
            (Some(b), None) => Some(GameChange::Ended(b.clone())),
            (Some(b), Some(n)) if b != n => Some(GameChange::Started(n.clone())),
            _ => None,
        };
        self.game = game;
        if !self.calmed.is_empty() {
            let live: HashSet<Key> = procs.iter().map(|p| (p.pid, p.create_time)).collect();
            self.calmed.retain(|k, _| live.contains(k));
        }

        let mut out = Vec::new();
        let Some((name, keys)) = &self.game else {
            for (_, target) in self.calmed.drain() {
                out.push(Decision::Restore {
                    source: Source::GameMode,
                    target,
                    reason: "game ended".into(),
                });
            }
            return (out, change);
        };
        // Processes that became part of the game are handed back.
        let joined: Vec<Key> = self
            .calmed
            .keys()
            .filter(|k| keys.contains(k))
            .copied()
            .collect();
        for k in joined {
            if let Some(target) = self.calmed.remove(&k) {
                out.push(Decision::Restore {
                    source: Source::GameMode,
                    target,
                    reason: "part of the game".into(),
                });
            }
        }
        // Only the game's own session is calmed: the full-screen report comes
        // from the window in that session, so it must not reach other users'
        // apps or the services in session 0.
        let session = procs
            .iter()
            .find(|p| keys.contains(&(p.pid, p.create_time)))
            .map(|p| p.session_id)
            .filter(|&s| s != 0);
        for p in procs {
            let key = (p.pid, p.create_time);
            if keys.contains(&key)
                || Some(p.session_id) != session
                || self.calmed.contains_key(&key)
                || p.cpu_percent < self.cfg.background_cpu_percent
                || protection_reason(p.pid, &p.name).is_some()
                || OWN_IMAGES.iter().any(|n| n.eq_ignore_ascii_case(&p.name))
                || self.cfg.exempt.iter().any(|g| glob_match(g, &p.name))
            {
                continue;
            }
            let target = ProcessRef::from(p);
            for action in &self.cfg.actions {
                out.push(Decision::Apply {
                    source: Source::GameMode,
                    target: target.clone(),
                    action: *action,
                    reason: format!("game mode: {name} is in front"),
                });
            }
            self.calmed.insert(key, target);
        }
        (out, change)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    fn proc(pid: u32, name: &str, cpu: f32) -> ProcessInfo {
        ProcessInfo {
            pid,
            parent_pid: 1,
            name: name.into(),
            cpu_percent: cpu,
            cpu_time: Duration::ZERO,
            working_set: 0,
            private_working_set: 0,
            private_bytes: 0,
            thread_count: 1,
            handle_count: 1,
            session_id: 1,
            base_priority: 8,
            gpu_percent: 0.0,
            gpu_memory: 0,
            io_read_rate: 0.0,
            io_write_rate: 0.0,
            net_rx_rate: 0.0,
            net_tx_rate: 0.0,
            create_time: i64::from(pid),
        }
    }

    fn game(pids: &[u32]) -> Option<(String, HashSet<Key>)> {
        Some((
            "game.exe".into(),
            pids.iter().map(|&p| (p, i64::from(p))).collect(),
        ))
    }

    #[test]
    fn calms_busy_background_apps_while_a_game_runs_and_restores_after() {
        let procs = [
            proc(10, "game.exe", 60.0),
            proc(20, "indexer.exe", 8.0),
            proc(30, "idle.exe", 0.5),
            proc(40, "obs64.exe", 9.0),
            proc(50, "csrss.exe", 5.0),
            proc(60, "RustWTM.exe", 3.0),
        ];
        let mut g = GameMode::new(GameModeConfig::default());
        let (d, change) = g.tick(game(&[10]), &procs);
        assert_eq!(change, Some(GameChange::Started("game.exe".into())));
        let targets: HashSet<u32> = d.iter().map(|d| d.target().pid).collect();
        assert_eq!(targets, [20].into());
        assert_eq!(d.len(), 2, "priority and efficiency mode");
        assert_eq!((g.game(), g.calmed()), (Some("game.exe"), 1));

        // Steady state: nothing new.
        let (d, change) = g.tick(game(&[10]), &procs);
        assert!(d.is_empty() && change.is_none());

        let (d, change) = g.tick(None, &procs);
        assert_eq!(change, Some(GameChange::Ended("game.exe".into())));
        assert!(matches!(&d[..], [Decision::Restore { target, .. }] if target.pid == 20));
        assert_eq!(g.calmed(), 0);
    }

    #[test]
    fn processes_joining_the_game_are_handed_back_and_off_means_off() {
        let procs = [proc(10, "game.exe", 60.0), proc(20, "helper.exe", 8.0)];
        let mut g = GameMode::new(GameModeConfig::default());
        g.tick(game(&[10]), &procs);
        let (d, _) = g.tick(game(&[10, 20]), &procs);
        assert!(
            matches!(&d[..], [Decision::Restore { reason, .. }] if reason == "part of the game")
        );

        let mut off = GameMode::new(GameModeConfig {
            enabled: false,
            ..GameModeConfig::default()
        });
        let (d, change) = off.tick(game(&[10]), &procs);
        assert!(d.is_empty() && change.is_none() && off.game().is_none());
    }

    #[test]
    fn only_the_games_own_session_is_calmed() {
        let mut other_user = proc(20, "indexer.exe", 8.0);
        other_user.session_id = 2;
        let mut service = proc(30, "svc.exe", 8.0);
        service.session_id = 0;
        let procs = [
            proc(10, "game.exe", 60.0),
            other_user,
            service,
            proc(40, "sync.exe", 8.0),
        ];
        let mut g = GameMode::new(GameModeConfig::default());
        let (d, _) = g.tick(game(&[10]), &procs);
        let targets: HashSet<u32> = d.iter().map(|d| d.target().pid).collect();
        assert_eq!(targets, [40].into());

        // A "game" in session 0 calms nothing.
        let mut g = GameMode::new(GameModeConfig::default());
        let (d, _) = g.tick(game(&[30]), &procs);
        assert!(d.is_empty());
    }

    #[test]
    fn validation() {
        assert!(GameModeConfig::default().validate().is_ok());
        let bad = GameModeConfig {
            background_cpu_percent: 120.0,
            ..GameModeConfig::default()
        };
        assert!(bad.validate().is_err());
        let relative = GameModeConfig {
            actions: vec![Action::CpuCapRelative(0.5)],
            ..GameModeConfig::default()
        };
        assert!(relative.validate().is_err());
    }
}
