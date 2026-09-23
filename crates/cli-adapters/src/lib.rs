#![forbid(unsafe_code)]

pub mod claude;
pub mod codex;
pub mod command;
pub mod commit;
pub mod cursor;
pub mod gemini;
pub mod null;
pub mod opencode;
pub mod shell;
pub mod smith;

pub use claude::ClaudeCodeAdapter;
pub use codex::CodexAdapter;
pub use cursor::CursorAdapter;
pub use gemini::GeminiAdapter;
pub use null::NullAdapter;
pub use opencode::OpencodeAdapter;
pub use shell::ShellAdapter;
pub use smith::SmithAdapter;

use executors::HarnessAdapterRegistry;
use executors::{CapabilitySupport as S, HarnessCapabilities as C};

pub(crate) fn harness_capabilities(
    resume: S,
    cancel: S,
    structured_events: S,
    usage_reporting: S,
    account_usage_observation: S,
    model_selection: S,
    reasoning_controls: S,
    approval_policy: S,
    sandbox_controls: S,
    planning: S,
    review_mode: S,
    fork: S,
    steer: S,
    pause_resume: S,
    compaction: S,
    subagents: S,
) -> C {
    C {
        resume,
        cancel,
        structured_events,
        usage_reporting,
        account_usage_observation,
        model_selection,
        reasoning_controls,
        approval_policy,
        sandbox_controls,
        planning,
        review_mode,
        fork,
        steer,
        pause_resume,
        compaction,
        subagents,
    }
}

/// Refuse to report a successful Resume unless the integration's normalized
/// result confirms the exact external session requested by Forge.
pub(crate) fn require_exact_resumed_session(
    harness: &str,
    requested: Option<&str>,
    reported: Option<&str>,
) -> Result<(), executors::ExecutorError> {
    let Some(requested) = requested else {
        return Ok(());
    };
    if reported == Some(requested) {
        return Ok(());
    }
    Err(executors::ExecutorError::Unavailable(format!(
        "{harness} did not confirm requested HarnessSession {requested}; refusing to report Resume success"
    )))
}

/// Build a registry with all built-in adapters.
pub fn default_registry() -> HarnessAdapterRegistry {
    let mut registry = HarnessAdapterRegistry::new();
    registry.register(Box::new(ShellAdapter::new()));
    registry.register(Box::new(CodexAdapter::new()));
    registry.register(Box::new(ClaudeCodeAdapter::new()));
    registry.register(Box::new(CursorAdapter::new()));
    registry.register(Box::new(OpencodeAdapter::new()));
    registry.register(Box::new(GeminiAdapter::new()));
    registry.register(Box::new(SmithAdapter::new()));
    registry.register(Box::new(NullAdapter::new()));
    registry
}

#[cfg(test)]
pub(crate) fn test_execution_context(
    invocation: executors::HarnessInvocation,
    agent_config: serde_json::Value,
) -> executors::ExecutionContext {
    executors::ExecutionContext {
        task_id: "task-test".to_owned(),
        execution_id: "execution-test".to_owned(),
        role: "coder".to_owned(),
        worktree_path: "/tmp/forge-test-worktree".to_owned(),
        description: "test invocation".to_owned(),
        agent_config,
        invocation,
        logs_path: "/tmp/forge-test-execution.jsonl".to_owned(),
        heartbeat_interval_seconds: 30,
        max_turns: None,
        log_sender: None,
    }
}

#[cfg(test)]
mod capability_tests {
    use super::*;
    use executors::CapabilitySupport as S;

    #[test]
    fn resume_result_must_confirm_exact_external_session() {
        assert!(
            require_exact_resumed_session("Cursor", Some("session-a"), Some("session-a"))
                .is_ok()
        );
        for reported in [None, Some("session-b")] {
            let error = require_exact_resumed_session("Cursor", Some("session-a"), reported)
                .expect_err("mismatched or absent session evidence fails closed");
            assert!(matches!(error, executors::ExecutorError::Unavailable(_)));
        }
    }

