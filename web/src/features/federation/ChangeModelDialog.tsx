import { useEffect, useState } from 'react'
import { useQueryClient } from '@tanstack/react-query'
import { ShieldCheck } from '@phosphor-icons/react'
import { useUpdateAgent } from '@/api/hooks'
import { ModelSelector } from '@/components/execution-config/ModelSelector'
import { PolicySelector } from '@/components/execution-config/PolicySelector'
import { ReasoningSelector } from '@/components/execution-config/ReasoningSelector'
import { Button } from '@/components/ui/button'
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from '@/components/ui/dialog'
import { Input } from '@/components/ui/input'
import { Label } from '@/components/ui/label'
import { Select } from '@/components/ui/select'
import { getReasoningOptionsForModel, useDiscoveredOptions } from '@/hooks/useDiscoveredOptions'
import { SectionKicker } from '@/features/federation/components'
import { useAuthStore } from '@/stores/auth'
import {
  federationQueryKeys,
  isVersionConflict,
  useAgentProfilesQuery,
  useSelectAgentProfileMutation,
} from '@/features/federation/hooks'
import type { FederatedAgent } from '@/features/federation/types'
import { humanize } from './format'

type Mode = 'existing' | 'update'

/** Edit an external harness Agent or select one exact, already published profile. */
export function ChangeModelDialog({
  agent,
  onClose,
}: {
  agent: FederatedAgent | null
  onClose: () => void
}) {
  const queryClient = useQueryClient()
  const isAdmin = useAuthStore((state) => Boolean(state.user?.is_admin))
  const currentUserId = useAuthStore((state) => state.user?.id)
  const [mode, setMode] = useState<Mode>('update')
  const [name, setName] = useState('')
  const [description, setDescription] = useState('')
  const [model, setModel] = useState('')
  const [reasoningEffort, setReasoningEffort] = useState('')
  const [permissionPolicy, setPermissionPolicy] = useState('')
  const [daemonId, setDaemonId] = useState('')
  const [profileId, setProfileId] = useState('')
  const [error, setError] = useState<string>()

  const profilesQuery = useAgentProfilesQuery(agent?.id)
  const profiles = profilesQuery.data ?? []
  const otherProfiles = profiles.filter((profile) => profile.id !== agent?.profile_id)
  const selectProfile = useSelectAgentProfileMutation(agent?.id ?? '')
  const updateAgent = useUpdateAgent()
  const discovered = useDiscoveredOptions(agent?.id ?? null, agent?.executor_type)
  const reasoningOptions = getReasoningOptionsForModel(discovered.data, model)
  const availableDaemons = discovered.data?.availableDaemons ?? []
  const canPinHost = isAdmin && agent?.owner_id === currentUserId

  useEffect(() => {
    if (!agent) return
    setMode('update')
    setName(agent.name)
    setDescription(agent.description ?? '')
    setModel(agent.model ?? '')
    setReasoningEffort(agent.reasoning_effort ?? '')
    setPermissionPolicy(agent.permission_policy ?? '')
    setDaemonId(agent.daemon_id ?? '')
    setProfileId('')
    setError(undefined)
  }, [agent?.id])

  async function submit(event: React.FormEvent<HTMLFormElement>) {
    event.preventDefault()
    if (!agent) return
    if (!name.trim()) {
      setError('An Agent name is required.')
      return
    }
    if (mode === 'existing' && !profileId) {
      setError('Choose an exact profile to activate.')
      return
    }
    setError(undefined)
    try {
      const nextName = name.trim()
      const nextDescription = description.trim() ? description.trim() : null
      const nextDaemonId = daemonId || null
      const hostChanged = canPinHost && nextDaemonId !== agent.daemon_id
      const metadataChanged =
        nextName !== agent.name || nextDescription !== agent.description || hostChanged

      if (mode === 'existing') {
        let version = agent.version
        if (metadataChanged) {
          const body: {
            version: number
            name?: string
            description?: string | null
            daemon_id?: string | null
          } = { version }
          if (nextName !== agent.name) body.name = nextName
          if (nextDescription !== agent.description) body.description = nextDescription
          if (hostChanged) body.daemon_id = nextDaemonId
          const updated = await updateAgent.mutateAsync({
            agentId: agent.id,
            body,
          })
          version = updated.version
        }
        await selectProfile.mutateAsync({ profileId, version })
      } else {
        const nextModel = model.trim() ? model.trim() : null
        const nextReasoningEffort = reasoningEffort.trim() || null
        const nextPermissionPolicy = permissionPolicy.trim() || null
        const body: {
          version: number
          name?: string
          description?: string | null
          model?: string | null
          reasoning_effort?: string | null
          permission_policy?: string | null
          daemon_id?: string | null
        } = { version: agent.version }
        if (nextName !== agent.name) body.name = nextName
        if (nextDescription !== agent.description) body.description = nextDescription
        if (nextModel !== agent.model) body.model = nextModel
        if (nextReasoningEffort !== agent.reasoning_effort) {
          body.reasoning_effort = nextReasoningEffort
        }
        if (nextPermissionPolicy !== agent.permission_policy) {
          body.permission_policy = nextPermissionPolicy
        }
        if (hostChanged) body.daemon_id = nextDaemonId
        await updateAgent.mutateAsync({
          agentId: agent.id,
          body,
        })
      }
      void queryClient.invalidateQueries({ queryKey: federationQueryKeys.agents })
      onClose()
    } catch (cause) {
      setError(
        isVersionConflict(cause)
          ? 'This Agent changed in another session. Refresh and try again.'
          : cause instanceof Error
            ? cause.message
            : 'The Agent update failed.',
      )
    }
  }

  const pending = selectProfile.isPending || updateAgent.isPending
  const hostOptions = [
    { value: '', label: 'Automatically resolve at Execution time' },
    ...(agent?.daemon_id && !availableDaemons.some((daemon) => daemon.id === agent.daemon_id)
      ? [{ value: agent.daemon_id, label: `Current host unavailable · ${agent.daemon_id}`, disabled: true }]
      : []),
    ...availableDaemons.map((daemon) => ({
      value: daemon.id,
      label: `${daemon.name} · ${humanize(daemon.status)}`,
    })),
  ]

  return (
    <Dialog open={Boolean(agent)} onOpenChange={(open) => !open && onClose()}>
      <DialogContent className="max-w-lg">
        <form onSubmit={submit}>
          <DialogHeader>
            <SectionKicker>Edit external harness Agent</SectionKicker>
            <DialogTitle className="mt-1">{agent?.name ?? 'Agent'}</DialogTitle>
            <DialogDescription>
              Update the Agent&apos;s harness defaults or select an exact published profile. Forge does
              not run a model loop for this Agent.
            </DialogDescription>
          </DialogHeader>

          <div className="mt-5 space-y-4">
            <div className="grid gap-4 sm:grid-cols-2">
              <div className="space-y-2">
                <Label htmlFor="edit-agent-name">Agent name</Label>
                <Input id="edit-agent-name" value={name} onChange={(event) => setName(event.target.value)} required />
              </div>
              <div className="space-y-2">
                <Label htmlFor="edit-agent-description">Description</Label>
                <Input id="edit-agent-description" value={description} onChange={(event) => setDescription(event.target.value)} />
              </div>
            </div>

            <div className="flex gap-1.5 rounded-md border border-border-subtle bg-muted/30 p-1" role="tablist" aria-label="Agent configuration mode">
              <button type="button" role="tab" aria-selected={mode === 'update'} onClick={() => setMode('update')} className="flex-1 rounded px-3 py-1.5 text-xs font-medium">Update harness defaults</button>
              <button type="button" role="tab" aria-selected={mode === 'existing'} onClick={() => setMode('existing')} className="flex-1 rounded px-3 py-1.5 text-xs font-medium">Select existing profile</button>
            </div>

            {canPinHost ? (
              <div className="space-y-2">
                <Label htmlFor="edit-agent-host">Execution host</Label>
                <Select
                  id="edit-agent-host"
                  value={daemonId}
                  placeholder="Select an execution host"
                  onChange={setDaemonId}
                  options={hostOptions}
                />
                <p className="text-xs text-muted-foreground">
                  Optional advanced placement. Leave automatic unless this Agent must run on a
                  specific authorized host. Automatic routing resolves an authorized host when an
                  Execution starts.
                </p>
              </div>
            ) : null}

            {mode === 'existing' ? (
              <div className="space-y-2">
                <Label htmlFor="change-model-profile">Profile</Label>
                {profilesQuery.isLoading ? (
                  <p className="text-xs text-muted-foreground">Loading profiles…</p>
                ) : otherProfiles.length === 0 ? (
                  <p className="text-xs text-muted-foreground">No other published profile exists for this Agent.</p>
                ) : (
                  <Select
                    id="change-model-profile"
                    value={profileId}
                    placeholder="Select a profile"
                    onChange={setProfileId}
                    options={otherProfiles.map((profile) => ({
                      value: profile.id,
                      label: `${humanize(profile.provider ?? profile.executor_type)} · ${profile.model ?? 'unknown model'} · v${profile.version}`,
                    }))}
                  />
                )}
              </div>
            ) : (
              <>
                <ModelSelector
                  id="change-model-model"
                  models={discovered.data?.models ?? []}
                  recentModelIds={[]}
                  value={model || null}
                  isLoading={discovered.isLoading}
                  hasError={discovered.isError}
                  warning={discovered.data?.warning}
                  onChange={(value) => setModel(value ?? '')}
                />
                {reasoningOptions.length > 0 ? (
                  <ReasoningSelector
                    id="change-model-reasoning"
                    label={agent?.executor_type === 'claude_code' ? 'Effort' : 'Reasoning'}
                    placeholder="Use harness default"
                    options={reasoningOptions}
                    value={reasoningEffort || null}
                    isLoading={discovered.isLoading}
                    hasError={discovered.isError}
                    onChange={(value) => setReasoningEffort(value ?? '')}
                  />
                ) : null}
                {(discovered.data?.permissionPolicies.length ?? 0) > 0 ? (
                  <PolicySelector
                    id="change-model-permission-policy"
                    policies={discovered.data?.permissionPolicies}
                    value={permissionPolicy || null}
                    onChange={(value) => setPermissionPolicy(value ?? '')}
                  />
                ) : null}
              </>
            )}
            {error ? <p role="alert" className="text-xs text-destructive">{error}</p> : null}
          </div>

          <DialogFooter className="mt-6 gap-2">
            <Button type="button" variant="ghost" onClick={onClose}>Cancel</Button>
            <Button type="submit" disabled={pending}>
              <ShieldCheck size={15} aria-hidden />
              {pending ? 'Saving…' : mode === 'existing' ? 'Select profile' : 'Save changes'}
            </Button>
          </DialogFooter>
        </form>
      </DialogContent>
    </Dialog>
  )
}
