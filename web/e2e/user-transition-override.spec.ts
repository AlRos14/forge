import { mkdtemp } from 'node:fs/promises'
import { tmpdir } from 'node:os'
import { join } from 'node:path'
import { execFile } from 'node:child_process'
import { promisify } from 'node:util'
import { expect, test, type APIRequestContext } from './fixtures'

const execFileAsync = promisify(execFile)

type Id = string

async function git(repoPath: string, ...args: string[]) {
  await execFileAsync('git', ['-C', repoPath, ...args])
}

// Create a project + local git repo + root task + subtask (subtask already in_progress).
// All via the real REST API (same auth path the UI uses).
async function setupSubtaskInProgress(
  request: APIRequestContext,
): Promise<{ projectId: Id; rootId: Id; subtaskId: Id; subtaskVersion: number }> {
  const repoDir = await mkdtemp(join(tmpdir(), 'forge-e2e-override-'))
  await git(repoDir, 'init', '-b', 'main')
  await git(repoDir, 'config', 'user.email', 'e2e@forge.test')
  await git(repoDir, 'config', 'user.name', 'e2e')
  await execFileAsync('sh', ['-c', `echo "# e2e" > "${repoDir}/README.md"`])
  await git(repoDir, 'add', '.')
  await git(repoDir, 'commit', '-m', 'init')

  const project = await request.post('/api/v1/projects', { data: { name: `Override E2E ${Date.now()}` } })
  const projectId = (await project.json()).id as Id

  await request.post(`/api/v1/projects/${projectId}/repos`, {
    data: {
      name: 'e2e-repo',
      local_path: repoDir,
      remote_url: repoDir,
      default_branch: 'main',
    },
  })

  const root = await request.post(`/api/v1/projects/${projectId}/tasks`, { data: { title: 'Root feature' } })
  const rootId = (await root.json()).id as Id

  const subtask = await request.post(`/api/v1/projects/${projectId}/tasks`, {
    data: { title: 'Subtask child', parent_task_id: rootId },
  })
  const subtaskJson = (await subtask.json()) as { id: Id; version: number }

  // todo -> in_progress (normal edge) so we are positioned to test in_progress -> review.
  const tr = await request.post(`/api/v1/tasks/${subtaskJson.id}/transition`, {
    data: { status: 'in_progress', version: subtaskJson.version },
  })
  expect(tr.ok(), `todo->in_progress should succeed: ${tr.status()}`).toBeTruthy()
  const after = (await tr.json()).task as { version: number }

  return { projectId, rootId, subtaskId: subtaskJson.id, subtaskVersion: after.version }
}

test('user can move a subtask into review (bug fix) via the task detail dropdown', async ({
  page,
  request,
}) => {
  const { projectId, subtaskId } = await setupSubtaskInProgress(request)

  // Open the board with this subtask's detail panel active.
  await page.goto(`/projects/${projectId}/board?task=${subtaskId}`)
  // The detail sidebar renders the status control; wait for the in_progress badge/dropdown.
  await expect(page.getByText('Subtask child').first()).toBeVisible({ timeout: 15000 })

  // The TaskStatusDropdown trigger has aria-label "Move status from <status>".
  // Wait for it to appear — its presence confirms the project workflow loaded and the
  // subtask has available transitions (otherwise a static badge renders instead).
  const statusTrigger = page.getByRole('button', { name: /Move status from/i }).first()
  await expect(statusTrigger).toBeVisible({ timeout: 15000 })
  await statusTrigger.click()

  // Menu item is portaled; in this DropdownMenu variant it renders with role=button
  // and accessible name equal to the target status ("review").
  const reviewItem = page.getByRole('button', { name: 'review', exact: true })
  await expect(reviewItem).toBeVisible({ timeout: 5000 })
  await reviewItem.click()

  // SUCCESS CRITERION (the bug fix): the transition is accepted (previously returned 400
  // "state 'review' is not defined in workflow") and the UI reflects status = review.
  await expect
    .poll(
      async () => {
        const res = await request.get(`/api/v1/tasks/${subtaskId}`)
        return res.ok() ? ((await res.json()) as { status: string }).status : null
      },
      { timeout: 15000 },
    )
    .toBe('review')
})

test('override across a missing edge is audited as user:override', async ({ page, request }) => {
  const { projectId, subtaskId } = await setupSubtaskInProgress(request)

  // in_progress -> review (valid project-workflow Accept edge).
  let res = await request.get(`/api/v1/tasks/${subtaskId}`)
  let version = ((await res.json()) as { version: number }).version
  await request.post(`/api/v1/tasks/${subtaskId}/transition`, {
    data: { status: 'review', version },
  })

  // review -> done is a MISSING edge (review's Accept edge goes to merging, not done).
  // A user move here must be served via the override path.
  res = await request.get(`/api/v1/tasks/${subtaskId}`)
  version = ((await res.json()) as { version: number }).version
  const override = await request.post(`/api/v1/tasks/${subtaskId}/transition`, {
    data: { status: 'done', version, reason: 'e2e override to done' },
  })
  expect(override.ok(), `review->done override should succeed: ${override.status()}`).toBeTruthy()

  // Audit: the transition log entry records the override marker + caller reason.
  const log = await request.get(`/api/v1/tasks/${subtaskId}/transitions`)
  const entries = (await log.json()).items as Array<{
    from_state: string
    to_state: string
    triggered_by: string
    trigger_reason: string
  }>
  const overrideEntry = entries.find((e) => e.from_state === 'review' && e.to_state === 'done')
  expect(overrideEntry).toBeTruthy()
  expect(overrideEntry!.triggered_by).toBe('user:override:api')
  expect(overrideEntry!.trigger_reason).toBe('e2e override to done')

  // And the board reflects the terminal state.
  await page.goto(`/projects/${projectId}/board`)
  await expect(page.getByText('Subtask child').first()).toBeVisible({ timeout: 15000 })
})
