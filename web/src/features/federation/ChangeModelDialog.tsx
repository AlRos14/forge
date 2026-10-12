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
  const [mode, setMode] = useState<Mode>('update')
  const [name, setName] = useState('')
  const [description, setDescription] = useState('')
  const [model, setModel] = useState('')
  const [reasoningEffort, setReasoningEffort] = useState('')
  const [permissionPolicy, setPermissionPolicy] = useState('')
  const [profileId, setProfileId] = useState('')
  const [error, setError] = useState<string>()

  const profilesQuery = useAgentProfilesQuery(agent?.id)
  const profiles = profilesQuery.data ?? []
  const otherProfiles = profiles.filter((profile) => profile.id !== agent?.profile_id)
  const selectProfile = useSelectAgentProfileMutation(agent?.id ?? '')
  const updateAgent = useUpdateAgent()
  const discovered = useDiscoveredOptions(agent?.id ?? null, agent?.executor_type)
  const reasoningOptions = getReasoningOptionsForModel(discovered.data, model)

  useEffect(() => {
    if (!agent) return
    setMode('update')
    setName(agent.name)
    setDescription(agent.description ?? '')
    setModel(agent.model ?? '')
    setReasoningEffort(agent.reasoning_effort ?? '')
    setPermissionPolicy(agent.permission_policy ?? '')
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
    if (mode === 'update' && !model.trim()) {
      setError('A model is required.')
      return
    }

    setError(undefined)
    try {
      const metadataChanged =
        name.trim() !== agent.name ||
        (description.trim() ? description.trim() : null) !== agent.description

      if (mode === 'existing') {
        let version = agent.version
        if (metadataChanged) {
          const updated = await updateAgent.mutateAsync({
            agentId: agent.id,
            body: {
              name: name.trim(),
              description: description.trim() ? description.trim() : null,
              version,
            },
          })
          version = updated.version
        }
        await selectProfile.mutateAsync({ profileId, version })
      } else {
        await updateAgent.mutateAsync({
          agentId: agent.id,
          body: {
            name: name.trim(),
            description: description.trim() ? description.trim() : null,
            model: model.trim(),
            reasoning_effort: reasoningEffort.trim() || null,
            permission_policy: permissionPolicy.trim() || null,
            version: agent.version,
          },
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

  return (
    <Dialog open={Boolean(agent)} onOpenChange={(open) => !open && onClose()}>
      <DialogContent className="max-w-lg">
        <form onSubmit={submit}>
          <DialogHeader>
            <SectionKicker>Edit external harness Agent</SectionKicker>
            <DialogTitle className="mt-1">{agent?.name ?? 'Agent'}</DialogTitle>
            <DialogDescription>
              Update the Agent's harness defaults or select an exact published profile. Forge does
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
                  onChange={(value) => setModel(value ?? '')}
                />
                {reasoningOptions.length > 0 ? (
                  <ReasoningSelector
                    id="change-model-reasoning"
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
