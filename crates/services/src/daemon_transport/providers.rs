use async_trait::async_trait;

use crate::ServiceError;

#[async_trait]
pub trait FilesystemProvider: Send + Sync {
    async fn list(
        &self,
        params: api_types::FsListParams,
    ) -> Result<api_types::FsListResult, ServiceError>;

    async fn branches(
        &self,
        params: api_types::FsBranchesParams,
    ) -> Result<api_types::FsBranchesResult, ServiceError>;
}

#[async_trait]
pub trait ExecutionProvider: Send + Sync {
    /// Whether this provider resolves credential identity from the frozen
    /// execution snapshot inside the same process before invoking a harness.
    /// Remote providers remain false until the daemon protocol has an exact,
    /// non-ambient credential contract.
    fn accepts_snapshot_credentials(&self) -> bool {
        false
    }

    async fn start(
        &self,
        params: api_types::ExecutionStartParams,
    ) -> Result<api_types::ExecutionStartResult, ServiceError>;

    async fn cancel(
        &self,
        params: api_types::ExecutionCancelParams,
    ) -> Result<api_types::ExecutionCancelResult, ServiceError>;
}
