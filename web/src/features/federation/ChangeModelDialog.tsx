import { useEffect, useId, useMemo, useState } from 'react'
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
  useAgentProviderCapabilitiesQuery,
  useConnectEmbeddedProfileMutation,
  useMainAgentBindingQuery,
  useProjectAgentBindingQuery,
  useSelectAgentProfileMutation,
  useSetMainAgentBindingMutation,
  useSetProjectAgentBindingMutation,
} from '@/features/federation/hooks'
import type { FederatedAgent } from '@/features/federation/types'
import type { ProviderEntryResponse } from '@/types/generated'
import {
  DEFAULT_CEILING,
  DEFAULT_PROJECT_PERMISSION_CEILING,
  canPublishEmbeddedProfile,
  humanize,
  numberValue,
} from './format'

export type ChangeModelBindingContext =
  | { kind: 'main' }
  | { kind: 'project'; projectId: string; projectName?: string }

type Mode = 'existing' | 'new'

/**
 * Single dialog that replaces the old two-step "publish a profile" +
 * "select a profile" flow. It can either activate an already-published
 * profile, or publish a brand-new model on a provider entry and activate it
 * — and, when opened from a binding row, updates that binding to match.
 */
export function ChangeModelDialog({
  agent,
  entries,
  binding,
  onClose,
}: {
  agent: FederatedAgent | null
  entries: ProviderEntryResponse[]
  binding?: ChangeModelBindingContext
  onClose: () => void
}) {
  const formId = useId()
  const queryClient = useQueryClient()
  const canPublish = agent ? canPublishEmbeddedProfile(agent.backend_kind) : false
  const [mode, setMode] = useState<Mode>('existing')
  const [entryId, setEntryId] = useState('')
  const [name, setName] = useState('')
  const [description, setDescription] = useState('')
  const [model, setModel] = useState('')
  const [reasoningEffort, setReasoningEffort] = useState('')
  const [permissionPolicy, setPermissionPolicy] = useState('')
  const [profileId, setProfileId] = useState('')
  const [error, setError] = useState<string>()

  const activeEntries = entries.filter((entry) => entry.status === 'configured')
  const profilesQuery = useAgentProfilesQuery(agent?.id)
  const profiles = profilesQuery.data ?? []
  const otherProfiles = profiles.filter((profile) => profile.id !== agent?.profile_id)
  const capabilities = useAgentProviderCapabilitiesQuery()
  const discovered = useDiscoveredOptions(agent?.id, agent?.executor_type)

  const mainBindingQuery = useMainAgentBindingQuery()
  const projectBindingQuery = useProjectAgentBindingQuery(
    binding?.kind === 'project' ? binding.projectId : undefined,
  )
  const connectProfile = useConnectEmbeddedProfileMutation()
  const selectProfile = useSelectAgentProfileMutation(agent?.id ?? '')
  const setMainBinding = useSetMainAgentBindingMutation()
  const setProjectBinding = useSetProjectAgentBindingMutation(
    binding?.kind === 'project' ? binding.projectId : '',
  )
  const updateAgent = useUpdateAgent()

  useEffect(() => {
    if (!agent) return
    setMode('new')
    setEntryId(agent.credential_handle_id ?? activeEntries[0]?.id ?? '')
    setName(agent.name)
    setDescription(agent.description ?? '')
    setModel(agent.model ?? '')
    setReasoningEffort(agent.reasoning_effort ?? '')
    setPermissionPolicy(agent.permission_policy ?? '')
    setProfileId(otherProfiles[0]?.id ?? '')
    setError(undefined)
    // Reset only when the target agent identity changes.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [agent?.id])

  const selectedEntryCapability = useMemo(() => {
    const entry = activeEntries.find((candidate) => candidate.id === entryId)
    if (!entry) return undefined
    return capabilities.data?.items.find((item) => item.provider === entry.provider)
  }, [activeEntries, capabilities.data?.items, entryId])

  const modelSuggestions = discovered.data?.models ?? []
  const reasoningOptionsForModel = useMemo(
    () => getReasoningOptionsForModel(discovered.data, model),
    [discovered.data, model],
  )
  const pending =
    connectProfile.isPending ||
    selectProfile.isPending ||
    setMainBinding.isPending ||
    setProjectBinding.isPending ||
    updateAgent.isPending

  useEffect(() => {
    if (!reasoningEffort || !discovered.data) return
    if (!reasoningOptionsForModel.some((option) => option.id === reasoningEffort)) {
      setReasoningEffort('')
    }
  }, [discovered.data, reasoningEffort, reasoningOptionsForModel])

  async function applyBindingIfNeeded(finalProfileId: string) {
    if (!agent || !binding) return
    if (binding.kind === 'main') {
      await setMainBinding.mutateAsync({
        identity_id: agent.id,
        profile_id: finalProfileId,
        expected_version: numberValue(mainBindingQuery.data?.version, 0),
        autonomy_policy: mainBindingQuery.data?.autonomy_policy ?? {},
      })
      return
    }
    await setProjectBinding.mutateAsync({
      identity_id: agent.id,
      profile_id: finalProfileId,
      expected_version: numberValue(projectBindingQuery.data?.version, 0),
      permission_ceiling: projectBindingQuery.data?.permission_ceiling ?? DEFAULT_PROJECT_PERMISSION_CEILING,
      autonomy_policy: projectBindingQuery.data?.autonomy_policy ?? {},
      subscriptions: projectBindingQuery.data?.subscriptions ?? [],
      wake_budget: numberValue(projectBindingQuery.data?.wake_budget, 3),
    })
  }

  async function submit(event: React.FormEvent<HTMLFormElement>) {
    event.preventDefault()
    if (!agent) return
    if (!binding && !name.trim()) {
      setError('A name is required.')
      return
    }
    const profileChanged =
      canPublish &&
      (entryId !== (agent.credential_handle_id ?? '') || model.trim() !== (agent.model ?? ''))
    if (mode === 'existing' && !profileId) {
      setError('Choose a profile to activate.')
      return
    }
    if (mode === 'new' && (!model.trim() || (profileChanged && !entryId))) {
      setError(canPublish ? 'A provider entry and a model are required.' : 'A model is required.')
      return
    }
    setError(undefined)
    try {
      let currentVersion = agent.version
      const metadataChanged =
        !binding &&
        (name.trim() !== agent.name ||
          (description.trim() ? description.trim() : null) !== agent.description)

      if (canPublish && metadataChanged) {
        const updated = await updateAgent.mutateAsync({
          agentId: agent.id,
          body: {
            name: name.trim(),
            description: description.trim() ? description.trim() : null,
            version: currentVersion,
          },
        })
        currentVersion = updated.version
      }

      if (mode === 'new' && canPublish && !profileChanged) {
        onClose()
        return
      }

      if (mode === 'existing') {
        if (!canPublish && metadataChanged) {
          const updated = await updateAgent.mutateAsync({
            agentId: agent.id,
            body: {
              name: name.trim(),
              description: description.trim() ? description.trim() : null,
              version: currentVersion,
            },
          })
          currentVersion = updated.version
        }
        await selectProfile.mutateAsync({ profileId, version: currentVersion })
        await applyBindingIfNeeded(profileId)
      } else if (canPublish) {
        const connected = await connectProfile.mutateAsync({
          identityId: agent.id,
          input: {
            version: currentVersion,
            credential_id: entryId,
            model: model.trim(),
            permission_policy: 'scoped_proposals',
            tool_policy: DEFAULT_CEILING,
          },
        })
        await selectProfile.mutateAsync({
          profileId: connected.profile.id,
          version: connected.agent.version,
        })
        await applyBindingIfNeeded(connected.profile.id)
      } else {
        await updateAgent.mutateAsync({
          agentId: agent.id,
          body: {
            name: name.trim(),
            description: description.trim() ? description.trim() : null,
            model: model.trim(),
            reasoning_effort: reasoningEffort.trim() ? reasoningEffort.trim() : null,
            permission_policy: permissionPolicy.trim() ? permissionPolicy.trim() : null,
            version: currentVersion,
          },
        })
        void queryClient.invalidateQueries({ queryKey: federationQueryKeys.agents })
      }
      onClose()
    } catch (cause) {
      setError(
        isVersionConflict(cause)
          ? 'This agent or binding changed in another session. Refresh and try again.'
          : cause instanceof Error
            ? cause.message
            : 'The model could not be changed.',
      )
    }
  }

  return (
    <Dialog open={Boolean(agent)} onOpenChange={(open) => !open && onClose()}>
      <DialogContent className="max-w-lg">
        <form id={formId} onSubmit={submit}>
          <DialogHeader>
            <SectionKicker>{binding ? 'Change model' : 'Edit agent'}</SectionKicker>
            <DialogTitle className="mt-1">
              {agent?.name ?? 'this agent'}
              {binding?.kind === 'main' ? ' · Main Agent Chat' : null}
              {binding?.kind === 'project' ? ` · ${binding.projectName ?? 'Project Agent'}` : null}
            </DialogTitle>
            <DialogDescription>
              {binding
                ? 'Activating a profile here also updates this binding, preserving its chat timeline.'
                : 'Update the agent identity and its harness defaults. Direct-agent model changes publish a new profile.'}
            </DialogDescription>
          </DialogHeader>

          <div className="mt-5 space-y-4">
            {!binding ? (
              <div className="grid gap-4 sm:grid-cols-2">
                <div className="space-y-2">
                  <Label htmlFor="edit-agent-name">Agent name</Label>
                  <Input
                    id="edit-agent-name"
                    value={name}
                    onChange={(event) => setName(event.target.value)}
                    required
                  />
                </div>
                <div className="space-y-2">
                  <Label htmlFor="edit-agent-description">Description</Label>
                  <Input
                    id="edit-agent-description"
                    value={description}
                    onChange={(event) => setDescription(event.target.value)}
                    placeholder="What this agent is for"
                  />
                </div>
              </div>
            ) : null}
            <div className="flex gap-1.5 rounded-md border border-border-subtle bg-muted/30 p-1" role="tablist" aria-label="Change model mode">
              <button
                type="button"
                role="tab"
                aria-selected={mode === 'existing'}
                className={`flex-1 rounded px-3 py-1.5 text-xs font-medium transition-colors ${mode === 'existing' ? 'bg-card text-foreground shadow-sm' : 'text-muted-foreground hover:text-foreground'}`}
                onClick={() => setMode('existing')}
              >
                Pick existing profile
              </button>
              <button
                type="button"
                role="tab"
                aria-selected={mode === 'new'}
                className={`flex-1 rounded px-3 py-1.5 text-xs font-medium transition-colors ${mode === 'new' ? 'bg-card text-foreground shadow-sm' : 'text-muted-foreground hover:text-foreground'}`}
                onClick={() => setMode('new')}
              >
                {canPublish ? 'New model on an entry' : 'Update model'}
              </button>
            </div>

            {mode === 'existing' ? (
              <div className="space-y-2">
                <Label htmlFor="change-model-profile">Profile</Label>
                {profilesQuery.isLoading ? (
                  <p className="text-xs text-muted-foreground">Loading profiles…</p>
                ) : otherProfiles.length === 0 ? (
                  <p className="text-xs text-muted-foreground">
                    No other profile exists yet for this agent — use the other tab to{' '}
                    {canPublish ? 'publish a new model' : 'change the model directly'}.
                  </p>
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
            ) : canPublish ? (
              <>
                <div className="space-y-2">
                  <Label htmlFor="change-model-entry">Provider entry</Label>
                  <Select
                    id="change-model-entry"
                    value={entryId}
                    placeholder={activeEntries.length === 0 ? 'No connected entries' : 'Select entry'}
                    onChange={setEntryId}
                    disabled={activeEntries.length === 0}
                    options={activeEntries.map((entry) => ({
                      value: entry.id,
                      label: `${humanize(entry.provider)} · ${entry.label}`,
                    }))}
                  />
                </div>
                <div className="space-y-2">
                  <Label htmlFor="change-model-model">Model</Label>
                  <Input
                    id="change-model-model"
                    list={`${formId}-model-suggestions`}
                    value={model}
                    onChange={(event) => setModel(event.target.value)}
                    placeholder={selectedEntryCapability?.default_model ?? 'e.g. gpt-5'}
                    required
                  />
                  {modelSuggestions.length > 0 ? (
                    <datalist id={`${formId}-model-suggestions`}>
                      {modelSuggestions.map((suggestion) => (
                        <option key={suggestion.id} value={suggestion.id}>
                          {suggestion.displayName}
                        </option>
                      ))}
                    </datalist>
                  ) : null}
                </div>
              </>
            ) : (
              <>
                <ModelSelector
                  id="change-model-model"
                  models={modelSuggestions}
                  recentModelIds={[]}
                  value={model || null}
                  isLoading={discovered.isLoading}
                  hasError={discovered.isError}
                  onChange={(value) => setModel(value ?? '')}
                />
                {reasoningOptionsForModel.length > 0 ? (
                  <ReasoningSelector
                    id="change-model-reasoning"
                    options={reasoningOptionsForModel}
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

            {error ? (
              <p role="alert" className="text-xs text-destructive">
                {error}
              </p>
            ) : null}
          </div>

          <DialogFooter className="mt-6 gap-2">
            <Button type="button" variant="ghost" onClick={onClose}>
              Cancel
            </Button>
            <Button type="submit" disabled={pending}>
              <ShieldCheck size={15} aria-hidden />
              {pending ? 'Applying…' : binding ? 'Change model' : 'Save changes'}
            </Button>
          </DialogFooter>
        </form>
      </DialogContent>
    </Dialog>
  )
}