    #[test]
    fn registered_adapters_expose_only_integration_evidence() {
        let registry = default_registry();
        let config = serde_json::json!({});
        let caps = |kind| {
            registry
                .get(&kind)
                .expect("adapter is registered")
                .capabilities(&config)
        };

        let codex = caps(executors::ExecutorKind::Codex);
        assert_eq!(
            codex,
            harness_capabilities(
                S::Native, S::Emulated, S::Native, S::Native, S::Native, S::Native,
                S::Native, S::Native, S::Native, S::Unsupported, S::Unsupported,
                S::Unsupported, S::Unsupported, S::Unsupported, S::Unsupported, S::Unsupported,
            )
        );
        assert_eq!(codex.resume, S::Native);
        assert_eq!(codex.cancel, S::Emulated);
        assert_eq!(codex.structured_events, S::Native);
        assert_eq!(codex.account_usage_observation, S::Native);
        assert_eq!(codex.fork, S::Unsupported);
        assert_eq!(codex.planning, S::Unsupported);

        let claude = caps(executors::ExecutorKind::ClaudeCode);
        assert_eq!(
            claude,
            harness_capabilities(
                S::Native, S::Emulated, S::Native, S::Native, S::Unsupported, S::Native,
                S::Native, S::Native, S::Unsupported, S::Native, S::Unsupported,
                S::Unsupported, S::Unsupported, S::Unsupported, S::Unsupported, S::Unsupported,
            )
        );
        assert_eq!(claude.resume, S::Native);
        assert_eq!(claude.cancel, S::Emulated);
        assert_eq!(claude.structured_events, S::Native);
        assert_eq!(claude.planning, S::Native);

        let cursor = caps(executors::ExecutorKind::Cursor);
        assert_eq!(
            cursor,
            harness_capabilities(
                S::Native, S::Emulated, S::Native, S::Native, S::Emulated, S::Native,
                S::Unsupported, S::Native, S::Unsupported, S::Unsupported, S::Unsupported,
                S::Unsupported, S::Unknown, S::Unsupported, S::Unsupported, S::Unsupported,
            )
        );
        assert_eq!(cursor.resume, S::Native);
        assert_eq!(cursor.cancel, S::Emulated);
        assert_eq!(cursor.account_usage_observation, S::Emulated);
        assert_eq!(cursor.steer, S::Unknown);

        let opencode = caps(executors::ExecutorKind::Opencode);
        assert_eq!(
            opencode,
            harness_capabilities(
                S::Native, S::Emulated, S::Native, S::Unsupported, S::Unsupported, S::Native,
                S::Unsupported, S::Native, S::Unsupported, S::Unsupported, S::Unsupported,
                S::Unsupported, S::Unsupported, S::Unsupported, S::Unsupported, S::Unsupported,
            )
        );
        assert_eq!(opencode.resume, S::Native);
        assert_eq!(opencode.usage_reporting, S::Unsupported);
        assert_eq!(opencode.planning, S::Unsupported);

        let gemini = caps(executors::ExecutorKind::Gemini);
        assert_eq!(
            gemini,
            harness_capabilities(
                S::Unsupported, S::Emulated, S::Unknown, S::Unsupported, S::Unsupported,
                S::Native, S::Unsupported, S::Native, S::Native, S::Unsupported, S::Unsupported,
                S::Unsupported, S::Unsupported, S::Unsupported, S::Unsupported, S::Unsupported,
            )
        );
        assert_eq!(gemini.resume, S::Unsupported);
        assert_eq!(gemini.cancel, S::Emulated);
        assert_eq!(gemini.structured_events, S::Unknown);
        assert_eq!(gemini.planning, S::Unsupported);

        let smith = caps(executors::ExecutorKind::Smith);
        assert_eq!(
            smith,
            harness_capabilities(
                S::Native, S::Emulated, S::Native, S::Native, S::Unsupported, S::Native,
                S::Native, S::Native, S::Unsupported, S::Unsupported, S::Unsupported,
                S::Unsupported, S::Unsupported, S::Unsupported, S::Unsupported, S::Unsupported,
            )
        );
        assert_eq!(smith.resume, S::Native);
        assert_eq!(smith.cancel, S::Emulated);
        assert_eq!(smith.usage_reporting, S::Native);

        let shell = caps(executors::ExecutorKind::Shell);
        assert_eq!(
            shell,
            harness_capabilities(
                S::Unsupported, S::Emulated, S::Unsupported, S::Unsupported, S::Unsupported,
                S::Unsupported, S::Unsupported, S::Unsupported, S::Unsupported, S::Unsupported,
                S::Unsupported, S::Unsupported, S::Unsupported, S::Unsupported, S::Unsupported,
                S::Unsupported,
            )
        );
        assert_eq!(shell.cancel, S::Emulated);
        let null = caps(executors::ExecutorKind::Null);
        assert_eq!(null, executors::HarnessCapabilities::unsupported());

        for capabilities in [shell, null] {
            assert_eq!(capabilities.planning, S::Unsupported);
            assert_eq!(capabilities.review_mode, S::Unsupported);
            assert_eq!(capabilities.reasoning_controls, S::Unsupported);
            assert_eq!(capabilities.subagents, S::Unsupported);
        }

        // PermissionPolicy::Plan is a permission setting. The Codex adapter
        // does not claim native planning from that setting.
        assert_eq!(
            registry
                .get(&executors::ExecutorKind::Codex)
                .unwrap()
                .capabilities(&serde_json::json!({"permission_policy":"plan"}))
                .planning,
            S::Unsupported
        );
    }

    #[test]
    fn adapter_policy_interpretation_preserves_core_high_risk_classification() {
        let registry = default_registry();
        let effective = |kind: executors::ExecutorKind, config: serde_json::Value| {
            let adapter = registry.get(&kind).expect("adapter registered");
            let interpretation = adapter.interpret_execution_policy(&config);
            executors::effective_policy::from_adapter_interpretation(
                &kind,
                &interpretation,
                Some("/tmp/workspace/task"),
                Some("/tmp/workspace"),
                &config,
            )
        };
        let codex_policy = effective(
            executors::ExecutorKind::Codex,
            serde_json::json!({"sandbox":"danger-full-access"}),
        );
        assert_eq!(codex_policy.isolation_posture, "danger-full-access");
        assert!(codex_policy.is_high_risk);

        let claude_policy = effective(
            executors::ExecutorKind::ClaudeCode,
            serde_json::json!({"dangerously_skip_permissions":true}),
        );
        assert_eq!(
            claude_policy.isolation_posture,
            "dangerously_skip_permissions"
        );
        assert!(claude_policy.is_high_risk);

        let force = effective(
            executors::ExecutorKind::Cursor,
            serde_json::json!({"force":true}),
        );
        let propose_only = effective(
            executors::ExecutorKind::Cursor,
            serde_json::json!({"force":false,"permission_policy":"plan"}),
        );
        assert_eq!(force.isolation_posture, "force");
        assert!(force.is_high_risk);
        assert_eq!(propose_only.isolation_posture, "propose_only");
        assert!(!propose_only.is_high_risk);
    }
}
