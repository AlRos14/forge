import { useEffect, useMemo, useState } from 'react'
import { ShieldCheck } from '@phosphor-icons/react'
import { toast } from 'sonner'
import { Button } from '@/components/ui/button'
import { Select } from '@/components/ui/select'
import { Card } from '@/components/ui/card'
import { ErrorPanel, SectionKicker, StateBadge } from '@/features/federation/components'
import { numberValue } from '@/features/federation/format'
import {
  isVersionConflict,
  useAgentProfilesQuery,
  useMainAgentBindingQuery,
  useSetMainAgentBindingMutation,
} from '@/features/federation/hooks'
import type { FederatedAgent, MainAgentBindingInput } from '@/features/federation/types'
import { ApiError } from '@/api/client'

export function MainAgentBindingCard({
  agents,
  onConnect,
  onChangeModel,
}: {
  agents: FederatedAgent[]
  onConnect: () => void
  onChangeModel: (agent: FederatedAgent) => void
}) {
  const bindingQuery = useMainAgentBindingQuery()
  const setBinding = useSetMainAgentBindingMutation()
  const [identityId, setIdentityId] = useState('')
  const [profileId, setProfileId] = useState('')
  const [formError, setFormError] = useState<string | null>(null)
  const bindingMissing = bindingQuery.error instanceof ApiError && bindingQuery.error.status === 404
  const selectedAgent = agents.find((agent) => agent.id === identityId)
  const profilesQuery = useAgentProfilesQuery(identityId || undefined)
  const profiles = profilesQuery.data ?? []

  useEffect(() => {
    if (!bindingQuery.data) return
    setIdentityId(bindingQuery.data.identity_id)
    setProfileId(bindingQuery.data.profile_id)
  }, [bindingQuery.data])

  useEffect(() => {
    if (!selectedAgent || profileId) return
    setProfileId(selectedAgent.profile_id)
  }, [profileId, selectedAgent])

  const identityOptions = useMemo(() => {
    const options = agents.map((agent) => ({
      value: agent.id,
      label: `${agent.name} · ${agent.provider ?? agent.executor_type}`,
    }))
    const currentIdentity = bindingQuery.data?.identity_id
    if (currentIdentity && !agents.some((agent) => agent.id === currentIdentity)) {
      options.unshift({ value: currentIdentity, label: 'Current binding · unavailable in roster' })
    }
    return options
  }, [agents, bindingQuery.data?.identity_id])

  const profileOptions = profiles.map((profile) => ({
    value: profile.id,
    label: `${profile.provider ?? profile.executor_type} · ${profile.model ?? 'profile'}`,
  }))

  function chooseIdentity(nextIdentityId: string) {
    setIdentityId(nextIdentityId)
    setProfileId(agents.find((agent) => agent.id === nextIdentityId)?.profile_id ?? '')
    setFormError(null)
  }

  async function save() {
    if (!identityId || !profileId) {
      setFormError('Choose a Main Agent identity and profile before saving.')
      return
    }
    setFormError(null)
    const input: MainAgentBindingInput = {
      identity_id: identityId,
      profile_id: profileId,
      expected_version: numberValue(bindingQuery.data?.version, 0),
      autonomy_policy: bindingQuery.data?.autonomy_policy ?? {},
    }
    try {
      await setBinding.mutateAsync(input)
      toast.success('Main Agent binding saved')
    } catch (cause) {
      if (isVersionConflict(cause)) {
        setFormError('The Main Agent changed elsewhere. Refresh the binding and try again.')
        void bindingQuery.refetch()
        return
      }
      setFormError(
        cause instanceof Error ? cause.message : 'Main Agent binding could not be saved.',
      )
    }
  }

  if (bindingQuery.isLoading) {
    return (
      <Card className="border-border-subtle bg-card/70 p-5" role="status" aria-live="polite">
        Loading Main Agent binding…
      </Card>
    )
  }
  if (bindingQuery.isError && !bindingMissing) {
    return (
      <ErrorPanel
        title="Main Agent binding unavailable"
        description="Forge could not load the account's Main Agent binding. Retry before changing it."
        onRetry={() => void bindingQuery.refetch()}
      />
    )
  }

  return (
    <Card className="border-ember-border bg-ember-surface p-5">
      <div className="flex flex-wrap items-start justify-between gap-3">
        <div>
          <SectionKicker>Main Agent Chat</SectionKicker>
          <h2 className="mt-1 text-lg font-semibold tracking-tight text-foreground">
            One account-owned Main Agent binding
          </h2>
          <p className="mt-1 max-w-2xl text-sm leading-6 text-muted-foreground">
            Choose the one identity and profile that can answer the global timeline. Connecting an
            identity alone never binds it to Main.
          </p>
        </div>
        <StateBadge
          status={bindingQuery.data?.state ?? 'setup_required'}
          label={(bindingQuery.data?.state ?? 'setup required').replaceAll('_', ' ')}
        />
      </div>

      {agents.length === 0 ? (
        <div className="mt-5 rounded-md border border-border-subtle bg-background/60 px-3 py-3 text-sm text-muted-foreground">
          Connect an identity below before selecting a Main Agent.
        </div>
      ) : (
        <div className="mt-5 grid gap-4 md:grid-cols-2">
          <div className="space-y-2">
            <label
              htmlFor="main-agent-identity"
              className="font-mono text-micro font-semibold uppercase tracking-[0.8px] text-muted-foreground"
            >
              Identity
            </label>
            <Select
              id="main-agent-identity"
              value={identityId}
              options={identityOptions}
              placeholder="Select identity"
              onChange={chooseIdentity}
              aria-label="Main Agent identity"
            />
          </div>
          <div className="space-y-2">
            <label
              htmlFor="main-agent-profile"
              className="font-mono text-micro font-semibold uppercase tracking-[0.8px] text-muted-foreground"
            >
              Profile
            </label>
            <Select
              id="main-agent-profile"
              value={profileId}
              options={profileOptions}
              placeholder={profilesQuery.isLoading ? 'Loading profiles…' : 'Select profile'}
              onChange={setProfileId}
              disabled={!identityId || profilesQuery.isLoading || profileOptions.length === 0}
              aria-label="Main Agent profile"
            />
            {profilesQuery.isError ? (
              <p className="text-xs text-destructive" role="alert">
                Profiles are unavailable for this identity.
              </p>
            ) : null}
          </div>
        </div>
      )}

      <div className="mt-5 flex flex-wrap items-center gap-3 border-t border-ember-border pt-4">
        <div className="flex min-w-0 flex-1 items-start gap-2 text-xs text-muted-foreground">
          <ShieldCheck size={15} className="mt-0.5 shrink-0 text-primary" aria-hidden />
          <span>
            Server-enforced Main scope. Expected version{' '}
            {numberValue(bindingQuery.data?.version, 0)}.
          </span>
        </div>
        <Button
          onClick={() => void save()}
          disabled={setBinding.isPending || !identityId || !profileId || agents.length === 0}
        >
          {setBinding.isPending ? 'Saving…' : bindingMissing ? 'Set Main Agent' : 'Save Main Agent'}
        </Button>
        <Button
          type="button"
          variant="outline"
          disabled={!selectedAgent}
          onClick={() => selectedAgent && onChangeModel(selectedAgent)}
        >
          Change model…
        </Button>
        <Button type="button" variant="outline" onClick={onConnect}>
          Connect identity
        </Button>
      </div>
      {formError ? (
        <p
          className="mt-3 rounded-md border border-destructive/30 bg-destructive/5 px-3 py-2 text-xs text-destructive"
          role="alert"
        >
          {formError}
        </p>
      ) : null}
    </Card>
  )
}
