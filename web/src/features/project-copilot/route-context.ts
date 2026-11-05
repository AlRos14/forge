export type ProjectCopilotRouteParams = {
  projectId?: string
  taskId?: string
}

export function projectCopilotRouteScope(
  pathname: string,
  params?: ProjectCopilotRouteParams,
): ProjectCopilotRouteParams {
  if (/^\/projects\/[^/]+(?:\/|$)/.test(pathname)) {
    return params?.projectId ? { projectId: params.projectId } : {}
  }
  if (/^\/tasks\/[^/]+(?:\/|$)/.test(pathname)) {
    return params?.taskId ? { taskId: params.taskId } : {}
  }
  return {}
}
