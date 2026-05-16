use std::{collections::HashMap, sync::Arc};

use tokio::sync::{Mutex, OwnedMutexGuard};

#[derive(Debug, Clone)]
pub struct RepoCacheLockManager {
    locks: Arc<Mutex<HashMap<String, Arc<Mutex<()>>>>>,
}

impl RepoCacheLockManager {
    pub fn new() -> Self {
        Self {
            locks: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    pub async fn acquire(&self, repo_id: &str) -> OwnedMutexGuard<()> {
        let entry = {
            let mut locks = self.locks.lock().await;
            Arc::clone(
                locks
                    .entry(repo_id.to_owned())
                    .or_insert_with(|| Arc::new(Mutex::new(()))),
            )
        };

        Arc::clone(&entry).lock_owned().await
    }
}

impl Default for RepoCacheLockManager {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::{sync::oneshot, task, time};

    #[tokio::test]
    async fn same_repo_serializes() {
        let locks = RepoCacheLockManager::new();
        let first_guard = locks.acquire("repo-a").await;
        let second_locks = locks.clone();
        let (completed_tx, mut completed_rx) = oneshot::channel();

        let second = task::spawn(async move {
            let _guard = second_locks.acquire("repo-a").await;
            let _ = completed_tx.send(());
        });

        task::yield_now().await;
        assert!(matches!(
            completed_rx.try_recv(),
            Err(oneshot::error::TryRecvError::Empty)
        ));

        drop(first_guard);
        time::timeout(std::time::Duration::from_millis(100), completed_rx)
            .await
            .expect("second acquire should complete")
            .expect("completion signal should send");
        second.await.expect("second task should finish");
    }

    #[tokio::test]
    async fn different_repos_do_not_contend() {
        let locks = RepoCacheLockManager::new();

        time::timeout(std::time::Duration::from_millis(10), async {
            let (_first, _second) = tokio::join!(locks.acquire("repo-a"), locks.acquire("repo-b"));
        })
        .await
        .expect("different repos should acquire concurrently");
    }
}
