import { useEffect, useMemo, useState } from 'react'
import { ArrowsClockwise, Trash } from '@phosphor-icons/react'
import { toast } from 'sonner'
import {
  useCreateIntegration,
  useDeleteIntegration,
  useIntegrationQuery,
  useUpdateIntegration,
  useTriggerSync,
  type CreateIntegrationRequest,
  type PatchIntegrationRequest,
} from '@/api/hooks'
import { parseGitRemoteUrl } from '@/components/settings/integration-utils'
import { SettingsSection } from '@/components/settings/SettingsSection'
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
import { Skeleton } from '@/components/ui/skeleton'
import { Switch } from '@/components/ui/switch'
import { getApiErrorMessage } from '@/lib/api-error'
import { isApiStatus } from '@/lib/api-error'

const PLATFORM_OPTIONS = [
  { value: 'github', label: 'GitHub' },
  { value: 'gitea', label: 'Gitea' },
]

const DEFAULT_BASE_URLS: Record<string, string> = {
  github: 'https://api.github.com',
  gitea: '',
}

interface IntegrationsTabProps {
  projectId: string
  repoRemoteUrl?: string | null
  embedded?: boolean
}

export function IntegrationsTab({
  projectId,
  repoRemoteUrl,
  embedded = false,
}: IntegrationsTabProps) {
  const integrationQuery = useIntegrationQuery(projectId)
  const createIntegration = useCreateIntegration()
  const updateIntegration = useUpdateIntegration()
  const deleteIntegration = useDeleteIntegration()
  const triggerSync = useTriggerSync()

  const is404 = isApiStatus(integrationQuery.error, 404)
  const hasIntegration = integrationQuery.isSuccess && integrationQuery.data != null
  const parsedRemote = useMemo(() => parseGitRemoteUrl(repoRemoteUrl), [repoRemoteUrl])

  // Form state
  const [platform, setPlatform] = useState('github')
  const [baseUrl, setBaseUrl] = useState('https://api.github.com')
  const [owner, setOwner] = useState('')
  const [repo, setRepo] = useState('')
  const [tokenSecretRef, setTokenSecretRef] = useState('')
  const [pollInterval, setPollInterval] = useState('300')
  const [enabled, setEnabled] = useState(true)
  const [confirmingDelete, setConfirmingDelete] = useState(false)

  const resetToRepoDefaults = () => {
    setPlatform(parsedRemote?.platform ?? 'github')
    setBaseUrl(parsedRemote?.baseUrl ?? 'https://api.github.com')
    setOwner(parsedRemote?.owner ?? '')
    setRepo(parsedRemote?.repo ?? '')
    setTokenSecretRef('')
    setPollInterval('300')
    setEnabled(true)
  }

  // Populate form when integration loads
  useEffect(() => {
    if (!integrationQuery.data) return
    const d = integrationQuery.data
    setPlatform(d.platform)
    setBaseUrl(d.base_url)
    setOwner(d.owner)
    setRepo(d.repo)
    setTokenSecretRef(d.token_secret_ref)
    setPollInterval(String(d.poll_interval_secs))
    setEnabled(d.enabled)
  }, [integrationQuery.data])

  // Seed new integrations from the repository's saved remote URL.
  useEffect(() => {
    if (integrationQuery.data || !parsedRemote) return
    setPlatform(parsedRemote.platform)
    setBaseUrl(parsedRemote.baseUrl)
    setOwner(parsedRemote.owner)
    setRepo(parsedRemote.repo)
  }, [integrationQuery.data, parsedRemote])

  const handlePlatformChange = (value: string) => {
    setPlatform(value)
    if (!hasIntegration) {
      setBaseUrl(DEFAULT_BASE_URLS[value] ?? '')
    }
  }

  const handleSave = () => {
    const trimmedOwner = owner.trim()
    const trimmedRepo = repo.trim()
    const trimmedUrl = baseUrl.trim()
    const trimmedToken = tokenSecretRef.trim()
    const interval = Number(pollInterval)

    if (!trimmedUrl) {
      toast.error('Base URL is required')
      return
    }
    if (!trimmedOwner) {
      toast.error('Owner is required')
      return
    }
    if (!trimmedRepo) {
      toast.error('Repo is required')
      return
    }
    if (!trimmedToken) {
      toast.error('Token secret ref is required')
      return
    }
    if (!Number.isInteger(interval) || interval < 1) {
      toast.error('Poll interval must be a positive integer')
      return
    }

    if (hasIntegration) {
      const body: PatchIntegrationRequest = {
        platform,
        base_url: trimmedUrl,
        owner: trimmedOwner,
        repo: trimmedRepo,
        token_secret_ref: trimmedToken,
        poll_interval_secs: interval,
        enabled,
      }
      updateIntegration.mutate(
        { projectId, body },
        {
          onSuccess: () => toast.success('Integration updated'),
          onError: (error) =>
            toast.error(getApiErrorMessage(error, 'Failed to update integration')),
        },
      )
    } else {
      const body: CreateIntegrationRequest = {
        platform,
        base_url: trimmedUrl,
        owner: trimmedOwner,
        repo: trimmedRepo,
        token_secret_ref: trimmedToken,
        poll_interval_secs: interval,
        enabled,
      }
      createIntegration.mutate(
        { projectId, body },
        {
          onSuccess: () => toast.success('Integration created'),
          onError: (error) =>
            toast.error(getApiErrorMessage(error, 'Failed to create integration')),
        },
      )
    }
  }

  const handleSync = () => {
    triggerSync.mutate(projectId, {
      onSuccess: (result) => {
        toast.success(
          `Sync complete: ${result.imported} imported, ${result.skipped} skipped, ${result.errors} errors`,
        )
      },
      onError: (error) => toast.error(getApiErrorMessage(error, 'Sync failed')),
    })
  }

  const handleDelete = () => {
    deleteIntegration.mutate(projectId, {
      onSuccess: () => {
        toast.success('Integration deleted')
        setConfirmingDelete(false)
        resetToRepoDefaults()
      },
      onError: (error) => toast.error(getApiErrorMessage(error, 'Failed to delete integration')),
    })
  }

  const isSaving = createIntegration.isPending || updateIntegration.isPending

  return (
    <>
      <div className={embedded ? 'mb-4 mt-8' : 'mb-8'}>
        <h2 className="text-page font-semibold tracking-tight">
          {embedded ? 'Issue Sync' : 'Integrations'}
        </h2>
        <p className="mt-1 text-sm text-muted-foreground">
          Sync issues from GitHub or Gitea into this project as tasks.
        </p>
      </div>

      {integrationQuery.isLoading && !is404 ? (
        <div className="space-y-4">
          <Skeleton className="h-10 w-full" />
          <Skeleton className="h-32 w-full" />
        </div>
      ) : integrationQuery.isError && !is404 ? (
        <div className="rounded-md border border-destructive/50 p-4 text-sm text-destructive">
          Failed to load integration settings.{' '}
          <button
            type="button"
            className="underline"
            onClick={() => void integrationQuery.refetch()}
          >
            Retry
          </button>
        </div>
      ) : (
        <>
          <SettingsSection title="Platform" description="The issue tracker platform to sync from.">
            <Select
              value={platform}
              options={PLATFORM_OPTIONS}
              onChange={handlePlatformChange}
              className="max-w-xs"
            />
          </SettingsSection>

          <SettingsSection
            title="Base URL"
            description="API base URL. For GitHub, use https://api.github.com. For Gitea, use your instance URL (e.g. https://gitea.example.com)."
          >
            <Input
              value={baseUrl}
              onChange={(e) => setBaseUrl(e.target.value)}
              placeholder="https://api.github.com"
              className="max-w-md"
            />
          </SettingsSection>

          <SettingsSection
            title="Repository"
            description="Owner and repository name to sync issues from. Forge prefills this from the saved remote URL when it can."
          >
            <div className="flex max-w-md items-center gap-2">
              <div className="flex-1 space-y-1">
                <Label htmlFor="integration-owner">Owner</Label>
                <Input
                  id="integration-owner"
                  value={owner}
                  onChange={(e) => setOwner(e.target.value)}
                  placeholder="owner"
                />
              </div>
              <span className="mt-5 text-muted-foreground">/</span>
              <div className="flex-1 space-y-1">
                <Label htmlFor="integration-repo">Repo</Label>
                <Input
                  id="integration-repo"
                  value={repo}
                  onChange={(e) => setRepo(e.target.value)}
                  placeholder="repo"
                />
              </div>
            </div>
          </SettingsSection>

          <SettingsSection
            title="Token secret ref"
            description="Reference to the secret containing the API token for authentication. This should be a key in your Forge secrets store."
          >
            <Input
              value={tokenSecretRef}
              onChange={(e) => setTokenSecretRef(e.target.value)}
              placeholder="GITHUB_TOKEN"
              className="max-w-md"
            />
          </SettingsSection>

          <SettingsSection
            title="Poll interval"
            description="How often (in seconds) to check for new issues."
          >
            <Input
              type="number"
              min={1}
              value={pollInterval}
              onChange={(e) => setPollInterval(e.target.value)}
              className="max-w-[120px]"
            />
          </SettingsSection>

          <SettingsSection title="Enabled" description="Toggle automatic polling on or off.">
            <Switch
              checked={enabled}
              onChange={() => setEnabled((v) => !v)}
              aria-label="Enable integration"
            />
          </SettingsSection>

          {hasIntegration && integrationQuery.data?.last_polled_at && (
            <SettingsSection title="Last polled" description="Timestamp of the most recent poll.">
              <p className="text-sm text-foreground">
                {new Date(integrationQuery.data.last_polled_at).toLocaleString()}
              </p>
            </SettingsSection>
          )}

          <div className="flex items-center justify-between border-t py-6">
            <div className="flex items-center gap-2">
              {hasIntegration && (
                <>
                  <Button
                    size="sm"
                    variant="outline"
                    disabled={triggerSync.isPending}
                    onClick={handleSync}
                  >
                    <ArrowsClockwise
                      size={14}
                      className={triggerSync.isPending ? 'mr-1.5 animate-spin' : 'mr-1.5'}
                    />
                    {triggerSync.isPending ? 'Syncing...' : 'Sync Now'}
                  </Button>
                  <Button
                    size="sm"
                    variant="ghost"
                    className="text-destructive hover:bg-destructive/10 hover:text-destructive"
                    onClick={() => setConfirmingDelete(true)}
                  >
                    <Trash size={14} className="mr-1.5" />
                    Delete
                  </Button>
                </>
              )}
            </div>
            <Button disabled={isSaving} onClick={handleSave}>
              {isSaving ? 'Saving...' : hasIntegration ? 'Save' : 'Create Integration'}
            </Button>
          </div>
        </>
      )}

      <Dialog open={confirmingDelete} onOpenChange={setConfirmingDelete}>
        <DialogContent>
          <DialogHeader>
            <DialogTitle>Delete integration</DialogTitle>
            <DialogDescription>
              Are you sure you want to delete this integration? Issue sync will stop and the
              configuration will be removed.
            </DialogDescription>
          </DialogHeader>
          <DialogFooter className="mt-4">
            <Button variant="outline" onClick={() => setConfirmingDelete(false)}>
              Cancel
            </Button>
            <Button
              variant="destructive"
              disabled={deleteIntegration.isPending}
              onClick={handleDelete}
            >
              {deleteIntegration.isPending ? 'Deleting...' : 'Delete'}
            </Button>
          </DialogFooter>
        </DialogContent>
      </Dialog>
    </>
  )
}
