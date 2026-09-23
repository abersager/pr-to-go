use std::sync::Mutex;
use std::time::{Duration, SystemTime};

/// Local clock. Only used for scheduling and "fetched at" bookkeeping; we never
/// compare it with GitHub's timestamps (DESIGN.md §7.1 rule 5).
pub trait Clock: Send + Sync {
    fn now(&self) -> SystemTime;
}

pub struct SystemClock;

impl Clock for SystemClock {
    fn now(&self) -> SystemTime {
        SystemTime::now()
    }
}

/// Manually advanced clock for tests.
pub struct TestClock(Mutex<SystemTime>);

impl TestClock {
    pub fn new() -> Self {
        TestClock(Mutex::new(
            SystemTime::UNIX_EPOCH + Duration::from_secs(1_790_000_000),
        ))
    }
    pub fn advance(&self, d: Duration) {
        *self.0.lock().unwrap() += d;
    }
}

impl Default for TestClock {
    fn default() -> Self {
        Self::new()
    }
}

impl Clock for TestClock {
    fn now(&self) -> SystemTime {
        *self.0.lock().unwrap()
    }
}

pub fn rfc3339(t: SystemTime) -> String {
    humantime::format_rfc3339_seconds(t).to_string()
}

pub fn parse_rfc3339(s: &str) -> Option<SystemTime> {
    humantime::parse_rfc3339_weak(s).ok()
}
