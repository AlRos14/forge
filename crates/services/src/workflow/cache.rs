use std::{collections::HashMap, future::Future, sync::Arc};

use api_types::WorkflowDefinition;
use db::Task;
use tokio::sync::RwLock;

use crate::workflow::engine::WorkflowEngine;

const INHERITED_SUBTASK_CACHE_KEY: &str = "__inherited_subtask__";

pub struct WorkflowCache {
    entries: RwLock<HashMap<String, Arc<WorkflowDefinition>>>,
}

impl WorkflowCache {
    pub fn new() -> Self {
        Self {
            entries: RwLock::new(HashMap::new()),
        }
    }

    pub async fn get_or_load<F, Fut>(
        &self,
        project_id: &str,
        load: F,
    ) -> crate::Result<Arc<WorkflowDefinition>>
    where
        F: FnOnce() -> Fut,
        Fut: Future<Output = crate::Result<String>>,
    {
        if let Some(workflow) = self.entries.read().await.get(project_id) {
            return Ok(Arc::clone(workflow));
        }

        let workflow_definition = load().await?;
        let workflow = Arc::new(WorkflowEngine::resolve_workflow(&workflow_definition));
        self.entries
            .write()
            .await
            .insert(project_id.to_owned(), Arc::clone(&workflow));

        Ok(workflow)
    }

    pub async fn get_or_load_for_task<F, Fut>(
        &self,
        task: &Task,
        load: F,
    ) -> crate::Result<Arc<WorkflowDefinition>>
    where
        F: FnOnce() -> Fut,
        Fut: Future<Output = crate::Result<String>>,
    {
        if task.parent_task_id.is_none() {
            return self.get_or_load(&task.project_id, load).await;
        }

        if let Some(workflow) = self.entries.read().await.get(INHERITED_SUBTASK_CACHE_KEY) {
            return Ok(Arc::clone(workflow));
        }

        let workflow = Arc::new(WorkflowEngine::resolve_subtask_workflow());
        self.entries
            .write()
            .await
            .insert(INHERITED_SUBTASK_CACHE_KEY.to_owned(), Arc::clone(&workflow));

        Ok(workflow)
    }

    pub async fn invalidate(&self, project_id: &str) {
        self.entries.write().await.remove(project_id);
    }

    pub async fn clear(&self) {
        self.entries.write().await.clear();
    }
}

#[cfg(test)]
mod tests {
    use std::sync::{
        atomic::{AtomicUsize, Ordering},
        Arc,
    };

    use super::WorkflowCache;

    #[tokio::test]
    async fn get_or_load_calls_loader_once_for_first_load() {
        let cache = WorkflowCache::new();
        let calls = Arc::new(AtomicUsize::new(0));
        let loader_calls = Arc::clone(&calls);

        cache
            .get_or_load("project-1", || async move {
                loader_calls.fetch_add(1, Ordering::SeqCst);
                Ok("{}".to_owned())
            })
            .await
            .unwrap();

        assert_eq!(calls.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn get_or_load_reuses_cached_entry_for_same_project() {
        let cache = WorkflowCache::new();
        let calls = Arc::new(AtomicUsize::new(0));
        let loader_calls = Arc::clone(&calls);

        let first = cache
            .get_or_load("project-1", || async move {
                loader_calls.fetch_add(1, Ordering::SeqCst);
                Ok("{}".to_owned())
            })
            .await
            .unwrap();

        let loader_calls = Arc::clone(&calls);
        let second = cache
            .get_or_load("project-1", || async move {
                loader_calls.fetch_add(1, Ordering::SeqCst);
                Ok("{}".to_owned())
            })
            .await
            .unwrap();

        assert_eq!(calls.load(Ordering::SeqCst), 1);
        assert!(Arc::ptr_eq(&first, &second));
    }

    #[tokio::test]
    async fn get_or_load_calls_loader_again_after_invalidate() {
        let cache = WorkflowCache::new();
        let calls = Arc::new(AtomicUsize::new(0));
        let loader_calls = Arc::clone(&calls);

        cache
            .get_or_load("project-1", || async move {
                loader_calls.fetch_add(1, Ordering::SeqCst);
                Ok("{}".to_owned())
            })
            .await
            .unwrap();

        cache.invalidate("project-1").await;

        let loader_calls = Arc::clone(&calls);
        cache
            .get_or_load("project-1", || async move {
                loader_calls.fetch_add(1, Ordering::SeqCst);
                Ok("{}".to_owned())
            })
            .await
            .unwrap();

        assert_eq!(calls.load(Ordering::SeqCst), 2);
    }
}
