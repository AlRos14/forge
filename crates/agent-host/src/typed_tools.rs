//! Scope-derived tools for Forge-hosted native Agent Runtime sessions.
//!
//! The runtime deliberately knows nothing about Forge identities, Projects,
//! Agent Chats, or Task roles. This module is the narrow host-owned composition
//! boundary: it turns one server-authorized canonical scope into an exact tool
//! registry and an authoritative security check.  Callers never provide the
//! actor or scope as tool arguments; those values are captured when the host
//! composes the tools.

use std::{
    collections::BTreeSet,
    fmt,
    path::{Component, Path},
    process::Stdio,
    sync::Arc,
};

use agent_runtime::core::{
    cancel::Cancellation,
    grant::{
        GrantConstraints, SecurityCheck, SecurityCheckId, SecurityCheckMode, SecurityCheckOutcome,
        SecurityCheckRevision,
    },
    prelude::{
        ActionClass, AuthorizationRequest, DecisionCode, InvocationContext, PermissionSet,
        PreparationContext, PreparedToolCall, RuntimeError, SecurityResource, Tool,
        ToolCallDisplay, ToolEffects, ToolOutcome, ToolSpec,
    },
    workspace::Workspace,
};
use agent_runtime::registry::Permission;
use agent_runtime::runtime::RuntimeBuilder;
use async_trait::async_trait;
use serde_json::{Map, Value, json};
use tokio::process::Command;

use crate::{AgentHostError, CanonicalScope, CanonicalScopeType, WorkspaceAccess};

/// Host-defined permission for a read-only Forge domain operation.
pub const FORGE_SCOPE_READ_PERMISSION: &str = "forge.scope.read";
/// Host-defined permission for a Forge proposal envelope.
pub const FORGE_SCOPE_PROPOSE_PERMISSION: &str = "forge.scope.propose";

const MAX_FILE_READ_BYTES: usize = 128 * 1024;
const MAX_FILE_WRITE_BYTES: usize = 1024 * 1024;
const MAX_COMMAND_OUTPUT_BYTES: usize = 128 * 1024;

/// A provider for Forge domain reads and proposal envelopes.
///
/// The host resolves the identity and canonical scope from the persisted
/// Forge session before it constructs the tools.  Implementations therefore
/// receive server-derived values and must not accept replacement identity or
/// scope values from the model arguments.
#[async_trait]
pub trait ForgeToolProvider: Send + Sync + fmt::Debug {
    /// Performs one already-scope-bound, read-only domain operation.
    async fn read(
        &self,
        actor_identity_id: &str,
        scope: &CanonicalScope,
        operation: &str,
        arguments: Value,
    ) -> Result<Value, AgentHostError>;

    /// Persists one already-scope-bound proposal envelope.  The provider is
    /// responsible for applying Forge's policy intersection and for keeping
    /// proposals separate from authoritative Task/workflow mutation.
    async fn propose(
        &self,
        actor_identity_id: &str,
        scope: &CanonicalScope,
        operation: &str,
        arguments: Value,
    ) -> Result<Value, AgentHostError>;
}

/// The role admitted for a Task-scoped native session.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TaskToolRole {
    /// May use bounded worktree reads, writes, and commands.
    Worker,
    /// May read the worktree and run the fixed validation check only.
    Reviewer,
}

impl TaskToolRole {
    fn parse(scope: &CanonicalScope, role: Option<&str>) -> Result<Self, AgentHostError> {
        match (scope.workspace_access, role) {
            (WorkspaceAccess::TaskWrite, Some("worker" | "coder")) => Ok(Self::Worker),
            (WorkspaceAccess::TaskRead, Some("reviewer")) => Ok(Self::Reviewer),
            (WorkspaceAccess::TaskWrite, Some(other))
            | (WorkspaceAccess::TaskRead, Some(other)) => Err(AgentHostError::Authority(format!(
                "Task workspace access is not valid for role `{other}`"
            ))),
            (_, None) => Err(AgentHostError::Authority(
                "Task tool composition requires a server-issued Task role".to_owned(),
            )),
            (WorkspaceAccess::Deny, _) => Err(AgentHostError::Authority(
                "Task tool composition requires TaskRead or TaskWrite access".to_owned(),
            )),
        }
    }
}

/// An immutable, scope-derived native tool composition.
#[derive(Clone)]
pub struct ScopeToolComposition {
    tools: Vec<Arc<dyn Tool>>,
    security_check: Arc<dyn SecurityCheck>,
    coverage: PermissionSet,
    actor_identity_id: String,
    scope: CanonicalScope,
}

impl fmt::Debug for ScopeToolComposition {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ScopeToolComposition")
            .field("actor_identity_id", &self.actor_identity_id)
            .field("scope", &self.scope)
            .field("tools", &self.tool_names())
            .finish_non_exhaustive()
    }
}

impl ScopeToolComposition {
    /// Composes one scope using a server-computed effective permission set.
    ///
    /// The set is computed by Forge after intersecting the identity, selected
    /// profile, membership, and canonical-scope ceilings.  Keeping it
    /// mandatory here prevents an exported host composition from silently
    /// falling back to role-only authority.
    pub fn for_scope_with_permissions(
        actor_identity_id: impl Into<String>,
        scope: CanonicalScope,
        task_role: Option<&str>,
        workspace_root: Option<&str>,
        allowed_permissions: &BTreeSet<String>,
        provider: Option<Arc<dyn ForgeToolProvider>>,
    ) -> Result<Self, AgentHostError> {
        Self::for_scope_with_permissions_and_project_chat(
            actor_identity_id,
            scope,
            task_role,
            workspace_root,
            allowed_permissions,
            false,
            provider,
        )
    }

