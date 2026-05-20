import { fireEvent, render, screen, waitFor } from '@testing-library/react'
import { describe, expect, it, vi } from 'vitest'
import { HooksTab } from '@/components/settings/HooksTab'

const hooks = vi.hoisted(() => ({
  useTestProjectLifecycleHook: vi.fn(),
}))

vi.mock('@/api/hooks', () => hooks)
vi.mock('@/components/settings/ProjectHooksSection', () => ({
  ProjectHooksSection: () => null,
}))

describe('HooksTab hook testing', () => {
  it('runs a hook test and renders debug output fields', async () => {
    const mutateAsync = vi.fn().mockResolvedValue({
      status: 'failed',
      stdout: 'out',
      stderr: 'err',
      exit_code: 2,
      duration_ms: 42,
      timeout: false,
      working_dir: '/tmp/worktree',
      environment_preview: { FORGE_TASK_ID: 'task-1' },
      hook_log_path: '/tmp/hook.log',
    })
    hooks.useTestProjectLifecycleHook.mockReturnValue({
      mutateAsync,
      isPending: false,
    })

    render(
      <HooksTab
        projectId="p1"
        projectIsLoading={false}
        canSave
        isSaving={false}
        lifecycleHooks={{
          before_work: [
            { type: 'script', command: 'echo hi', timeout_seconds: 10, blocking: false },
          ],
        }}
        onLifecycleHooksChange={() => {}}
        onSave={() => {}}
      />,
    )

    fireEvent.change(screen.getByLabelText('Task ID for hook test'), {
      target: { value: 'task-1' },
    })
    fireEvent.click(screen.getByRole('button', { name: 'Test' }))

    await waitFor(() => expect(mutateAsync).toHaveBeenCalled())
    expect(screen.getByText(/Last hook test: before_work #0/)).toBeTruthy()
    expect(screen.getByText(/status=failed/)).toBeTruthy()
    expect(screen.getByText(/working_dir:/)).toBeTruthy()
    expect(screen.getByText(/hook_log_path:/)).toBeTruthy()
    expect(screen.getByText(/environment_preview:/)).toBeTruthy()
    expect(screen.getByText('stdout')).toBeTruthy()
    expect(screen.getByText('stderr')).toBeTruthy()
  })
})
