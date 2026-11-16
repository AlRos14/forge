//! Read-only convergence watcher for paused same-PR reworks (plan node P3,
//! «Congelado 11» 11.6).
//!
//! For every `open` `pr_rework_supersession` it reads the PR status (`get_pr_status`
//! only; the watcher never mutates the provider) and hands it to
//! [`PrReworkService::observe_remote`]:
//! - merged remotely: the pause records `remote_merged` and the convergence
//!   Decision is raised (idempotent by its exact source);
//! - closed without merge: `remote_closed` (the next admission creates a new PR);
//! - open, even with a different head: nothing (the head mismatch is handled
//!   by the re-admission, 11.5).
//!
//! Once the Decision is answered, RT's durable decision scanner calls
//! [`PrReworkService::converge_remote_merged`] and cancels unstarted rework.
//! Keeping that effect in RT avoids two scanners racing to write the same
//! lifecycle receipt.

use std::sync::Arc;

use db::{PrMetadataRepo, PrReworkRepo, PrReworkState, SqliteDb, TaskIntegrationOperationRepo};
use events::EventBus;

use crate::credential_service::CredentialService;
use crate::pr_github_client::{DefaultPrProviderFactory, PrProviderFactory};
use crate::pr_rework::{ObserveOutcome, PrReworkService};
use crate::pr_service::{provider_for_admission, PrProvider};
use crate::Result;

const WATCH_BATCH: i64 = 200;

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct PrReworkWatchStats {
    pub observed: usize,
    pub remote_merged: usize,
    pub remote_closed: usize,
}

#[derive(Clone)]
pub struct PrReworkWatcher {
    db: Arc<SqliteDb>,
    rework: PrReworkService,
    credentials: Option<Arc<CredentialService>>,
    provider_factory: Arc<dyn PrProviderFactory>,
    #[cfg(test)]
    provider_override: Option<Arc<dyn PrProvider>>,
}

impl PrReworkWatcher {
    pub fn new(db: Arc<SqliteDb>, event_bus: Arc<EventBus>) -> Self {
        Self {
            rework: PrReworkService::new(Arc::clone(&db), event_bus),
            db,
            credentials: None,
            provider_factory: Arc::new(DefaultPrProviderFactory),
            #[cfg(test)]
            provider_override: None,
        }
    }

    pub fn with_credentials(mut self, credentials: Arc<CredentialService>) -> Self {
        self.credentials = Some(credentials);
        self
    }

    pub fn with_provider_factory(mut self, factory: Arc<dyn PrProviderFactory>) -> Self {
        self.provider_factory = factory;
        self
    }

    #[cfg(test)]
    pub(crate) fn with_provider_for_test(mut self, provider: Arc<dyn PrProvider>) -> Self {
        self.provider_override = Some(provider);
        self
    }

    pub async fn watch_once(&self) -> Result<PrReworkWatchStats> {
        let mut stats = PrReworkWatchStats::default();
        let open =
            PrReworkRepo::list_supersessions_in_state(&*self.db, PrReworkState::Open, WATCH_BATCH)
                .await?;
        for supersession in open {
            match self
                .observe(&supersession.id, &supersession.task_merge_operation_id)
                .await
            {
                Ok(Some(ObserveOutcome::RemoteMerged { .. })) => {
                    stats.observed += 1;
                    stats.remote_merged += 1;
                }
                Ok(Some(ObserveOutcome::RemoteClosed)) => {
                    stats.observed += 1;
                    stats.remote_closed += 1;
                }
                Ok(Some(ObserveOutcome::Unchanged)) => stats.observed += 1,
                Ok(None) => {}
                Err(error) => {
                    tracing::warn!(%error, "PR rework observation failed");
                }
            }
        }
        // RT's answered-decision scanner owns convergence. Calling it here as
        // well could race WorkUnit cancellation and the lifecycle receipt.
        Ok(stats)
    }

    /// `Ok(None)` when the provider could not be read (nothing is decided).
    async fn observe(
        &self,
        supersession_id: &str,
        task_merge_operation_id: &str,
    ) -> Result<Option<ObserveOutcome>> {
        let Some(admission) = TaskIntegrationOperationRepo::get_remote_pr_admission(
            &*self.db,
            task_merge_operation_id,
        )
        .await?
        else {
            return Ok(None);
        };
        let Some(metadata) = PrMetadataRepo::get_by_task_id(&*self.db, &admission.task_id).await?
        else {
            return Ok(None);
        };
        let provider = match self.provider(&admission).await {
            Ok(provider) => provider,
            Err(error) => {
                tracing::debug!(%error, "PR rework watcher has no usable provider");
                return Ok(None);
            }
        };
        let status = match provider.get_pr_status(&admission, &metadata).await {
            Ok(status) => status,
            Err(error) => {
                tracing::debug!(%error, "PR rework watcher could not read the PR");
                return Ok(None);
            }
        };
        Ok(Some(
            self.rework.observe_remote(supersession_id, &status).await?,
        ))
    }

    async fn provider(
        &self,
        admission: &db::RemotePrAdmission,
    ) -> std::result::Result<Arc<dyn PrProvider>, crate::pr_service::PrProviderError> {
        #[cfg(test)]
        if let Some(provider) = &self.provider_override {
            return Ok(Arc::clone(provider));
        }
        provider_for_admission(
            &self.db,
            self.credentials.as_deref(),
            self.provider_factory.as_ref(),
            admission,
        )
        .await
    }
}

#[cfg(test)]
mod tests;
