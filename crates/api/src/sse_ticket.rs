use std::{
    collections::HashMap,
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};

use chrono::{SecondsFormat, Utc};
use sha2::{Digest, Sha256};

use crate::routes::auth::AuthenticatedUser;

const TICKET_TTL: Duration = Duration::from_secs(30);

#[derive(Debug, Clone)]
struct TicketRecord {
    user_id: String,
    email: String,
    is_admin: bool,
    expires_at: Instant,
}

/// Short-lived, single-use credentials for the browser EventSource transport.
///
/// EventSource cannot attach an Authorization header. A ticket is scoped to one
/// authenticated user, expires after 30 seconds, and is removed on first use.
/// It is deliberately not a JWT and cannot be replayed as a general API token.
#[derive(Debug, Clone, Default)]
pub struct SseTicketStore {
    records: Arc<Mutex<HashMap<String, TicketRecord>>>,
}

impl SseTicketStore {
    pub fn issue(&self, user: &AuthenticatedUser) -> (String, String) {
        let raw = uuid::Uuid::new_v4().to_string();
        let hash = hash_ticket(&raw);
        let expires_at = Instant::now() + TICKET_TTL;
        let mut records = self.records.lock().expect("sse ticket store lock");
        sweep_expired(&mut records);
        records.insert(
            hash,
            TicketRecord {
                user_id: user.user_id.clone(),
                email: user.email.clone(),
                is_admin: user.is_admin,
                expires_at,
            },
        );
        let expires_at = Utc::now()
            + chrono::Duration::from_std(TICKET_TTL).expect("ticket ttl fits chrono duration");
        (raw, expires_at.to_rfc3339_opts(SecondsFormat::Secs, true))
    }

    pub fn consume(&self, raw: &str) -> Option<AuthenticatedUser> {
        let hash = hash_ticket(raw);
        let mut records = self.records.lock().expect("sse ticket store lock");
        sweep_expired(&mut records);
        let record = records.remove(&hash)?;
        if record.expires_at <= Instant::now() {
            return None;
        }
        Some(AuthenticatedUser {
            user_id: record.user_id,
            email: record.email,
            is_admin: record.is_admin,
        })
    }
}

fn hash_ticket(raw: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(raw.as_bytes());
    hex::encode(hasher.finalize())
}

fn sweep_expired(records: &mut HashMap<String, TicketRecord>) {
    let now = Instant::now();
    records.retain(|_, record| record.expires_at > now);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn user() -> AuthenticatedUser {
        AuthenticatedUser {
            user_id: "user-1".to_owned(),
            email: "user@example.com".to_owned(),
            is_admin: true,
        }
    }

    #[test]
    fn ticket_is_single_use_and_preserves_current_authority() {
        let store = SseTicketStore::default();
        let (ticket, _) = store.issue(&user());

        let consumed = store.consume(&ticket).expect("first use succeeds");
        assert_eq!(consumed.user_id, "user-1");
        assert_eq!(consumed.email, "user@example.com");
        assert!(consumed.is_admin);
        assert!(
            store.consume(&ticket).is_none(),
            "ticket cannot be replayed"
        );
    }

    #[test]
    fn unknown_ticket_is_rejected() {
        let store = SseTicketStore::default();
        assert!(store.consume("not-a-ticket").is_none());
    }
}