    /// Compose a scope using an optional server-derived Project Agent Chat
    /// authority.  The default composition above deliberately does not infer
    /// chat kind from an opaque id.  Native execution passes this bit only
    /// after the protected store has joined the canonical chat row and its
    /// owning Project, so Main Chat cannot acquire Task proposals by sending
    /// a forged permission set or prompt.
    pub fn for_scope_with_permissions_and_project_chat(
        actor_identity_id: impl Into<String>,
        scope: CanonicalScope,
        task_role: Option<&str>,
        workspace_root: Option<&str>,
        allowed_permissions: &BTreeSet<String>,
        project_agent_chat: bool,
        provider: Option<Arc<dyn ForgeToolProvider>>,
    ) -> Result<Self, AgentHostError> {
        scope.validate()?;
        let actor_identity_id = actor_identity_id.into();
        if actor_identity_id.trim().is_empty() {
            return Err(AgentHostError::Authority(
                "native tool composition requires a server-issued identity".to_owned(),
            ));
        }
        if matches!(scope.scope_type, CanonicalScopeType::Task)
            && workspace_root
                .filter(|root| !root.trim().is_empty())
                .is_none()
        {
            return Err(AgentHostError::Authority(
                "Task tool composition requires the host-issued workspace root".to_owned(),
            ));
        }
        if !matches!(scope.scope_type, CanonicalScopeType::Task) && workspace_root.is_some() {
            return Err(AgentHostError::Authority(
                "non-Task tool composition cannot receive a workspace root".to_owned(),
            ));
        }

        let mut tools: Vec<Arc<dyn Tool>> = Vec::new();
        let mut coverage_set = BTreeSet::new();
        let mut custom_permissions = BTreeSet::new();

        match scope.scope_type {
            CanonicalScopeType::Task => {
                let role = TaskToolRole::parse(&scope, task_role)?;
                let root = workspace_root.expect("validated Task workspace root");
                let task_read_allowed = allowed_permissions.contains("task_read");
                let task_write_allowed = allowed_permissions.contains("task_write");
                if task_read_allowed {
                    tools.push(Arc::new(TaskReadTool));
                    coverage_set.insert(Permission::FsRead);
                }
                match role {
                    TaskToolRole::Worker => {
                        if task_write_allowed {
                            tools.push(Arc::new(TaskWriteTool));
                            tools.push(Arc::new(TaskCommandTool));
                            coverage_set.insert(Permission::FsWrite);
                            coverage_set.insert(Permission::ProcessSpawn);
                        }
                    }
                    TaskToolRole::Reviewer => {
                        if task_read_allowed {
                            tools.push(Arc::new(TaskValidateTool));
                            coverage_set.insert(Permission::ProcessSpawn);
                        }
                    }
                }
                if let Some(provider) = provider {
                    let (read_operations, propose_operations) = task_operations(role);
                    let read_operations =
                        filter_operations(scope.scope_type, &read_operations, allowed_permissions);
                    let propose_operations = filter_operations(
                        scope.scope_type,
                        &propose_operations,
                        allowed_permissions,
                    );
                    if !read_operations.is_empty() {
                        tools.push(Arc::new(ForgeScopeReadTool::new(
                            actor_identity_id.clone(),
                            scope.clone(),
                            read_operations,
                            provider.clone(),
                        )));
                        custom_permissions.insert(Permission::other(FORGE_SCOPE_READ_PERMISSION));
                    }
                    if !propose_operations.is_empty() {
                        tools.push(Arc::new(ForgeScopeProposeTool::new(
                            actor_identity_id.clone(),
                            scope.clone(),
                            propose_operations,
                            provider,
                        )));
                        custom_permissions
                            .insert(Permission::other(FORGE_SCOPE_PROPOSE_PERMISSION));
                    }
                }
                let _ = root;
            }
            CanonicalScopeType::Account
            | CanonicalScopeType::Project
            | CanonicalScopeType::AgentChat => {
                if let Some(provider) = provider {
                    let (read_operations, propose_operations) =
                        non_task_operations(scope.scope_type, project_agent_chat);
                    let read_operations =
                        filter_operations(scope.scope_type, &read_operations, allowed_permissions);
                    let propose_operations = filter_operations(
                        scope.scope_type,
                        &propose_operations,
                        allowed_permissions,
                    );
                    if !read_operations.is_empty() {
                        tools.push(Arc::new(ForgeScopeReadTool::new(
                            actor_identity_id.clone(),
                            scope.clone(),
                            read_operations,
                            provider.clone(),
                        )));
                        custom_permissions.insert(Permission::other(FORGE_SCOPE_READ_PERMISSION));
                    }
                    if !propose_operations.is_empty() {
                        tools.push(Arc::new(ForgeScopeProposeTool::new(
                            actor_identity_id.clone(),
                            scope.clone(),
                            propose_operations,
                            provider,
                        )));
                        custom_permissions
                            .insert(Permission::other(FORGE_SCOPE_PROPOSE_PERMISSION));
                    }
                }
            }
        }
        coverage_set.extend(custom_permissions);
        let coverage: PermissionSet = coverage_set.into_iter().collect();
        let security_check = Arc::new(ForgeScopeSecurityCheck {
            id: SecurityCheckId::new(format!(
                "forge-scope:{}:{}",
                scope_type_name(scope.scope_type),
                scope.scope_id
            )),
            revision: SecurityCheckRevision::new("forge-native-tools-v1"),
            coverage: coverage.clone(),
            workspace_root: workspace_root.map(str::to_owned),
        });
        Ok(Self {
            tools,
            security_check,
            coverage,
            actor_identity_id,
            scope,
        })
    }

    /// Returns the exact advertised names in deterministic order.
    pub fn tool_names(&self) -> Vec<String> {
        let mut names = self
            .tools
            .iter()
            .map(|tool| tool.spec().name)
            .collect::<Vec<_>>();
        names.sort();
        names
    }

    /// Returns the exact tool registry entries for inspection or composition.
    pub fn tools(&self) -> Vec<Arc<dyn Tool>> {
        self.tools.clone()
    }

    /// Returns the host-assigned typed permission coverage.
    pub fn coverage(&self) -> PermissionSet {
        self.coverage.clone()
    }

