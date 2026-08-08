use std::sync::{Condvar, Mutex, MutexGuard};
use std::time::{Duration, Instant};

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum PlaybackOutcome {
    Completed,
    Stopped,
    Failed(String),
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct PlaybackSnapshot {
    pub(crate) playing: bool,
    pub(crate) paused: bool,
    pub(crate) stopping: bool,
}

#[derive(Debug, Default)]
struct PlaybackState {
    playing: bool,
    paused: bool,
    stop_requested: bool,
    outcome: Option<PlaybackOutcome>,
}

/// Coordinates playback state changes with the playback worker.
///
/// The worker remains `playing` while a stop is being acknowledged, so a new
/// session cannot accidentally revive the old worker. The condition variable
/// also makes pause, resume, and stop responsive without a polling sleep.
#[derive(Debug, Default)]
pub(crate) struct PlaybackControl {
    state: Mutex<PlaybackState>,
    changed: Condvar,
}

impl PlaybackControl {
    pub(crate) fn new() -> Self {
        Self::default()
    }

    fn lock_state(&self) -> MutexGuard<'_, PlaybackState> {
        self.state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    pub(crate) fn snapshot(&self) -> PlaybackSnapshot {
        let state = self.lock_state();
        PlaybackSnapshot {
            playing: state.playing,
            paused: state.paused,
            stopping: state.stop_requested,
        }
    }

    pub(crate) fn begin(&self) -> bool {
        let mut state = self.lock_state();
        if state.playing {
            return false;
        }

        state.playing = true;
        state.paused = false;
        state.stop_requested = false;
        state.outcome = None;
        self.changed.notify_all();
        true
    }

    pub(crate) fn request_pause(&self) -> bool {
        let mut state = self.lock_state();
        if !state.playing || state.paused || state.stop_requested {
            return false;
        }

        state.paused = true;
        self.changed.notify_all();
        true
    }

    pub(crate) fn request_resume(&self) -> bool {
        let mut state = self.lock_state();
        if !state.playing || !state.paused || state.stop_requested {
            return false;
        }

        state.paused = false;
        self.changed.notify_all();
        true
    }

    pub(crate) fn request_stop(&self) -> bool {
        let mut state = self.lock_state();
        if !state.playing || state.stop_requested {
            return false;
        }

        state.stop_requested = true;
        state.paused = false;
        self.changed.notify_all();
        true
    }

    pub(crate) fn finish(&self, outcome: PlaybackOutcome) {
        let mut state = self.lock_state();
        state.playing = false;
        state.paused = false;
        state.stop_requested = false;
        state.outcome = Some(outcome);
        self.changed.notify_all();
    }

    pub(crate) fn take_outcome(&self) -> Option<PlaybackOutcome> {
        let mut state = self.lock_state();
        if state.playing {
            None
        } else {
            state.outcome.take()
        }
    }

    /// Waits for playback time while excluding time spent paused.
    ///
    /// Pause is checked even for a zero-length delay, which is essential for
    /// recordings containing multiple events with the same timestamp.
    pub(crate) fn wait_for_delay(&self, delay: Duration) -> bool {
        let mut remaining = delay;
        let mut state = self.lock_state();

        loop {
            while state.playing && state.paused && !state.stop_requested {
                state = self
                    .changed
                    .wait(state)
                    .unwrap_or_else(|poisoned| poisoned.into_inner());
            }

            if !state.playing || state.stop_requested {
                return false;
            }
            if remaining.is_zero() {
                return true;
            }

            let started = Instant::now();
            let (next_state, _) = self
                .changed
                .wait_timeout(state, remaining)
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            state = next_state;
            remaining = remaining.saturating_sub(started.elapsed());
        }
    }

    /// Runs one input injection while holding the state gate.
    ///
    /// A pause or stop that wins the gate runs first; otherwise this one event
    /// is considered already in progress and all later events are held back.
    pub(crate) fn run_when_ready<T>(&self, action: impl FnOnce() -> T) -> Option<T> {
        let mut state = self.lock_state();
        while state.playing && state.paused && !state.stop_requested {
            state = self
                .changed
                .wait(state)
                .unwrap_or_else(|poisoned| poisoned.into_inner());
        }

        if !state.playing || state.stop_requested {
            return None;
        }

        Some(action())
    }
}

#[cfg(test)]
mod tests {
    use super::{PlaybackControl, PlaybackOutcome};
    use std::sync::{mpsc, Arc};
    use std::thread;
    use std::time::Duration;

    #[test]
    fn pause_blocks_zero_delay_events_until_resume() {
        let control = Arc::new(PlaybackControl::new());
        assert!(control.begin());
        assert!(control.request_pause());

        let worker_control = Arc::clone(&control);
        let (sent, received) = mpsc::channel();
        let worker = thread::spawn(move || {
            assert!(worker_control.wait_for_delay(Duration::ZERO));
            assert!(worker_control
                .run_when_ready(|| sent.send(()).expect("receiver should exist"))
                .is_some());
            worker_control.finish(PlaybackOutcome::Completed);
        });

        assert!(received.recv_timeout(Duration::from_millis(40)).is_err());
        assert!(control.request_resume());
        received
            .recv_timeout(Duration::from_secs(1))
            .expect("event should run after resume");
        worker.join().expect("worker should finish");
        assert_eq!(control.take_outcome(), Some(PlaybackOutcome::Completed));
    }

    #[test]
    fn stop_wakes_a_paused_worker() {
        let control = Arc::new(PlaybackControl::new());
        assert!(control.begin());
        assert!(control.request_pause());

        let worker_control = Arc::clone(&control);
        let (sent, received) = mpsc::channel();
        let worker = thread::spawn(move || {
            sent.send(worker_control.wait_for_delay(Duration::from_secs(10)))
                .expect("receiver should exist");
            worker_control.finish(PlaybackOutcome::Stopped);
        });

        assert!(control.request_stop());
        assert!(!received
            .recv_timeout(Duration::from_secs(1))
            .expect("stop should wake the worker"));
        worker.join().expect("worker should finish");
        assert_eq!(control.take_outcome(), Some(PlaybackOutcome::Stopped));
    }

    #[test]
    fn a_stopping_worker_cannot_be_replaced() {
        let control = PlaybackControl::new();
        assert!(control.begin());
        assert!(control.request_stop());
        assert!(!control.begin());

        control.finish(PlaybackOutcome::Stopped);
        assert!(control.begin());
    }
}
