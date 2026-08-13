//! Pure finite-state rules shared by Agent Chat admission and the worker.
//!
//! Persistence is deliberately left to the DB repositories. Keeping retry and
//! lease decisions deterministic here prevents a worker restart or a failed
//! failure-write from inventing an unbounded retry loop.

use api_types::AgentChatTurnStatus;
use chrono::{DateTime, Duration, Utc};

const MAX_BACKOFF_SECONDS: i64 = 300;
const ERROR_LIMIT: usize = 512;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LeaseDecision {
    pub status: AgentChatTurnStatus,
    pub lease_owner: Option<String>,
    pub lease_expires_at: Option<DateTime<Utc>>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FailureDecision {
    pub status: AgentChatTurnStatus,
    pub attempt_count: i64,
    pub next_attempt_at: Option<DateTime<Utc>>,
    pub error: String,
}

/// Claim is valid only for a queued or retry-wait job whose cooldown elapsed.
pub fn claim(
    status: AgentChatTurnStatus,
    attempt_count: i64,
    max_attempts: i64,
    now: DateTime<Utc>,
    lease_until: DateTime<Utc>,
    owner: &str,
) -> Option<LeaseDecision> {
    if owner.trim().is_empty() || max_attempts <= 0 || attempt_count >= max_attempts {
        return None;
    }
    if !matches!(
        status,
        AgentChatTurnStatus::Queued | AgentChatTurnStatus::RetryWait
    ) {
        return None;
    }
    if lease_until <= now {
        return None;
    }
    Some(LeaseDecision {
        status: AgentChatTurnStatus::Leased,
        lease_owner: Some(owner.to_owned()),
        lease_expires_at: Some(lease_until),
    })
}

/// A failed invocation consumes one attempt. Once the stored budget is
/// exhausted the status is terminal and no future wake is produced.
pub fn failure(
    attempt_count: i64,
    max_attempts: i64,
    now: DateTime<Utc>,
    error: &str,
) -> FailureDecision {
    let attempt_count = attempt_count.saturating_add(1).max(1);
    let error = bounded_error(error);
    if max_attempts <= 0 || attempt_count >= max_attempts {
        return FailureDecision {
            status: AgentChatTurnStatus::Failed,
            attempt_count,
            next_attempt_at: None,
            error,
        };
    }
    let exponent = attempt_count.saturating_sub(1).min(8) as u32;
    let seconds = 5_i64
        .saturating_mul(1_i64 << exponent)
        .min(MAX_BACKOFF_SECONDS);
    FailureDecision {
        status: AgentChatTurnStatus::RetryWait,
        attempt_count,
        next_attempt_at: Some(now + Duration::seconds(seconds)),
        error,
    }
}

/// Record a failure after the dispatcher has already consumed an attempt at
/// claim time. This keeps a backend error or lease expiry from charging the
/// same invocation twice.
pub fn failure_after_claim(
    attempt_count: i64,
    max_attempts: i64,
    now: DateTime<Utc>,
    error: &str,
) -> FailureDecision {
    failure(attempt_count.saturating_sub(1), max_attempts, now, error)
}

/// Expired leases become retryable/terminal using the same rule as an
/// invocation failure; no model call is needed to recover a stale lease.
pub fn recover_expired(
    status: AgentChatTurnStatus,
    lease_expires_at: Option<DateTime<Utc>>,
    attempt_count: i64,
    max_attempts: i64,
    now: DateTime<Utc>,
) -> Option<FailureDecision> {
    if status != AgentChatTurnStatus::Leased || lease_expires_at.is_none_or(|until| until > now) {
        return None;
    }
    Some(failure_after_claim(
        attempt_count,
        max_attempts,
        now,
        "turn execution lease expired",
    ))
}

pub fn bounded_error(error: &str) -> String {
    error
        .chars()
        .filter(|character| !matches!(character, '\r' | '\n' | '\0'))
        .take(ERROR_LIMIT)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(seconds: i64) -> DateTime<Utc> {
        DateTime::from_timestamp(seconds, 0).expect("valid timestamp")
    }

    #[test]
    fn claim_accepts_only_live_queued_or_retry_jobs() {
        let now = at(100);
        let lease_until = at(220);
        assert_eq!(
            claim(
                AgentChatTurnStatus::Queued,
                0,
                3,
                now,
                lease_until,
                "worker-1"
            )
            .expect("queued claim")
            .status,
            AgentChatTurnStatus::Leased
        );
        assert!(claim(
            AgentChatTurnStatus::Succeeded,
            0,
            3,
            now,
            lease_until,
            "worker-1"
        )
        .is_none());
        assert!(claim(
            AgentChatTurnStatus::Queued,
            3,
            3,
            now,
            lease_until,
            "worker-1"
        )
        .is_none());
    }

    #[test]
    fn failures_use_finite_exponential_backoff_then_terminal_state() {
        let first = failure(0, 3, at(100), "temporary\nprovider failure");
        assert_eq!(first.status, AgentChatTurnStatus::RetryWait);
        assert_eq!(first.attempt_count, 1);
        assert_eq!(first.next_attempt_at, Some(at(105)));
        assert_eq!(first.error, "temporaryprovider failure");

        let second = failure(first.attempt_count, 3, at(105), "again");
        assert_eq!(second.next_attempt_at, Some(at(115)));
        let terminal = failure(second.attempt_count, 3, at(115), "last");
        assert_eq!(terminal.status, AgentChatTurnStatus::Failed);
        assert_eq!(terminal.attempt_count, 3);
        assert!(terminal.next_attempt_at.is_none());
    }

    #[test]
    fn failure_after_claim_does_not_charge_attempt_twice() {
        let decision = failure_after_claim(1, 3, at(100), "backend");
        assert_eq!(decision.status, AgentChatTurnStatus::RetryWait);
        assert_eq!(decision.attempt_count, 1);
        assert_eq!(decision.next_attempt_at, Some(at(105)));

        let terminal = failure_after_claim(3, 3, at(100), "backend");
        assert_eq!(terminal.status, AgentChatTurnStatus::Failed);
        assert_eq!(terminal.attempt_count, 3);
        assert!(terminal.next_attempt_at.is_none());
    }

    #[test]
    fn expired_lease_recovery_is_deterministic_and_does_not_reinvoke_model() {
        let recovered = recover_expired(AgentChatTurnStatus::Leased, Some(at(99)), 2, 3, at(100))
            .expect("expired lease is recoverable");
        assert_eq!(recovered.status, AgentChatTurnStatus::RetryWait);
        assert_eq!(recovered.attempt_count, 2);
        assert_eq!(recovered.next_attempt_at, Some(at(110)));
        assert!(
            recover_expired(AgentChatTurnStatus::Leased, Some(at(101)), 0, 3, at(100),).is_none()
        );
    }

    #[test]
    fn bounded_error_is_safe_for_visible_turn_state() {
        let error = bounded_error(&format!("{}\nsecret", "x".repeat(600)));
        assert_eq!(error.len(), 512);
        assert!(!error.contains('\n'));
    }
}