    /// Applies this composition to a RuntimeBuilder.
    pub fn apply(self, builder: RuntimeBuilder) -> RuntimeBuilder {
        builder.tools(self.tools).security_check(
            self.security_check,
            SecurityCheckMode::Authoritative,
            self.coverage,
            ActionClass::new("forge-native-scope"),
        )
    }

    /// The identity captured by the host composition.  This is informational
    /// and useful for setting RuntimeBuilder's security subject; tools never
    /// read it from model arguments.
    pub fn actor_identity_id(&self) -> &str {
        &self.actor_identity_id
    }

    /// The canonical scope captured by the host composition.
    pub fn scope(&self) -> &CanonicalScope {
        &self.scope
    }
}

fn non_task_operations(
    scope_type: CanonicalScopeType,
    project_agent_chat: bool,
) -> (Vec<String>, Vec<String>) {
    match scope_type {
        CanonicalScopeType::Account => (
            vec![
                "account.summary".to_owned(),
                // Main/account reads are bounded projections.  They do not
                // expose another Agent Chat's history or private memory.
                "discovery.read".to_owned(),
                "portfolio.read".to_owned(),
                "inbox.read".to_owned(),
                "commitments.read".to_owned(),
                "delivery.read".to_owned(),
            ],
            vec![
                "message.send".to_owned(),
                "commitment.update".to_owned(),
                "memory.publish".to_owned(),
                "memory.supersede".to_owned(),
                "session.action".to_owned(),
                "web.search".to_owned(),
                "project.lifecycle".to_owned(),
                "handoff.publish".to_owned(),
            ],
        ),
        CanonicalScopeType::Project => (
            vec![
                "project.summary".to_owned(),
                "work.read".to_owned(),
                "decisions.read".to_owned(),
                "events.read".to_owned(),
                "memory.read".to_owned(),
                "inbox.read".to_owned(),
                "commitments.read".to_owned(),
                "delivery.read".to_owned(),
            ],
            vec![
                "task.propose".to_owned(),
                "message.send".to_owned(),
                "commitment.update".to_owned(),
                "memory.publish".to_owned(),
                "memory.supersede".to_owned(),
                "review.request".to_owned(),
                "decision.request".to_owned(),
                "session.action".to_owned(),
            ],
        ),
        CanonicalScopeType::AgentChat => {
            let mut reads = vec![
                "agent_chat.summary".to_owned(),
                "events.read".to_owned(),
                "decisions.read".to_owned(),
                "memory.read".to_owned(),
                "inbox.read".to_owned(),
                "commitments.read".to_owned(),
                "delivery.read".to_owned(),
            ];
            let mut operations = vec![
                "message.send".to_owned(),
                "commitment.update".to_owned(),
                "memory.publish".to_owned(),
                "memory.supersede".to_owned(),
                "session.action".to_owned(),
            ];
            if project_agent_chat {
                operations.push("task.propose".to_owned());
            } else {
                // A Main Chat receives the global portfolio/discovery
                // surface.  These operations are deliberately absent
                // from Project Chat, whose only mutation authority is
                // its own Project Task proposal envelope.
                reads.extend([
                    "discovery.read".to_owned(),
                    "portfolio.read".to_owned(),
                    "project.summary".to_owned(),
                ]);
                operations.extend([
                    "web.search".to_owned(),
                    "project.lifecycle".to_owned(),
                    "handoff.publish".to_owned(),
                ]);
            }
            (reads, operations)
        }
        CanonicalScopeType::Task => (Vec::new(), Vec::new()),
    }
}

fn task_operations(_role: TaskToolRole) -> (Vec<String>, Vec<String>) {
    let read = vec![
        "task.summary".to_owned(),
        "work.read".to_owned(),
        "decisions.read".to_owned(),
        "events.read".to_owned(),
        "memory.read".to_owned(),
        "inbox.read".to_owned(),
        "commitments.read".to_owned(),
        "delivery.read".to_owned(),
    ];
    // Task mutation/review workflow remains in the existing executor and
    // review services.  Native Worker/reviewer tools never provide a second
    // Task mutation or workflow path; the reviewer receives read/validation
    // only, while Worker writes through the bounded worktree tools above.
    let propose = Vec::new();
    (read, propose)
}

fn filter_operations(
    scope_type: CanonicalScopeType,
    operations: &[String],
    allowed_permissions: &BTreeSet<String>,
) -> Vec<String> {
    operations
        .iter()
        .filter(|operation| {
            allowed_permissions.contains(operation_permission(scope_type, operation.as_str()))
        })
        .cloned()
        .collect()
}

/// Maps a public native operation to the persisted Forge permission ceiling.
/// The Agent Runtime only sees the single typed tool permission; this mapping
/// keeps the more detailed Forge policy from being flattened into an
/// all-or-nothing provider grant.
fn operation_permission(scope_type: CanonicalScopeType, operation: &str) -> &'static str {
    match (scope_type, operation) {
        (
            CanonicalScopeType::Account,
            "account.summary" | "discovery.read" | "portfolio.read" | "inbox.read"
            | "commitments.read" | "delivery.read",
        ) => "read_account",
        (
            CanonicalScopeType::Project,
            "project.summary" | "work.read" | "events.read" | "inbox.read" | "commitments.read"
            | "delivery.read",
        ) => "read_project",
        (
            CanonicalScopeType::AgentChat,
            "agent_chat.summary" | "discovery.read" | "portfolio.read" | "project.summary"
            | "events.read" | "inbox.read" | "commitments.read" | "delivery.read",
        ) => "read_agent_chat",
        (
            CanonicalScopeType::Task,
            "task.summary" | "work.read" | "events.read" | "inbox.read" | "commitments.read"
            | "delivery.read",
        ) => "read_task",
        (_, "memory.read" | "decisions.read") => "read_memory",
        (_, "task.propose") => "propose_task",
        (_, "message.propose" | "message.send") => "propose_message",
        (_, "commitment.propose" | "commitment.update") => "propose_commitment",
        (_, "memory.publish" | "memory.supersede") => "propose_memory",
        (_, "review.propose" | "review.request") => "propose_review",
        (_, "decision.request") => "propose_decision",
        (_, "session.action") => "propose_session",
        (_, "web.search") => "propose_discovery",
        (_, "project.lifecycle") => "propose_project",
        (_, "handoff.publish") => "propose_handoff",
        _ => "__unknown_forge_permission__",
    }
}

