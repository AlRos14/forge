import { ProjectHooksSection } from '@/components/settings/ProjectHooksSection'
import type { Project } from '@/types/generated'

interface HooksTabProps {
  project?: Project
  projectId: string
  projectIsLoading: boolean
}

export function HooksTab({ project, projectId, projectIsLoading }: HooksTabProps) {
  return (
    <ProjectHooksSection
      project={project}
      projectId={projectId}
      projectIsLoading={projectIsLoading}
    />
  )
}
