//! Fault injection. A fault matches a request key (a GraphQL operation name
//! such as `AddReview`, or `rest:files`, `rest:compare`, `rest:blob`,
//! `rest:rate_limit`, `rest:asset`), optionally lets `skip` matching requests
//! through first, then fires `times` times.

use std::sync::Arc;
use std::time::Duration;

use crate::model::World;

#[derive(Clone)]
pub enum FaultAction {
    /// Respond with this status without applying the request.
    Status(u16),
    /// Apply the request, then respond with this status. For a mutation the
    /// client can't tell whether it was applied.
    ApplyThenStatus(u16),
    /// Apply the request, then stall past the client's timeout.
    ApplyThenDelay(Duration),
    /// Stall before handling the request.
    Delay(Duration),
    /// Answer with an HTML login page, as a captive portal does.
    CaptivePortal,
    /// 403 secondary rate limit, with or without a Retry-After header.
    SecondaryRateLimit { retry_after: Option<u64> },
    /// Change the world before handling the request (for example push to
    /// the PR in the middle of a submission).
    Hook(Arc<dyn Fn(&mut World) + Send + Sync>),
}

impl std::fmt::Debug for FaultAction {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            FaultAction::Status(s) => write!(f, "Status({s})"),
            FaultAction::ApplyThenStatus(s) => write!(f, "ApplyThenStatus({s})"),
            FaultAction::ApplyThenDelay(d) => write!(f, "ApplyThenDelay({d:?})"),
            FaultAction::Delay(d) => write!(f, "Delay({d:?})"),
            FaultAction::CaptivePortal => write!(f, "CaptivePortal"),
            FaultAction::SecondaryRateLimit { retry_after } => {
                write!(f, "SecondaryRateLimit({retry_after:?})")
            }
            FaultAction::Hook(_) => write!(f, "Hook"),
        }
    }
}

#[derive(Debug)]
pub struct Fault {
    pub key: String,
    pub skip: usize,
    pub times: usize,
    pub action: FaultAction,
}

impl Fault {
    pub fn matches(&self, key: &str) -> bool {
        match self.key.strip_suffix('*') {
            Some(prefix) => key.starts_with(prefix),
            None => self.key == key,
        }
    }
}

impl World {
    /// Fires `action` on the next request matching `key`.
    pub fn fault(&mut self, key: &str, action: FaultAction) {
        self.fault_after(key, 0, 1, action);
    }

    /// Lets `skip` matching requests through, then fires `times` times.
    pub fn fault_after(&mut self, key: &str, skip: usize, times: usize, action: FaultAction) {
        self.faults.push(Fault { key: key.into(), skip, times, action });
    }

    /// Runs `f` against the world right before the next request matching `key`.
    pub fn hook(&mut self, key: &str, f: impl Fn(&mut World) + Send + Sync + 'static) {
        self.fault(key, FaultAction::Hook(Arc::new(f)));
    }
}