#[derive(Debug)]
struct ForgeScopeSecurityCheck {
    id: SecurityCheckId,
    revision: SecurityCheckRevision,
    coverage: PermissionSet,
    workspace_root: Option<String>,
}

#[async_trait]
impl SecurityCheck for ForgeScopeSecurityCheck {
    fn id(&self) -> &SecurityCheckId {
        &self.id
    }

    fn revision(&self) -> &SecurityCheckRevision {
        &self.revision
    }

    async fn evaluate(
        &self,
        request: &AuthorizationRequest,
        _cancel: &Cancellation,
    ) -> SecurityCheckOutcome {
        if !request.requested.is_subset(&self.coverage) {
            return SecurityCheckOutcome::Deny {
                code: DecisionCode::other("forge_scope_permission_not_covered"),
            };
        }
        let valid_resource = match &request.resource {
            SecurityResource::Filesystem { mount, segments } => {
                self.workspace_root.as_deref() == Some(mount.as_str())
                    && segments.iter().all(|segment| {
                        !segment.is_empty()
                            && segment != "."
                            && segment != ".."
                            && !segment.contains('/')
                            && !segment.contains('\\')
                    })
            }
            SecurityResource::Other { kind, .. } => kind == "forge.scope" || kind == "process",
            _ => false,
        };
        if !valid_resource {
            return SecurityCheckOutcome::Deny {
                code: DecisionCode::other("forge_scope_resource_not_bound"),
            };
        }
        SecurityCheckOutcome::Allow {
            constraints: GrantConstraints::unconstrained(),
        }
    }
}

#[derive(Debug)]
struct ForgeScopeReadTool {
    actor_identity_id: String,
    scope: CanonicalScope,
    operations: BTreeSet<String>,
    provider: Arc<dyn ForgeToolProvider>,
}

impl ForgeScopeReadTool {
    fn new(
        actor_identity_id: String,
        scope: CanonicalScope,
        operations: Vec<String>,
        provider: Arc<dyn ForgeToolProvider>,
    ) -> Self {
        Self {
            actor_identity_id,
            scope,
            operations: operations.into_iter().collect(),
            provider,
        }
    }

    fn spec_with_operations(&self) -> ToolSpec {
        ToolSpec::new(
            "forge_scope_read",
            "Read one bounded Forge resource from the current canonical scope.",
            json!({
                "type": "object",
                "required": ["operation"],
                "properties": {
                    "operation": {
                        "type": "string",
                        "enum": self.operations.iter().collect::<Vec<_>>(),
                    },
                    "arguments": {"type": "object"}
                },
                "additionalProperties": false
            }),
            ToolEffects::new(Vec::new()),
        )
        .with_permission_upper_bound(PermissionSet::single(Permission::other(
            FORGE_SCOPE_READ_PERMISSION,
        )))
    }
}

#[async_trait]
impl Tool for ForgeScopeReadTool {
    fn spec(&self) -> ToolSpec {
        self.spec_with_operations()
    }

    async fn prepare(
        &self,
        arguments: Value,
        ctx: &PreparationContext,
    ) -> Result<PreparedToolCall, RuntimeError> {
        let operation = required_string(&arguments, "operation")?;
        if !self.operations.contains(operation) {
            return Err(RuntimeError::tool(
                "Forge read operation is outside this scope",
            ));
        }
        let resource = SecurityResource::other(
            "forge.scope",
            format!(
                "{}:{}",
                scope_type_name(self.scope.scope_type),
                self.scope.scope_id
            ),
        );
        Ok(PreparedToolCall::new(
            ctx.call_id.clone(),
            "forge_scope_read",
            arguments,
            PermissionSet::single(Permission::other(FORGE_SCOPE_READ_PERMISSION)),
            resource,
            ToolEffects::new(Vec::new()),
            ToolCallDisplay::new("Read Forge scope"),
        ))
    }

    async fn invoke(
        &self,
        prepared: PreparedToolCall,
        _ctx: &InvocationContext,
    ) -> Result<ToolOutcome, RuntimeError> {
        let arguments = prepared.into_arguments();
        let operation = required_string(&arguments, "operation")?;
        let input = arguments
            .get("arguments")
            .cloned()
            .unwrap_or_else(|| Value::Object(Map::new()));
        let output = self
            .provider
            .read(&self.actor_identity_id, &self.scope, operation, input)
            .await
            .map_err(host_error_to_runtime)?;
        Ok(ToolOutcome::json(output))
    }
}

#[derive(Debug)]
struct ForgeScopeProposeTool {
    actor_identity_id: String,
    scope: CanonicalScope,
    operations: BTreeSet<String>,
    provider: Arc<dyn ForgeToolProvider>,
}

impl ForgeScopeProposeTool {
    fn new(
        actor_identity_id: String,
        scope: CanonicalScope,
        operations: Vec<String>,
        provider: Arc<dyn ForgeToolProvider>,
    ) -> Self {
        Self {
            actor_identity_id,
            scope,
            operations: operations.into_iter().collect(),
            provider,
        }
    }

    fn spec_with_operations(&self) -> ToolSpec {
        ToolSpec::new(
            "forge_scope_propose",
            "Submit a typed Forge proposal in the current canonical scope.",
            json!({
                "type": "object",
                "required": ["operation", "payload", "dedupe_key", "correlation_id"],
                "properties": {
                    "operation": {
                        "type": "string",
                        "enum": self.operations.iter().collect::<Vec<_>>(),
                    },
                    "payload": {"type": "object"},
                    "dedupe_key": {"type": "string", "minLength": 1},
                    "correlation_id": {"type": "string", "minLength": 1},
                    "causation_id": {"type": "string"},
                    "causation_depth": {"type": "integer", "minimum": 0, "maximum": 8},
                    "target_type": {"type": "string"},
                    "target_id": {"type": "string"}
                },
                "additionalProperties": false
            }),
            ToolEffects::new(Vec::new()),
        )
        .with_permission_upper_bound(PermissionSet::single(Permission::other(
            FORGE_SCOPE_PROPOSE_PERMISSION,
        )))
    }
}

