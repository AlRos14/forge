use crate::workflow::{
    default_roles, default_states,
    dispatch::{
        default_tool_names, AgentDispatchContext, AgentPrompt, PromptBuilder,
        BUILDER_ID_CODER_IMPLEMENTATION_V2, BUILDER_ID_CODER_MERGE_FIX_V2,
        BUILDER_ID_CODER_REVIEW_FIX_V2, MANAGED_EXECUTION_CONTRACT,
    },
};

pub struct CoderImplementationPromptBuilder;
pub struct CoderReviewFixPromptBuilder;
pub struct CoderMergeFixPromptBuilder;

impl PromptBuilder for CoderImplementationPromptBuilder {
    fn id(&self) -> &'static str {
        BUILDER_ID_CODER_IMPLEMENTATION_V2
    }

    fn build(&self, ctx: &AgentDispatchContext) -> AgentPrompt {
        AgentPrompt {
            system: coder_system(ctx, None),
            user: implementation_user(ctx),
            tools: default_tool_names(default_roles::CODER),
        }
    }
}

impl PromptBuilder for CoderReviewFixPromptBuilder {
    fn id(&self) -> &'static str {
        BUILDER_ID_CODER_REVIEW_FIX_V2
    }

    fn build(&self, ctx: &AgentDispatchContext) -> AgentPrompt {
        AgentPrompt {
            system: coder_system(ctx, Some(REVIEW_FIX_ROLE_BOUNDARY)),
            user: review_fix_user(ctx),
            tools: default_tool_names(default_roles::CODER),
        }
    }
}

impl PromptBuilder for CoderMergeFixPromptBuilder {
    fn id(&self) -> &'static str {
        BUILDER_ID_CODER_MERGE_FIX_V2
    }

    fn build(&self, ctx: &AgentDispatchContext) -> AgentPrompt {
        AgentPrompt {
            system: coder_system(ctx, Some(MERGE_FIX_ROLE_BOUNDARY)),
            user: merge_fix_user(ctx),
            tools: default_tool_names(default_roles::CODER),
        }
    }
}

const CODER_ROLE_BOUNDARY: &str = "\
Coder boundary:
- Must implement only the requested task in the task worktree.
- Must inspect supplied plans, comments, and review feedback first, keep scope tight, run relevant verification, and commit completed changes.
- Must not change unrelated behavior, ignore failed verification, treat review feedback as optional, or claim success without running verification.
- Red flags: broad refactors, skipped checks, missing proof media for UI/runtime changes.";

const REVIEW_FIX_ROLE_BOUNDARY: &str = "\
Review-fix boundary:
- Must address prior review or CI feedback precisely while preserving the implementation direction.
- Must not reopen solved work or add unrelated changes.
- Red flags: ignored reviewer evidence, broad rewrites, fixes without verification.";

const MERGE_FIX_ROLE_BOUNDARY: &str = "\
Merge-fix boundary:
- Must resolve merge conflicts minimally while preserving implementation intent, then run targeted verification.
- Must not rewrite the feature or add unrelated cleanup.
- Red flags: redesigns, formatting churn outside conflicted areas, unrelated fixes.";

const CODER_HANDOFF_CONTRACT: &str = "\
Completion handoff: End your response with a handoff block containing sections Summary | Deliverables | Verification | Deviations | Next Step.
List any verification not run with the reason. For UI/runtime behavior changes, include proof media (screenshot or log snippet) or explain why proof could not be captured.";

fn coder_system(ctx: &AgentDispatchContext, extra_role_boundary: Option<&str>) -> String {
    let has_plan = ctx
        .plan
        .as_deref()
        .is_some_and(|plan| !plan.trim().is_empty());
    let mut system = "You are the coder agent for this Forge workflow task. Your job is to implement code changes in the worktree. Once you finish, the task moves to the reviewer agent for verification. Keep the scope tight, verify the result compiles and passes locally, and commit your changes.".to_string();
    system.push_str("\n\n");
    system.push_str(MANAGED_EXECUTION_CONTRACT);
    system.push_str("\n\n");
    system.push_str(CODER_ROLE_BOUNDARY);
    if let Some(extra_role_boundary) = extra_role_boundary {
        system.push_str("\n\n");
        system.push_str(extra_role_boundary);
    }
    system.push_str("\n\n");
    system.push_str(CODER_HANDOFF_CONTRACT);
    system.push_str("\n\nProof of work for app-touching changes: If your task modifies user-facing UI or runtime behavior, capture a screenshot (or short walkthrough video) demonstrating the change. Upload it with forge-ctl task media upload --task-id <id> --file <path> and post a comment with forge-ctl task media comment --task-id <id> --content validation-notes --media-url <url> before transitioning to review.");
    if has_plan {
        system.push_str(" A planner agent already investigated and produced a plan — do not redo that work. Treat the provided plan as instructions to execute now.");
    }
    if let Some(reason) = ctx.last_manual_bounce_reason.as_deref() {
        system.push_str("\n\nThis task was sent back with the following feedback: ");
        system.push_str(reason);
        system.push_str(". Address it in this attempt.");
    }
    system
}

