import { useAssignRole, useRemoveRole } from '@/api/hooks'
import type { AssigneeSelection } from '@/components/task-controls'
import type { AssignRoleRequest } from '@/types/generated'

type RoleResetFlags = {
  reset_workspace?: boolean
  reset_worktree?: boolean
}

type RolePickerAssignRequest = AssignRoleRequest & RoleResetFlags

export type RolePickerSubmitArgs = {
  taskId: string
  roleName: string
  selection: AssigneeSelection
  resetWorkspace?: boolean
  resetWorktree?: boolean
  onError?: (error: Error) => void
}

export function useRolePicker() {
  const assignRole = useAssignRole()
  const removeRole = useRemoveRole()

  const submit = ({
    taskId,
    roleName,
    selection,
    resetWorkspace,
    resetWorktree,
    onError,
  }: RolePickerSubmitArgs) => {
    if (selection.type === 'unassigned') {
      removeRole.mutate(
        {
          taskId,
          roleName,
          body:
            resetWorkspace || resetWorktree
              ? {
                  reset_workspace: Boolean(resetWorkspace),
                  reset_worktree: Boolean(resetWorktree),
                }
              : undefined,
        },
        { onError },
      )
      return
    }

    const body: RolePickerAssignRequest =
      selection.type === 'agent'
        ? {
            assignee_type: 'agent',
            assignee_id: selection.agentId,
            reset_workspace: resetWorkspace,
            reset_worktree: resetWorktree,
          }
        : {
            assignee_type: 'user',
            assignee_id: selection.userId,
            reset_workspace: resetWorkspace,
            reset_worktree: resetWorktree,
          }

    assignRole.mutate(
      {
        taskId,
        roleName,
        body,
      },
      { onError },
    )
  }

  return {
    submit,
    isPending: assignRole.isPending || removeRole.isPending,
  }
}