#[async_trait]
impl Tool for ForgeScopeProposeTool {
    fn spec(&self) -> ToolSpec {
        self.spec_with_operations()
    }

    async fn prepare(
        &self,
        arguments: Value,
        ctx: &PreparationContext,
    ) -> Result<PreparedToolCall, RuntimeError> {
        let operation = required_string(&arguments, "operation")?;
        if !self.operations.contains(operation) {
            return Err(RuntimeError::tool(
                "Forge proposal operation is outside this scope",
            ));
        }
        for field in ["dedupe_key", "correlation_id"] {
            if required_string(&arguments, field)?.trim().is_empty() {
                return Err(RuntimeError::tool(format!("{field} cannot be empty")));
            }
        }
        let resource = SecurityResource::other(
            "forge.scope",
            format!(
                "{}:{}",
                scope_type_name(self.scope.scope_type),
                self.scope.scope_id
            ),
        );
        Ok(PreparedToolCall::new(
            ctx.call_id.clone(),
            "forge_scope_propose",
            arguments,
            PermissionSet::single(Permission::other(FORGE_SCOPE_PROPOSE_PERMISSION)),
            resource,
            ToolEffects::new(Vec::new()),
            ToolCallDisplay::new("Propose Forge action"),
        ))
    }

    async fn invoke(
        &self,
        prepared: PreparedToolCall,
        _ctx: &InvocationContext,
    ) -> Result<ToolOutcome, RuntimeError> {
        let output = self
            .provider
            .propose(
                &self.actor_identity_id,
                &self.scope,
                required_string(prepared.arguments(), "operation")?,
                prepared.arguments().clone(),
            )
            .await
            .map_err(host_error_to_runtime)?;
        Ok(ToolOutcome::json(output))
    }
}

#[derive(Debug)]
struct TaskReadTool;

#[async_trait]
impl Tool for TaskReadTool {
    fn spec(&self) -> ToolSpec {
        ToolSpec::new(
            "forge_task_read",
            "Read a UTF-8 file inside the admitted Task Workspace.",
            json!({
                "type":"object",
                "required":["path"],
                "properties":{"path":{"type":"string","minLength":1}},
                "additionalProperties":false
            }),
            ToolEffects::read_only(),
        )
    }

    async fn prepare(
        &self,
        arguments: Value,
        ctx: &PreparationContext,
    ) -> Result<PreparedToolCall, RuntimeError> {
        let path =
            bounded_workspace_path(ctx.workspace.as_ref(), required_string(&arguments, "path")?)?;
        let resource = filesystem_resource(ctx.workspace.root(), &path)?;
        let arguments = json!({"path": path});
        Ok(PreparedToolCall::new(
            ctx.call_id.clone(),
            "forge_task_read",
            arguments,
            PermissionSet::single(Permission::FsRead),
            resource,
            ToolEffects::read_only(),
            ToolCallDisplay::new("Read Task Workspace file"),
        ))
    }

    async fn invoke(
        &self,
        prepared: PreparedToolCall,
        ctx: &InvocationContext,
    ) -> Result<ToolOutcome, RuntimeError> {
        let path = required_string(prepared.arguments(), "path")?;
        let path = bounded_workspace_path(ctx.workspace.as_ref(), path)?;
        let bytes = std::fs::read(&path).map_err(|error| RuntimeError::tool(error.to_string()))?;
        let bounded = bytes
            .into_iter()
            .take(MAX_FILE_READ_BYTES)
            .collect::<Vec<_>>();
        let text = String::from_utf8_lossy(&bounded).into_owned();
        Ok(ToolOutcome::json(json!({
            "path": path,
            "content": text,
            "truncated": bounded.len() == MAX_FILE_READ_BYTES
        })))
    }
}

#[derive(Debug)]
struct TaskWriteTool;

#[async_trait]
impl Tool for TaskWriteTool {
    fn spec(&self) -> ToolSpec {
        ToolSpec::new(
            "forge_task_write",
            "Write UTF-8 content inside the admitted Task Workspace.",
            json!({
                "type":"object",
                "required":["path","content"],
                "properties":{
                    "path":{"type":"string","minLength":1},
                    "content":{"type":"string","maxLength":MAX_FILE_WRITE_BYTES}
                },
                "additionalProperties":false
            }),
            // The concrete write path is derived during `prepare`; this
            // static scope supplies the typed FsWrite upper bound without
            // granting a literal path outside the host workspace.
            ToolEffects::new(Vec::new()).with_write("<task-workspace>"),
        )
    }

    async fn prepare(
        &self,
        arguments: Value,
        ctx: &PreparationContext,
    ) -> Result<PreparedToolCall, RuntimeError> {
        let path =
            bounded_workspace_path(ctx.workspace.as_ref(), required_string(&arguments, "path")?)?;
        let content = required_string(&arguments, "content")?;
        if content.len() > MAX_FILE_WRITE_BYTES {
            return Err(RuntimeError::tool("Task Workspace write is too large"));
        }
        let resource = filesystem_resource(ctx.workspace.root(), &path)?;
        let arguments = json!({"path": path, "content": content});
        let effects = ToolEffects::new(Vec::new()).with_write(path.clone());
        Ok(PreparedToolCall::new(
            ctx.call_id.clone(),
            "forge_task_write",
            arguments,
            PermissionSet::single(Permission::FsWrite),
            resource,
            effects,
            ToolCallDisplay::new("Write Task Workspace file"),
        ))
    }