fn implementation_user(ctx: &AgentDispatchContext) -> String {
    if let Some(ordered_prompt) =
        crate::task_service::build_first_turn_prompt_from_context(&ctx.task, &ctx.sub_tasks)
    {
        return ordered_prompt;
    }

    let mut user = format!(
        "Task: {}\n\nImplementation objective:\nMake the requested code changes in the worktree and leave the task ready for review.\n",
        ctx.task.title
    );
    if let Some(description) = ctx.task.description.as_deref() {
        user.push_str("\nDescription:\n");
        user.push_str(description);
        user.push('\n');
    }

    if let Some(reason) = last_merge_failed_reason(ctx) {
        user.push_str("\nMerge failed on the prior attempt:\n");
        user.push_str(&reason);
        user.push_str(
            "\nRebase your worktree onto the latest main and resolve the conflict before re-submitting.\n",
        );
    }

    if let Some(plan) = ctx.plan.as_deref().filter(|plan| !plan.trim().is_empty()) {
        user.push_str("\nPlan:\n");
        user.push_str(plan);
        user.push('\n');
    }

    if !ctx.comments.is_empty() {
        user.push_str("\nRecent comments:\n");
        for comment in &ctx.comments {
            user.push_str("- ");
            user.push_str(&comment.author_name);
            user.push_str(": ");
            user.push_str(&comment.content);
            user.push('\n');
        }
    }
    user
}

fn review_fix_user(ctx: &AgentDispatchContext) -> String {
    let mut user = format!("Task: {}\n", ctx.task.title);
    if let Some(description) = ctx.task.description.as_deref() {
        user.push_str("\nDescription:\n");
        user.push_str(description);
        user.push('\n');
    }
    if let Some(plan) = ctx.plan.as_deref().filter(|plan| !plan.trim().is_empty()) {
        user.push_str("\nOriginal plan:\n");
        user.push_str(plan);
        user.push('\n');
    }
    user.push_str(
        "\nA Collaboration Message attached the exact ReviewReport that requests changes. Inspect the current worktree, address those findings, and keep the repair scoped.\n",
    );
    if let Some(reason) = collaboration_context(ctx) {
        user.push_str("\nCollaboration Message and attached Artifact:\n");
        user.push_str(&reason);
        user.push('\n');
    }
    if let Some(execution_id) = ctx.continuation_of_execution_id.as_deref() {
        user.push_str("\nPrevious coder execution:\n");
        user.push_str(execution_id);
        user.push('\n');
    }
    if let Some(logs_path) = ctx.continuation_logs_path.as_deref() {
        user.push_str("\nPrevious coder log file:\n");
        user.push_str(logs_path);
        user.push('\n');
    }
    user
}

fn merge_fix_user(_ctx: &AgentDispatchContext) -> String {
    let mut user = String::new();
    user.push_str("Rebase your worktree branch onto the latest default branch, resolve the merge conflicts, and verify CI passes.");
    user
}

fn last_merge_failed_reason(ctx: &AgentDispatchContext) -> Option<String> {
    ctx.transition_log
        .iter()
        .rev()
        .find(|entry| entry.to_state == default_states::MERGE_FAILED)
        .map(|entry| entry.trigger_reason.clone())
}

fn collaboration_context(ctx: &AgentDispatchContext) -> Option<String> {
    ctx.latest_review_feedback
        .as_deref()
        .map(str::trim)
        .filter(|feedback| !feedback.is_empty())
        .map(str::to_owned)
}
