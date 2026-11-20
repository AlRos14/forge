import { render, screen } from '@testing-library/react'
import { describe, expect, it } from 'vitest'
import type { Execution } from '@/types/generated'
import { ExecutionDetailSidebar } from './ExecutionDetailSidebar'

const execution = {
  id: 'execution-1',
  task_id: 'task-1',
  actor_ref: null,
  role: 'implementation',
  status: 'completed',
  parent_execution_id: null,
  harness_session_id: null,
  prompt: null,
  summary: null,
  logs_path: null,
  before_sha: null,
  after_sha: null,
  error: null,
  stop_reason: null,
  stopped_by: null,
  resume_policy: null,
  stopped_at: null,
  executor_config_snapshot: null,
  workspace_id: null,
  work_unit_id: 'work-unit-1',
  work_unit_version: 1,
  repository_results: [
    {
      repo_id: 'repo-api',
      workspace_id: 'workspace-api',
      before_sha: '12345678-base',
      after_sha: 'abcdef01-head',
      snapshot_digest: 'a'.repeat(64),
    },
    {
      repo_id: 'repo-web',
      workspace_id: 'workspace-web',
      before_sha: '87654321-base',
      after_sha: 'fedcba98-head',
      snapshot_digest: 'b'.repeat(64),
    },
  ],
  usage: null,
  account_usage: null,
  created_at: '2026-11-20T10:00:00Z',
  updated_at: '2026-11-20T10:00:00Z',
} as unknown as Execution

describe('ExecutionDetailSidebar repository results', () => {
  it('multirepo_result_execution_sidebar_renders_repo_shas', () => {
    render(
      <ExecutionDetailSidebar
        isLoading={false}
        execution={execution}
        logs={[]}
        usage={[]}
        hookLogs={[]}
        agentName={() => null}
        taskId="task-1"
        onNavigateParent={() => undefined}
      />,
    )

    expect(screen.getByText('repo-api')).toBeTruthy()
    expect(screen.getByText('repo-web')).toBeTruthy()
    expect(screen.getByText('12345678')).toBeTruthy()
    expect(screen.getByText('abcdef01')).toBeTruthy()
    expect(screen.getByText('87654321')).toBeTruthy()
    expect(screen.getByText('fedcba98')).toBeTruthy()
  })

  it('renders one nonprimary repository when legacy scalar fields are unset', () => {
    render(
      <ExecutionDetailSidebar
        isLoading={false}
        execution={{
          ...execution,
          repository_results: [execution.repository_results[1]],
        }}
        logs={[]}
        usage={[]}
        hookLogs={[]}
        agentName={() => null}
        taskId="task-1"
        onNavigateParent={() => undefined}
      />,
    )

    expect(screen.getByText('repo-web')).toBeTruthy()
    expect(screen.getByText('87654321')).toBeTruthy()
    expect(screen.getByText('fedcba98')).toBeTruthy()
  })
})
