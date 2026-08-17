import { useEffect, useRef } from 'react'
import { Link } from '@tanstack/react-router'
import { ArrowUpRight } from '@phosphor-icons/react'
import { useProjectsQuery } from '@/api/hooks'
import { MainAgentBindingCard } from '@/components/settings/MainAgentBindingCard'
import { ProjectAgentTab } from '@/components/settings/ProjectAgentTab'
import type { AgentChatEntry } from '@/features/agent-chat/types'
import type { FederatedAgent } from '@/features/federation/types'
import { ErrorPanel, LoadingPanel, SectionKicker, StateBadge } from '@/features/federation/components'
import type { ChangeModelBindingContext } from './ChangeModelDialog'
import { humanize } from './format'

function BoundAgentScopes({
  entries,
  isLoading,
  isError,
  onRetry,
}: {
  entries: AgentChatEntry[]
  isLoading: boolean
  isError: boolean
  onRetry: () => void
}) {
  if (isLoading) return <LoadingPanel label="Loading Main and Project Agent bindings" />
  if (isError) {
    return (
      <ErrorPanel
        title="Agent binding projection unavailable"
        description="Forge could not load the current Main and Project Agent scopes. Retry before relying on this view."
        onRetry={onRetry}
      />
    )
  }
  return (
    <section
      aria-labelledby="bound-agent-scopes-heading"
      className="overflow-hidden rounded-xl border border-border-subtle bg-card shadow-soft"
    >
      <header className="border-b border-border-subtle px-4 py-4 sm:px-5">
        <SectionKicker>Agent chat scopes</SectionKicker>
        <h2 id="bound-agent-scopes-heading" className="mt-1 text-lg font-semibold text-foreground">
          Main and Project Agent bindings
        </h2>
        <p className="mt-1 max-w-2xl text-sm leading-6 text-muted-foreground">
          These are the only durable chat owners. Task Workers and reviewers appear in Task detail,
          while unbound agents stay in the Agents tab.
        </p>
      </header>
      {entries.length > 0 ? (
        <div className="divide-y divide-ember-border">
          {entries.map((entry) => {
            const isMain = entry.kind === 'main'
            const label = isMain ? 'Global · Main' : (entry.project_name ?? 'Project Agent')
            const identity = entry.identity_name ?? 'Setup required'
            const status =
              entry.binding_state === 'active' ? entry.chat_status : entry.binding_state
            return (
              <div
                key={entry.chat_id}
                className="flex min-w-0 items-center justify-between gap-3 px-4 py-3"
              >
                <div className="min-w-0">
                  <p className="truncate text-sm font-medium text-foreground">{label}</p>
                  <p className="mt-1 truncate text-xs text-muted-foreground">
                    {identity} · {isMain ? 'account-owned timeline' : 'Project-owned timeline'}
                  </p>
                </div>
                <div className="flex shrink-0 items-center gap-3">
                  <StateBadge status={status} label={humanize(status)} />
                  {isMain ? (
                    <Link
                      to="/chat"
                      className="inline-flex items-center gap-1 text-xs font-medium text-primary hover:underline focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring"
                    >
                      Open chat
                      <ArrowUpRight size={13} aria-hidden />
                    </Link>
                  ) : (
                    <Link
                      to="/projects/$projectId/chat"
                      params={{ projectId: entry.project_id ?? '' }}
                      className="inline-flex items-center gap-1 text-xs font-medium text-primary hover:underline focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring"
                    >
                      Open chat
                      <ArrowUpRight size={13} aria-hidden />
                    </Link>
                  )}
                </div>
              </div>
            )
          })}
        </div>
      ) : (
        <p className="px-4 py-4 text-sm text-muted-foreground">
          No Main or Project Agent binding is visible yet. Create an agent and choose its owning
          scope below.
        </p>
      )}
    </section>
  )
}

/**
 * Bindings tab: Main Agent binding, every Project's agent binding (always
 * listed, never gated behind a URL param), and the read-only chat-scope
 * projection.
 */
export function BindingsTab({
  agents,
  chatEntries,
  chatsLoading,
  chatsError,
  onRetryChats,
  onConnect,
  onChangeModel,
  highlightedProjectId,
}: {
  agents: FederatedAgent[]
  chatEntries: AgentChatEntry[]
  chatsLoading: boolean
  chatsError: boolean
  onRetryChats: () => void
  onConnect: () => void
  onChangeModel: (agent: FederatedAgent, binding: ChangeModelBindingContext) => void
  highlightedProjectId?: string
}) {
  const projectsQuery = useProjectsQuery()
  const projects = projectsQuery.data?.items ?? []
  const scrolledFor = useRef<string | undefined>(undefined)

  useEffect(() => {
    if (!highlightedProjectId || scrolledFor.current === highlightedProjectId) return
    const element = document.getElementById(`project-agent-${highlightedProjectId}`)
    if (!element) return
    scrolledFor.current = highlightedProjectId
    element.scrollIntoView({ behavior: 'smooth', block: 'center' })
  }, [highlightedProjectId, projects.length])

  return (
    <div
      role="tabpanel"
      id="agent-settings-panel-bindings"
      aria-labelledby="agent-settings-tab-bindings"
      className="space-y-6"
    >
      <MainAgentBindingCard
        agents={agents}
        onConnect={onConnect}
        onChangeModel={(agent) => onChangeModel(agent, { kind: 'main' })}
      />

      <section aria-labelledby="project-agent-bindings-heading" className="space-y-3">
        <div>
          <SectionKicker>Every project</SectionKicker>
          <h2 id="project-agent-bindings-heading" className="mt-1 text-lg font-semibold text-foreground">
            Project Agent bindings
          </h2>
          <p className="mt-1 max-w-2xl text-sm leading-6 text-muted-foreground">
            Each Project has at most one Agent Chat binding. Projects without one yet still appear
            here, marked not configured.
          </p>
        </div>
        {projectsQuery.isLoading ? <LoadingPanel label="Loading projects" /> : null}
        {projectsQuery.isError ? (
          <ErrorPanel
            title="Projects unavailable"
            description="Forge could not load the project list. Retry before configuring a Project Agent."
            onRetry={() => void projectsQuery.refetch()}
          />
        ) : null}
        {!projectsQuery.isLoading && !projectsQuery.isError && projects.length === 0 ? (
          <p className="text-sm text-muted-foreground">No projects exist yet.</p>
        ) : null}
        <div className="space-y-4">
          {projects.map((project) => (
            <ProjectAgentTab
              key={project.id}
              projectId={project.id}
              projectName={project.name}
              highlighted={project.id === highlightedProjectId}
              onChangeModel={(agent) =>
                onChangeModel(agent, { kind: 'project', projectId: project.id, projectName: project.name })
              }
            />
          ))}
        </div>
      </section>

      <BoundAgentScopes
        entries={chatEntries}
        isLoading={chatsLoading}
        isError={chatsError}
        onRetry={onRetryChats}
      />
    </div>
  )
}
