use std::{
    sync::Mutex,
    time::{Duration, Instant},
};

/// Tracks whether an AVFoundation capture session is still delivering frames.
/// This lives outside the platform client module so its interruption semantics
/// remain unit-testable even when `livekit_client` substitutes its mock client
/// for ordinary library tests.
pub(crate) struct CameraActivity {
    started_at: Instant,
    last_frame_at: Mutex<Option<Instant>>,
}

#[cfg_attr(test, allow(dead_code))]
impl CameraActivity {
    pub(crate) fn new() -> Self {
        Self::new_at(Instant::now())
    }

    fn new_at(started_at: Instant) -> Self {
        Self {
            started_at,
            last_frame_at: Mutex::new(None),
        }
    }

    pub(crate) fn mark_frame(&self) {
        self.mark_frame_at(Instant::now());
    }

    fn mark_frame_at(&self, now: Instant) {
        if let Ok(mut last_frame_at) = self.last_frame_at.lock() {
            *last_frame_at = Some(now);
        }
    }

    pub(crate) fn is_stalled(&self, timeout: Duration) -> bool {
        self.is_stalled_at(Instant::now(), timeout)
    }

    fn is_stalled_at(&self, now: Instant, timeout: Duration) -> bool {
        let last_frame_at = self
            .last_frame_at
            .lock()
            .ok()
            .and_then(|last_frame_at| *last_frame_at);
        now.saturating_duration_since(last_frame_at.unwrap_or(self.started_at)) >= timeout
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn distinguishes_startup_grace_live_frames_and_stalls() {
        let start = Instant::now();
        let activity = CameraActivity::new_at(start);
        let timeout = Duration::from_secs(5);

        assert!(!activity.is_stalled_at(start + Duration::from_secs(4), timeout));
        assert!(activity.is_stalled_at(start + timeout, timeout));

        activity.mark_frame_at(start + Duration::from_secs(6));
        assert!(!activity.is_stalled_at(start + Duration::from_secs(10), timeout));
        assert!(activity.is_stalled_at(start + Duration::from_secs(11), timeout));
    }
}
