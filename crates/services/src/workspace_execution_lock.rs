use std::{collections::HashMap, sync::Arc};

use tokio::sync::{Mutex, OwnedMutexGuard};

#[derive(Debug, Clone)]
pub struct WorkspaceExecutionLockManager {
    locks: Arc<Mutex<HashMap<String, Arc<Mutex<()>>>>>,
}

impl WorkspaceExecutionLockManager {
    pub fn new() -> Self {
        Self {
            locks: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    pub async fn acquire(&self, workspace_id: &str) -> OwnedMutexGuard<()> {
        let entry = {
            let mut locks = self.locks.lock().await;
            Arc::clone(
                locks
                    .entry(workspace_id.to_owned())
                    .or_insert_with(|| Arc::new(Mutex::new(()))),
            )
        };

        Arc::clone(&entry).lock_owned().await
    }

    pub async fn try_acquire_async(&self, workspace_id: &str) -> Option<OwnedMutexGuard<()>> {
        let entry = {
            let mut locks = self.locks.lock().await;
            Arc::clone(
                locks
                    .entry(workspace_id.to_owned())
                    .or_insert_with(|| Arc::new(Mutex::new(()))),
            )
        };

        entry.try_lock_owned().ok()
    }

    pub fn try_acquire(&self, workspace_id: &str) -> Option<OwnedMutexGuard<()>> {
        let entry = {
            let mut locks = self.locks.try_lock().ok()?;
            Arc::clone(
                locks
                    .entry(workspace_id.to_owned())
                    .or_insert_with(|| Arc::new(Mutex::new(()))),
            )
        };

        entry.try_lock_owned().ok()
    }
}

impl Default for WorkspaceExecutionLockManager {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::{sync::oneshot, task, time};

    #[tokio::test]
    async fn same_workspace_serializes() {
        let locks = WorkspaceExecutionLockManager::new();
        let first_guard = locks.acquire("workspace-a").await;
        let second_locks = locks.clone();
        let (completed_tx, mut completed_rx) = oneshot::channel();

        let second = task::spawn(async move {
            let _guard = second_locks.acquire("workspace-a").await;
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
    async fn different_workspaces_do_not_contend() {
        let locks = WorkspaceExecutionLockManager::new();

        time::timeout(std::time::Duration::from_millis(10), async {
            let (_first, _second) =
                tokio::join!(locks.acquire("workspace-a"), locks.acquire("workspace-b"));
        })
        .await
        .expect("different workspaces should acquire concurrently");
    }

    #[tokio::test]
    async fn try_acquire_reports_lock_state() {
        let locks = WorkspaceExecutionLockManager::new();
        let first_guard = locks
            .try_acquire("workspace-a")
            .expect("first try acquire should succeed");

        assert!(locks.try_acquire("workspace-a").is_none());
        assert!(locks.try_acquire("workspace-b").is_some());

        drop(first_guard);
        assert!(locks.try_acquire("workspace-a").is_some());
    }
}
