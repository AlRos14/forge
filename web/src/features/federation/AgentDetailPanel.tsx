import { Robot } from '@phosphor-icons/react'
import { useState } from 'react'
import { Button } from '@/components/ui/button'
import { CollapsibleSection } from '@/components/ui/collapsible-section'
import type { AgentChatEntry } from '@/features/agent-chat/types'
import { useAgentProfilesQuery, useSelectAgentProfileMutation, isVersionConflict } from '@/features/federation/hooks'
import type { FederatedAgent } from '@/features/federation/types'
import type { ProviderEntryResponse } from '@/types/generated'
import { StateBadge, StatusDot } from '@/features/federation/components'
import { allowedPolicyValues, humanize, runtimeDisplayNames } from './format'

export function AgentDetailPanel({
  agent,
  entries,
  chatEntries,
  onChangeModel,
}: {
  agent: FederatedAgent
  entries: ProviderEntryResponse[]
  chatEntries: AgentChatEntry[]
  onChangeModel: (agent: FederatedAgent) => void
}) {
  const profilesQuery = useAgentProfilesQuery(agent.id)
  const selectProfile = useSelectAgentProfileMutation(agent.id)
  const [profileError, setProfileError] = useState<string | null>(null)
  const profiles = profilesQuery.data ?? []
  const selectedProfile = profiles.find((profile) => profile.id === agent.profile_id)
  const selectedEntry = entries.find((entry) => entry.id === selectedProfile?.credential_handle_id)
  const requiresRecovery =
    selectedEntry != null && (selectedEntry.status === 'revoked' || selectedEntry.status === 'invalid')
  const connectionStatus = requiresRecovery
    ? 'recovery_required'
    : (agent.effective_status ?? agent.status)
  const runtime = agent.executor_type === 'embedded' ? 'direct' : agent.executor_type

  const boundChips = chatEntries
    .filter((entry) => entry.identity_id === agent.id)
    .map((entry) => (entry.kind === 'main' ? 'Main Agent' : (entry.project_name ?? 'Project Agent')))

  async function selectProfileVersion(profileId: string) {
    setProfileError(null)
    try {
      await selectProfile.mutateAsync({ profileId, version: agent.version })
    } catch (cause) {
      setProfileError(
        isVersionConflict(cause)
          ? 'Agent changed in another session. Refresh the roster before selecting a profile.'
          : cause instanceof Error
            ? cause.message
            : 'Profile selection failed.',
      )
    }
  }

  return (
    <div className="flex flex-1 flex-col overflow-y-auto">
      <header className="flex shrink-0 items-start gap-4 border-b border-border-subtle px-6 py-4">
        <div className="flex h-10 w-10 shrink-0 items-center justify-center rounded-lg bg-ember-surface text-primary">
          <Robot size={20} weight="duotone" aria-hidden />
        </div>
        <div className="min-w-0 flex-1">
          <div className="flex flex-wrap items-center gap-2">
            <h2 className="truncate text-lg font-semibold text-foreground">{agent.name}</h2>
            <StateBadge status={agent.effective_status ?? agent.status} label={humanize(agent.effective_status ?? agent.status)} />
          </div>
          <p className="mt-1 truncate font-mono text-xs text-muted-foreground">
            {runtimeDisplayNames[runtime] ?? humanize(runtime)}
            {selectedEntry ? ` · ${selectedEntry.label}` : ''}
          </p>
          {boundChips.length > 0 ? (
            <div className="mt-2 flex flex-wrap gap-1.5">
              {boundChips.map((chip) => (
                <span
                  key={chip}
                  className="rounded-full border border-ember-border bg-ember-surface px-2 py-0.5 font-mono text-micro font-semibold uppercase tracking-[0.6px] text-primary"
                >
                  {chip}
                </span>
              ))}
            </div>
          ) : null}
        </div>
      </header>

      <div className="flex-1 space-y-6 px-6 py-5">
        {/* Stat grid */}
        <div className="grid grid-cols-2 gap-2.5 sm:grid-cols-4">
          {[
            { label: 'Model', value: agent.model ?? 'Pending' },
            { label: 'Provider', value: agent.provider ? humanize(agent.provider) : 'CLI-managed' },
            {
              label: agent.reasoning_effort ? 'Reasoning' : 'Permission',
              value: agent.reasoning_effort
                ? humanize(agent.reasoning_effort)
                : agent.permission_policy
                  ? humanize(agent.permission_policy)
                  : '—',
            },
            { label: 'Total runs', value: agent.total_runs },
          ].map((stat) => (
            <div key={stat.label} className="rounded-lg border border-border-subtle bg-muted/40 px-3.5 py-3">
              <p className="mb-2 font-mono text-micro font-semibold uppercase tracking-[0.8px] text-muted-foreground">
                {stat.label}
              </p>
              <p className="truncate font-mono text-lg font-semibold tabular-nums text-foreground" title={String(stat.value)}>
                {stat.value}
              </p>
            </div>
          ))}
        </div>

        <div>
          <Button onClick={() => onChangeModel(agent)}>Change model…</Button>
        </div>

        {/* Profiles */}
        <section aria-labelledby="agent-detail-profiles-heading">
          <h3
            id="agent-detail-profiles-heading"
            className="mb-3 font-mono text-micro font-semibold uppercase tracking-[0.8px] text-muted-foreground"
          >
            Profiles
          </h3>
          {profilesQuery.isLoading ? (
            <p className="text-xs text-muted-foreground">Loading profiles…</p>
          ) : null}
          {profilesQuery.isError ? (
            <p className="text-xs text-destructive">Profiles are unavailable for this agent.</p>
          ) : null}
          {profileError ? (
            <p className="mt-2 text-xs text-destructive" role="alert">
              {profileError}
            </p>
          ) : null}
          <div className="space-y-2">
            {profiles.map((profile) => (
              <div
                key={profile.id}
                className="flex items-center justify-between gap-3 rounded-md border border-border-subtle bg-card px-3 py-2"
              >
                <div className="min-w-0">
                  <p className="truncate text-xs font-medium text-foreground">
                    {profile.provider ?? profile.executor_type} · {profile.model ?? 'unknown model'}
                  </p>
                  <p className="mt-0.5 font-mono text-micro text-muted-foreground">
                    v{profile.version} · {profile.id === agent.profile_id ? 'selected' : 'available'}
                  </p>
                </div>
                {profile.id !== agent.profile_id ? (
                  <Button
                    size="sm"
                    variant="ghost"
                    disabled={selectProfile.isPending}
                    onClick={() => void selectProfileVersion(profile.id)}
                  >
                    Select
                  </Button>
                ) : (
                  <span className="font-mono text-micro uppercase text-primary">Current</span>
                )}
              </div>
            ))}
            {profiles.length === 0 && !profilesQuery.isLoading ? (
              <p className="text-xs text-muted-foreground">No profiles published yet.</p>
            ) : null}
          </div>
        </section>

        {agent.description ? (
          <p className="border-t border-border-subtle pt-4 text-xs leading-5 text-muted-foreground">
            {agent.description}
          </p>
        ) : null}

        {/* Collapsed identity facts */}
        <CollapsibleSection title="Identity details" className="border-t border-border-subtle pt-4">
          <dl className="mt-2 space-y-2 text-xs">
            <div className="flex justify-between gap-4">
              <dt className="text-muted-foreground">Stable ID</dt>
              <dd className="truncate font-mono text-foreground">{agent.id}</dd>
            </div>
            <div className="flex justify-between gap-4">
              <dt className="text-muted-foreground">Current profile</dt>
              <dd className="truncate font-mono text-foreground">{agent.profile_id}</dd>
            </div>
            <div className="flex justify-between gap-4">
              <dt className="text-muted-foreground">Connection</dt>
              <dd className="inline-flex items-center gap-1.5 text-foreground">
                <StatusDot status={connectionStatus} />
                {humanize(connectionStatus)}
              </dd>
            </div>
          </dl>
          {requiresRecovery ? (
            <p
              className="mt-3 rounded-md border border-warning/30 bg-warning/10 px-3 py-2 text-xs text-warning"
              role="status"
            >
              This agent&apos;s provider entry is disconnected. Publish a profile on another entry
              before relying on its Main or Project binding.
            </p>
          ) : null}
          <div className="mt-4 border-t border-border-subtle pt-3">
            <p className="text-xs text-muted-foreground">Profile tool ceiling</p>
            <div className="mt-2 flex flex-wrap gap-1.5" aria-label="Profile tool ceiling">
              {allowedPolicyValues(selectedProfile?.tool_policy).length > 0 ? (
                allowedPolicyValues(selectedProfile?.tool_policy).map((capability) => (
                  <span key={capability} className="rounded bg-muted px-2 py-1 font-mono text-micro text-foreground">
                    {capability}
                  </span>
                ))
              ) : (
                <span className="text-xs text-muted-foreground">Unavailable in this projection.</span>
              )}
            </div>
            <p className="mt-2 text-micro leading-5 text-muted-foreground">
              This is a ceiling, not a grant. Effective permissions are recomputed for each account,
              Main Agent Chat, Project Agent Chat, or Task scope.
            </p>
          </div>
        </CollapsibleSection>
      </div>
    </div>
  )
}
