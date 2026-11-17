import { describe, expect, it } from 'vitest'
import { projectCopilotRouteScope } from './route-context'

describe('Project Copilot route scope', () => {
  it('uses the Task route context without falling back to a stored Project ID', () => {
    expect(
      projectCopilotRouteScope('/tasks/task-b/work', {
        taskId: 'task-b',
        projectId: 'stored-project-a',
      }),
    ).toEqual({ taskId: 'task-b' })
  })

  it('uses the exact Project route and ignores Task params', () => {
    expect(
      projectCopilotRouteScope('/projects/project-b/board', {
        projectId: 'project-b',
        taskId: 'task-a',
      }),
    ).toEqual({ projectId: 'project-b' })
    expect(
      projectCopilotRouteScope('/projects/project-b/plan', {
        projectId: 'project-b',
        taskId: 'task-a',
      }),
    ).toEqual({ projectId: 'project-b' })
  })

  it('does not expose the Project Copilot outside Project and Task routes', () => {
    expect(projectCopilotRouteScope('/operations', { projectId: 'stored-project-a' })).toEqual({})
  })
})
