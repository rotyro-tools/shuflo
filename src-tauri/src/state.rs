use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Mutex;
use std::time::Instant;

use crate::spotify::api::User;

pub struct AccessToken {
    pub value: String,
    pub expires_at: Instant,
}

pub struct AppState {
    /// Set while a shuffle or restore is writing to Spotify.
    pub running: AtomicBool,
    pub token: tokio::sync::Mutex<Option<AccessToken>>,
    /// In-memory copy of the refresh token; the keychain is the persistent copy.
    pub refresh_token: Mutex<Option<String>>,
    /// False when the OS keychain could not be used, so the login won't survive a restart.
    pub keychain_ok: AtomicBool,
    pub user: Mutex<Option<User>>,
    /// Port of the loopback listener of a login still waiting for its callback.
    pub login_port: Mutex<Option<u16>>,
}

/// Clears the `running` flag when dropped, so a failed run never leaves the lock held.
pub struct RunGuard<'a> {
    flag: &'a AtomicBool,
}

impl Drop for RunGuard<'_> {
    fn drop(&mut self) {
        self.flag.store(false, Ordering::SeqCst);
    }
}

impl AppState {
    pub fn new() -> Self {
        Self {
            running: AtomicBool::new(false),
            token: tokio::sync::Mutex::new(None),
            refresh_token: Mutex::new(None),
            keychain_ok: AtomicBool::new(true),
            user: Mutex::new(None),
            login_port: Mutex::new(None),
        }
    }

    /// Takes the run lock, or returns `None` if another run holds it.
    pub fn try_start_run(&self) -> Option<RunGuard<'_>> {
        self.running
            .compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst)
            .ok()
            .map(|_| RunGuard {
                flag: &self.running,
            })
    }

    pub fn is_running(&self) -> bool {
        self.running.load(Ordering::SeqCst)
    }
}

impl Default for AppState {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn new_state_is_idle_and_logged_out() {
        let state = AppState::new();
        assert!(!state.is_running());
        assert!(state.refresh_token.lock().unwrap().is_none());
        assert!(state.user.lock().unwrap().is_none());
        assert!(state.keychain_ok.load(Ordering::SeqCst));
    }

    #[test]
    fn run_lock_blocks_a_second_run_until_dropped() {
        let state = AppState::default();
        let guard = state.try_start_run();
        assert!(guard.is_some());
        assert!(state.is_running());
        assert!(state.try_start_run().is_none());
        drop(guard);
        assert!(!state.is_running());
        assert!(state.try_start_run().is_some());
    }
}