    async fn invoke(
        &self,
        prepared: PreparedToolCall,
        ctx: &InvocationContext,
    ) -> Result<ToolOutcome, RuntimeError> {
        let path = required_string(prepared.arguments(), "path")?;
        let content = required_string(prepared.arguments(), "content")?;
        let path = bounded_workspace_path(ctx.workspace.as_ref(), path)?;
        if let Some(parent) = Path::new(&path).parent() {
            std::fs::create_dir_all(parent)
                .map_err(|error| RuntimeError::tool(error.to_string()))?;
        }
        let path = bounded_workspace_path(ctx.workspace.as_ref(), &path)?;
        std::fs::write(&path, content).map_err(|error| RuntimeError::tool(error.to_string()))?;
        Ok(ToolOutcome::json(json!({"path": path, "written": true})))
    }
}

#[derive(Debug)]
struct TaskCommandTool;

#[async_trait]
impl Tool for TaskCommandTool {
    fn spec(&self) -> ToolSpec {
        ToolSpec::new(
            "forge_task_command",
            "Run one allowlisted command with the Task Workspace as its current directory.",
            json!({
                "type":"object",
                "required":["program"],
                "properties":{
                    "program":{"type":"string","minLength":1},
                    "args":{"type":"array","items":{"type":"string"},"maxItems":128}
                },
                "additionalProperties":false
            }),
            ToolEffects::new(Vec::new()).with_spawn(),
        )
    }

    async fn prepare(
        &self,
        arguments: Value,
        ctx: &PreparationContext,
    ) -> Result<PreparedToolCall, RuntimeError> {
        let program = required_string(&arguments, "program")?;
        validate_command_program(program)?;
        let args = string_array(&arguments, "args")?;
        validate_command_args(&args)?;
        if ctx.workspace.root() == "<none>" {
            return Err(RuntimeError::workspace("Task command requires a workspace"));
        }
        let arguments = json!({"program": program, "args": args});
        Ok(PreparedToolCall::new(
            ctx.call_id.clone(),
            "forge_task_command",
            arguments,
            PermissionSet::single(Permission::ProcessSpawn),
            SecurityResource::other("process", "forge_task_command"),
            ToolEffects::new(Vec::new()).with_spawn(),
            ToolCallDisplay::new("Run Task Workspace command"),
        ))
    }

    async fn invoke(
        &self,
        prepared: PreparedToolCall,
        ctx: &InvocationContext,
    ) -> Result<ToolOutcome, RuntimeError> {
        let program = required_string(prepared.arguments(), "program")?;
        let args = string_array(prepared.arguments(), "args")?;
        run_workspace_command(program, &args, ctx).await
    }
}

#[derive(Debug)]
struct TaskValidateTool;

#[async_trait]
impl Tool for TaskValidateTool {
    fn spec(&self) -> ToolSpec {
        ToolSpec::new(
            "forge_task_validate",
            "Run the fixed read-only git whitespace validation for the Task Workspace.",
            json!({"type":"object","additionalProperties":false}),
            ToolEffects::new(Vec::new()).with_spawn(),
        )
    }

    async fn prepare(
        &self,
        arguments: Value,
        ctx: &PreparationContext,
    ) -> Result<PreparedToolCall, RuntimeError> {
        if !arguments.is_object() {
            return Err(RuntimeError::tool("validation arguments must be an object"));
        }
        if ctx.workspace.root() == "<none>" {
            return Err(RuntimeError::workspace(
                "Task validation requires a workspace",
            ));
        }
        Ok(PreparedToolCall::new(
            ctx.call_id.clone(),
            "forge_task_validate",
            json!({}),
            PermissionSet::single(Permission::ProcessSpawn),
            SecurityResource::other("process", "forge_task_validate"),
            ToolEffects::new(Vec::new()).with_spawn(),
            ToolCallDisplay::new("Validate Task Workspace"),
        ))
    }

    async fn invoke(
        &self,
        _prepared: PreparedToolCall,
        ctx: &InvocationContext,
    ) -> Result<ToolOutcome, RuntimeError> {
        run_workspace_command("git", &["diff".to_owned(), "--check".to_owned()], ctx).await
    }
}

async fn run_workspace_command(
    program: &str,
    args: &[String],
    ctx: &InvocationContext,
) -> Result<ToolOutcome, RuntimeError> {
    if ctx.should_stop() {
        return Err(RuntimeError::cancelled("Task command cancelled"));
    }
    let mut command = Command::new(program);
    command
        .args(args)
        .current_dir(ctx.workspace.root())
        .env_clear()
        .env("PATH", std::env::var("PATH").unwrap_or_default())
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let output = command
        .output()
        .await
        .map_err(|error| RuntimeError::tool(format!("Task command failed: {error}")))?;
    let stdout = bounded_text(&output.stdout, MAX_COMMAND_OUTPUT_BYTES);
    let stderr = bounded_text(&output.stderr, MAX_COMMAND_OUTPUT_BYTES);
    Ok(ToolOutcome::json(json!({
        "program": program,
        "args": args,
        "status": output.status.code(),
        "success": output.status.success(),
        "stdout": stdout.0,
        "stderr": stderr.0,
        "truncated": stdout.1 || stderr.1
    })))
}

fn bounded_text(bytes: &[u8], limit: usize) -> (String, bool) {
    let truncated = bytes.len() > limit;
    let bytes = &bytes[..bytes.len().min(limit)];
    (String::from_utf8_lossy(bytes).into_owned(), truncated)
}

fn required_string<'a>(arguments: &'a Value, field: &str) -> Result<&'a str, RuntimeError> {
    arguments
        .get(field)
        .and_then(Value::as_str)
        .ok_or_else(|| RuntimeError::tool(format!("{field} must be a string")))
}

fn string_array(arguments: &Value, field: &str) -> Result<Vec<String>, RuntimeError> {
    let Some(value) = arguments.get(field) else {
        return Ok(Vec::new());
    };
    value
        .as_array()
        .ok_or_else(|| RuntimeError::tool(format!("{field} must be an array")))?
        .iter()
        .map(|value| {
            value
                .as_str()
                .map(str::to_owned)
                .ok_or_else(|| RuntimeError::tool(format!("{field} must contain strings")))
        })
        .collect()
}

