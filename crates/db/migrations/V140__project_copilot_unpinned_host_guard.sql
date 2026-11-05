-- Project Copilot may resolve an unpinned Agent to the embedded host at
-- admission. The original V139 domain-event guard joined the daemon through
-- agent.daemon_id, so it rejected every mutation for an unpinned Agent even
-- though the run snapshot named the exact resolved host. Recreate the guard
-- against the frozen resolved_daemon_id, falling back to agent.daemon_id for
-- snapshots written before that field existed.
DROP TRIGGER IF EXISTS project_copilot_domain_event_guard;

CREATE TRIGGER project_copilot_domain_event_guard
BEFORE INSERT ON domain_event
WHEN EXISTS (
    SELECT 1 FROM project_copilot_run
    WHERE id = NEW.correlation_id
)
BEGIN
    SELECT CASE WHEN NOT EXISTS (
        SELECT 1
        FROM project_copilot_run AS run
        JOIN project_copilot AS binding
          ON binding.project_id = run.project_id
        JOIN project AS p
          ON p.id = run.project_id
        JOIN agent_current AS agent
          ON agent.id = run.agent_id
        JOIN daemon AS host
          ON host.id = COALESCE(
              json_extract(run.agent_snapshot_json, '$.resolved_daemon_id'),
              agent.daemon_id
          )
        JOIN user AS agent_owner ON agent_owner.id = agent.owner_id
        JOIN user AS current_user
          ON current_user.id = run.user_id
        JOIN project_member AS member
          ON member.project_id = run.project_id
         AND member.user_id = run.user_id
        JOIN project_member AS owner_membership
          ON owner_membership.project_id = run.project_id
         AND owner_membership.user_id = p.owner_id
        WHERE run.id = NEW.correlation_id
          AND run.status = 'running'
          AND run.mcp_token_hash IS NOT NULL
          AND run.mcp_token_expires_at IS NOT NULL
          AND julianday(run.mcp_token_expires_at) > julianday('now')
          AND binding.agent_id = run.agent_id
          AND binding.version = run.binding_version
          AND agent.version = run.agent_version
          AND agent.paused = 0
          AND host.status = 'online'
          AND host.visibility IN ('account', 'global')
          AND host.machine_id LIKE 'embedded:%'
          AND agent_owner.is_admin = 1
          AND (host.owner_id IS NULL OR host.owner_id = agent.owner_id)
          AND json_valid(run.agent_snapshot_json)
          AND json_extract(run.agent_snapshot_json, '$.daemon_authority.agent_owner_id') IS agent.owner_id
          AND json_extract(run.agent_snapshot_json, '$.daemon_authority.profile_id') IS agent.profile_id
          AND json_extract(run.agent_snapshot_json, '$.daemon_authority.executor_type') IS agent.executor_type
          AND json_extract(run.agent_snapshot_json, '$.daemon_authority.agent_daemon_id') IS agent.daemon_id
          AND json_extract(run.agent_snapshot_json, '$.daemon_authority.daemon_id') IS host.id
          AND json_extract(run.agent_snapshot_json, '$.daemon_authority.machine_id') IS host.machine_id
          AND json_extract(run.agent_snapshot_json, '$.daemon_authority.daemon_owner_id') IS host.owner_id
          AND json_extract(run.agent_snapshot_json, '$.daemon_authority.daemon_visibility') IS host.visibility
          AND EXISTS (
              SELECT 1 FROM json_each(CASE WHEN json_valid(host.detected_clis_json)
                  THEN host.detected_clis_json ELSE '[]' END) cli
              WHERE json_extract(cli.value, '$.kind') = agent.executor_type
                AND json_extract(cli.value, '$.availability') = 'authenticated'
          )
          AND p.owner_id IS NOT NULL
          AND p.owner_id = agent.owner_id
          AND owner_membership.role = 'owner'
          AND member.role IN ('owner', 'admin', 'member')
          AND (run.context_task_id IS NULL OR EXISTS (
              SELECT 1 FROM task context_task WHERE context_task.id = run.context_task_id
                AND context_task.project_id = run.project_id AND context_task.deleted_at IS NULL
          ))
          AND NEW.actor_type = 'agent'
          AND NEW.actor_id = run.agent_id
          AND (
              (NEW.scope_type = 'project' AND NEW.scope_id = run.project_id)
              OR (
                  NEW.scope_type = 'task'
                  AND (run.context_task_id IS NULL OR run.context_task_id = NEW.scope_id)
                  AND EXISTS (
                      SELECT 1 FROM task
                      WHERE task.id = NEW.scope_id
                        AND task.project_id = run.project_id
                        AND task.deleted_at IS NULL
                  )
              )
          )
    ) THEN RAISE(ABORT, 'project copilot run authority is stale') END;
END;
