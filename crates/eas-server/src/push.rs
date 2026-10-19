//! Push hub: one JMAP EventSource per user, shared by every waiting `Ping`
//! and `Sync` of that user's devices.
//!
//! A [`Feed`] is a counter that goes up whenever Stalwart reports a mail
//! change (or the stream reconnected and may have missed one). Waiters
//! recheck their folders when it moves; they never trust it to say which
//! folder changed. The stream task lives as long as some request holds the
//! feed and is aborted when the last one lets go.

use eas_jmap::push::PushEvent;
use eas_jmap::{JmapClient, JmapError};
use std::collections::HashMap;
use std::sync::{Arc, Mutex, Weak};
use std::time::Duration;
use tokio::sync::{watch, Notify};
use tokio::task::AbortHandle;

/// Keep-alive interval asked of Stalwart.
const PING: Duration = Duration::from_secs(60);
/// Reconnect at least this often, so a silently dead connection can't
/// stall push for longer.
const MAX_AGE: Duration = Duration::from_secs(30 * 60);
const BACKOFF_MAX: Duration = Duration::from_secs(300);

pub struct Feed {
    rx: watch::Receiver<u64>,
    task: AbortHandle,
}

impl Feed {
    pub fn receiver(&self) -> watch::Receiver<u64> {
        let mut rx = self.rx.clone();
        rx.borrow_and_update();
        rx
    }
}

impl Drop for Feed {
    fn drop(&mut self) {
        self.task.abort();
    }
}

#[derive(Default)]
pub struct PushHub {
    feeds: Mutex<HashMap<String, Weak<Feed>>>,
}

impl PushHub {
    /// The user's feed, starting its stream if no request holds one.
    pub fn subscribe(&self, jmap: &JmapClient) -> Arc<Feed> {
        let user = jmap.session().username.clone();
        let mut feeds = self.feeds.lock().unwrap_or_else(|e| e.into_inner());
        // A feed whose task ended (credentials rejected) is replaced, so the
        // next request with good credentials gets push again.
        if let Some(f) = feeds.get(&user).and_then(Weak::upgrade) {
            if !f.task.is_finished() {
                return f;
            }
        }
        if feeds.len() > 1_000 {
            feeds.retain(|_, w| w.strong_count() > 0);
        }
        let (tx, rx) = watch::channel(0u64);
        let task = tokio::spawn(run(jmap.clone(), tx)).abort_handle();
        let feed = Arc::new(Feed { rx, task });
        feeds.insert(user, Arc::downgrade(&feed));
        feed
    }
}

fn relevant(changed: &HashMap<String, HashMap<String, String>>, account: &str) -> bool {
    changed
        .get(account)
        .is_some_and(|t| t.contains_key("Email") || t.contains_key("Mailbox"))
}

async fn run(jmap: JmapClient, tx: watch::Sender<u64>) {
    let account = jmap.account().to_string();
    let user = jmap.session().username.clone();
    let mut backoff = Duration::from_secs(5);
    loop {
        match jmap.event_source(PING, MAX_AGE).await {
            Ok(mut es) => {
                backoff = Duration::from_secs(5);
                tracing::debug!(%user, "push stream open");
                loop {
                    match es.next().await {
                        Ok(Some(PushEvent::State(changed))) if relevant(&changed, &account) => {
                            tx.send_modify(|n| *n += 1);
                        }
                        Ok(Some(_)) => {}
                        Ok(None) => break,
                        Err(e) => {
                            tracing::debug!(%user, error = %e, "push stream ended");
                            break;
                        }
                    }
                }
            }
            Err(JmapError::Unauthorized) => {
                // Password changed: wake everyone so their next JMAP call
                // fails with 401 and the device asks for new credentials.
                tracing::info!(%user, "push stream unauthorized");
                tx.send_modify(|n| *n += 1);
                return;
            }
            Err(e) => {
                tracing::warn!(%user, error = %e, "push stream failed");
            }
        }
        // Changes may have been missed while disconnected.
        tx.send_modify(|n| *n += 1);
        tokio::time::sleep(backoff).await;
        backoff = (backoff * 2).min(BACKOFF_MAX);
    }
}

/// One slot per device: a new `Ping` ends the one still waiting, since the
/// device has given up on it.
#[derive(Default)]
pub struct PingSlots {
    map: Mutex<HashMap<i64, Weak<Notify>>>,
}

impl PingSlots {
    /// Ends any waiting `Ping` of `device` and returns the slot for the new
    /// one.
    pub fn take(&self, device: i64) -> Arc<Notify> {
        let mut map = self.map.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(old) = map.get(&device).and_then(Weak::upgrade) {
            old.notify_waiters();
            return old;
        }
        if map.len() > 1_000 {
            map.retain(|_, w| w.strong_count() > 0);
        }
        let n = Arc::new(Notify::new());
        map.insert(device, Arc::downgrade(&n));
        n
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_mail_changes_of_the_account_count() {
        let mut changed = HashMap::new();
        changed.insert(
            "a".to_string(),
            HashMap::from([("CalendarEvent".to_string(), "1".to_string())]),
        );
        assert!(!relevant(&changed, "a"));
        changed
            .get_mut("a")
            .unwrap()
            .insert("Email".into(), "2".into());
        assert!(relevant(&changed, "a"));
        assert!(!relevant(&changed, "b"));
    }

    #[tokio::test]
    async fn a_new_ping_ends_the_old_one() {
        let slots = PingSlots::default();
        let first = slots.take(1);
        let waiting = first.notified();
        tokio::pin!(waiting);
        waiting.as_mut().enable();
        let _second = slots.take(1);
        tokio::time::timeout(Duration::from_secs(1), waiting)
            .await
            .expect("first ping woken");
    }
}