fn bounded_workspace_path(workspace: &dyn Workspace, raw: &str) -> Result<String, RuntimeError> {
    if raw.trim().is_empty() {
        return Err(RuntimeError::workspace(
            "Task Workspace path cannot be empty",
        ));
    }
    let raw_path = Path::new(raw);
    if raw_path
        .components()
        .any(|component| matches!(component, Component::ParentDir | Component::Prefix(_)))
    {
        return Err(RuntimeError::workspace(
            "Task Workspace path contains a forbidden traversal component",
        ));
    }
    let root = Path::new(workspace.root());
    let candidate = if raw_path.is_absolute() {
        raw_path.to_path_buf()
    } else {
        root.join(raw_path)
    };
    let candidate = candidate
        .to_str()
        .ok_or_else(|| RuntimeError::workspace("Task Workspace path is not valid UTF-8"))?;
    if !workspace.contains(candidate) {
        return Err(RuntimeError::workspace(
            "Task Workspace path resolves outside its root",
        ));
    }
    workspace.resolve(candidate)
}

fn filesystem_resource(root: &str, path: &str) -> Result<SecurityResource, RuntimeError> {
    let root_path = Path::new(root);
    let path = Path::new(path);
    let relative = path
        .strip_prefix(root_path)
        .map_err(|_| RuntimeError::workspace("Task Workspace path is outside its root"))?;
    let segments = relative
        .components()
        .filter_map(|component| match component {
            Component::Normal(value) => value.to_str().map(str::to_owned),
            Component::CurDir => None,
            _ => None,
        })
        .collect::<Vec<_>>();
    if segments.iter().any(|segment| segment.is_empty()) {
        return Err(RuntimeError::workspace(
            "Task Workspace resource contains an invalid component",
        ));
    }
    Ok(SecurityResource::filesystem(root.to_owned(), segments))
}

fn validate_command_program(program: &str) -> Result<(), RuntimeError> {
    const ALLOWED: &[&str] = &[
        "bash", "bundle", "cargo", "cat", "diff", "echo", "false", "find", "git", "go", "gradle",
        "grep", "head", "java", "make", "mvn", "node", "npm", "pnpm", "pytest", "python",
        "python3", "rg", "rustc", "sed", "sh", "swift", "tail", "true", "wc",
    ];
    if program.contains('/') || program.contains('\\') || !ALLOWED.contains(&program) {
        return Err(RuntimeError::tool(
            "Task command is not in the host allowlist",
        ));
    }
    Ok(())
}

fn validate_command_args(args: &[String]) -> Result<(), RuntimeError> {
    if args.len() > 128 {
        return Err(RuntimeError::tool("Task command has too many arguments"));
    }
    for arg in args {
        let path = Path::new(arg);
        if path.is_absolute()
            || path
                .components()
                .any(|component| component == Component::ParentDir)
        {
            return Err(RuntimeError::tool(
                "Task command arguments cannot escape the admitted workspace",
            ));
        }
    }
    Ok(())
}

fn scope_type_name(scope_type: CanonicalScopeType) -> &'static str {
    match scope_type {
        CanonicalScopeType::Account => "account",
        CanonicalScopeType::Project => "project",
        CanonicalScopeType::AgentChat => "agent_chat",
        CanonicalScopeType::Task => "task",
    }
}

fn host_error_to_runtime(error: AgentHostError) -> RuntimeError {
    match error {
        AgentHostError::Authority(message)
        | AgentHostError::Configuration(message)
        | AgentHostError::Unsupported(message) => RuntimeError::tool(message),
        AgentHostError::CredentialNotFound | AgentHostError::SessionNotFound => {
            RuntimeError::not_found("Forge runtime resource unavailable")
        }
        AgentHostError::Runtime(_) | AgentHostError::ProtectedPersistence => {
            RuntimeError::tool("Forge tool provider failed")
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use agent_runtime::core::{ids::ToolCallId, tool::Tool};

    fn scope(scope_type: CanonicalScopeType, access: WorkspaceAccess) -> CanonicalScope {
        CanonicalScope {
            scope_type,
            scope_id: "scope-1".to_owned(),
            workspace_access: access,
        }
    }

    fn all_permissions() -> BTreeSet<String> {
        BTreeSet::from([
            "read_account".to_owned(),
            "read_project".to_owned(),
            "read_agent_chat".to_owned(),
            "read_task".to_owned(),
            "read_memory".to_owned(),
            "propose_task".to_owned(),
            "propose_message".to_owned(),
            "propose_review".to_owned(),
            "propose_commitment".to_owned(),
            "propose_memory".to_owned(),
            "propose_decision".to_owned(),
            "propose_session".to_owned(),
            "task_read".to_owned(),
            "task_write".to_owned(),
        ])
    }

    #[test]
    fn account_without_service_is_deny_all_and_has_no_task_tools() {
        let composition = ScopeToolComposition::for_scope_with_permissions(
            "identity-1",
            scope(CanonicalScopeType::Account, WorkspaceAccess::Deny),
            None,
            None,
            &all_permissions(),
            None,
        )
        .expect("account composition");
        assert!(composition.tool_names().is_empty());
        assert!(composition.coverage().is_empty());
    }

    #[test]
    fn project_service_tools_are_proposals_not_task_workspace_authority() {
        let provider = Arc::new(TestProvider);
        let composition = ScopeToolComposition::for_scope_with_permissions(
            "identity-1",
            scope(CanonicalScopeType::Project, WorkspaceAccess::Deny),
            None,
            None,
            &all_permissions(),
            Some(provider),
        )
        .expect("project composition");
        let names = composition.tool_names();
        assert!(names.contains(&"forge_scope_propose".to_owned()));
        assert!(names.contains(&"forge_scope_read".to_owned()));
        assert!(!names.iter().any(|name| name.starts_with("forge_task_")));
        assert!(!composition.coverage().contains(&Permission::FsRead));
        assert!(!composition.coverage().contains(&Permission::FsWrite));
        assert!(!composition.coverage().contains(&Permission::ProcessSpawn));
    }

    #[test]
    fn persisted_permission_ceiling_filters_domain_operations() {
        let provider = Arc::new(TestProvider);
        let allowed = BTreeSet::from(["read_project".to_owned()]);
        let composition = ScopeToolComposition::for_scope_with_permissions(
            "identity-1",
            scope(CanonicalScopeType::Project, WorkspaceAccess::Deny),
            None,
            None,
            &allowed,
            Some(provider),
        )
        .expect("project composition");
        let names = composition.tool_names();
        assert_eq!(names, vec!["forge_scope_read"]);
        assert!(!composition.coverage().contains(&Permission::FsRead));
        assert!(!composition.coverage().contains(&Permission::ProcessSpawn));
    }

    #[test]
    fn task_worker_cannot_retain_write_tools_when_profile_is_read_only() {
        let allowed = BTreeSet::from(["task_read".to_owned()]);
        let composition = ScopeToolComposition::for_scope_with_permissions(
            "identity-1",
            scope(CanonicalScopeType::Task, WorkspaceAccess::TaskWrite),
            Some("worker"),
            Some("/tmp/forge/task-1"),
            &allowed,
            None,
        )
        .expect("worker composition");
        let names = composition.tool_names();
        assert!(names.contains(&"forge_task_read".to_owned()));
        assert!(!names.contains(&"forge_task_write".to_owned()));
        assert!(!names.contains(&"forge_task_command".to_owned()));
        assert!(composition.coverage().contains(&Permission::FsRead));
        assert!(!composition.coverage().contains(&Permission::FsWrite));
        assert!(!composition.coverage().contains(&Permission::ProcessSpawn));
    }

    #[test]
    fn worker_and_reviewer_surfaces_are_disjoint() {
        let worker = ScopeToolComposition::for_scope_with_permissions(
            "identity-1",
            scope(CanonicalScopeType::Task, WorkspaceAccess::TaskWrite),
            Some("worker"),
            Some("/tmp/forge/task-1"),
            &all_permissions(),
            None,
        )
        .expect("worker composition");
        let reviewer = ScopeToolComposition::for_scope_with_permissions(
            "identity-1",
            scope(CanonicalScopeType::Task, WorkspaceAccess::TaskRead),
            Some("reviewer"),
            Some("/tmp/forge/task-1"),
            &all_permissions(),
            None,
        )
        .expect("reviewer composition");
        let worker_names = worker.tool_names();
        let reviewer_names = reviewer.tool_names();
        assert!(worker_names.contains(&"forge_task_read".to_owned()));
        assert!(worker_names.contains(&"forge_task_write".to_owned()));
        assert!(worker_names.contains(&"forge_task_command".to_owned()));
        assert!(!worker_names.contains(&"forge_task_validate".to_owned()));
        assert!(reviewer_names.contains(&"forge_task_read".to_owned()));
        assert!(reviewer_names.contains(&"forge_task_validate".to_owned()));
        assert!(!reviewer_names.contains(&"forge_task_write".to_owned()));
        assert!(!reviewer_names.contains(&"forge_task_command".to_owned()));
        assert!(reviewer.coverage().contains(&Permission::FsRead));
        assert!(!reviewer.coverage().contains(&Permission::FsWrite));
    }

    #[test]
    fn task_role_and_workspace_must_be_server_derived() {
        let missing_role = ScopeToolComposition::for_scope_with_permissions(
            "identity-1",
            scope(CanonicalScopeType::Task, WorkspaceAccess::TaskWrite),
            None,
            Some("/tmp/forge/task-1"),
            &all_permissions(),
            None,
        );
        assert!(matches!(missing_role, Err(AgentHostError::Authority(_))));
        let missing_workspace = ScopeToolComposition::for_scope_with_permissions(
            "identity-1",
            scope(CanonicalScopeType::Task, WorkspaceAccess::TaskWrite),
            Some("worker"),
            None,
            &all_permissions(),
            None,
        );
        assert!(matches!(
            missing_workspace,
            Err(AgentHostError::Authority(_))
        ));
    }

    #[tokio::test]
    async fn task_read_preparation_rejects_sibling_and_parent_paths() {
        let tool = TaskReadTool;
        let workspace: Arc<dyn Workspace> = Arc::new(TestWorkspace {
            root: "/tmp/forge/task-1".to_owned(),
        });
        let context = PreparationContext {
            session: agent_runtime::core::ids::SessionId::new("session"),
            turn: None,
            call_id: ToolCallId::new("call"),
            request: agent_runtime::core::ids::RequestId::new("request"),
            workspace,
            clock: Arc::new(agent_runtime::core::clock::SystemClock),
            cancel: agent_runtime::core::cancel::Cancellation::new(),
            deadline: agent_runtime::core::clock::Deadline::never(),
        };
        let sibling = tool
            .prepare(json!({"path":"/tmp/forge/task-10/file"}), &context)
            .await;
        assert!(sibling.is_err());
        let parent = tool
            .prepare(json!({"path":"../task-2/file"}), &context)
            .await;
        assert!(parent.is_err());
    }

    #[derive(Debug)]
    struct TestProvider;

    #[async_trait]
    impl ForgeToolProvider for TestProvider {
        async fn read(
            &self,
            _actor_identity_id: &str,
            scope: &CanonicalScope,
            operation: &str,
            _arguments: Value,
        ) -> Result<Value, AgentHostError> {
            Ok(json!({"scope": scope.scope_id, "operation": operation}))
        }

        async fn propose(
            &self,
            _actor_identity_id: &str,
            scope: &CanonicalScope,
            operation: &str,
            _arguments: Value,
        ) -> Result<Value, AgentHostError> {
            Ok(json!({"scope": scope.scope_id, "operation": operation}))
        }
    }

    #[derive(Debug)]
    struct TestWorkspace {
        root: String,
    }

    impl Workspace for TestWorkspace {
        fn root(&self) -> &str {
            &self.root
        }

        fn contains(&self, path: &str) -> bool {
            Path::new(path) == Path::new(&self.root)
                || Path::new(path).starts_with(Path::new(&self.root))
        }
    }
}
