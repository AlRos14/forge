import { useEffect, useState } from 'react'
import { toast } from 'sonner'
import { useQuery } from '@tanstack/react-query'
import {
  ArrowsClockwise,
  CaretDown,
  Check,
  CircleNotch,
  CloudArrowDown,
  Database,
  FolderOpen,
  GitBranch,
  GitMerge,
  GitPullRequest,
  Key,
  LinkSimple,
  PencilSimple,
  Timer,
  X,
} from '@phosphor-icons/react'
import { listBranches } from '@/api/client'
import { qk } from '@/api/query-keys'
import {
  useMembersQuery,
  useCreateRepo,
  useDaemonsQuery,
  useReposQuery,
  useSyncRepo,
  useUpdateRepo,
} from '@/api/hooks'
import { ErrorBanner } from '@/components/error-banner'
import { IntegrationsTab } from '@/components/settings/IntegrationsTab'
import { RepoDialog } from '@/components/settings/RepoDialog'
import { RepoPrFeedbackPolicyEditor } from '@/components/settings/RepoPrFeedbackPolicyEditor'
import { emptyRepoForm, type RepoFormState } from '@/components/settings/RepoForm'
import {
  repoFormFromRepo,
  repoPrProviderConfigFromForm,
  settingsErrorMessage,
} from '@/components/settings/project-settings-utils'
import { Badge } from '@/components/ui/badge'
import { Button } from '@/components/ui/button'
import { ComboSelect } from '@/components/ui/combo-select'
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuItem,
  DropdownMenuTrigger,
} from '@/components/ui/dropdown-menu'
import { Input } from '@/components/ui/input'
import { Skeleton } from '@/components/ui/skeleton'
import { productTerm } from '@/lib/i18n'
import { useAuthStore } from '@/stores/auth'
import type { CreateRepoRequest, Project, Repo, UpdateRepoRequest } from '@/types/generated'

function AdditionalRepoCard({
  repo,
  onEdit,
  workModeLabel,
}: {
  repo: Repo
  onEdit: (repo: Repo) => void
  workModeLabel: (mode: Repo['work_mode']) => string
}) {
  const provider = repo.pr_provider_status?.provider_type ?? repo.pr_provider ?? 'Not configured'
  const credentialStatus = repo.pr_provider_status?.has_credential
    ? repo.pr_provider_status.credential_label
      ? `Credential connected: ${repo.pr_provider_status.credential_label}`
      : 'Credential connected'
    : 'No credential'

  return (
    <div className="rounded-md border">
      <div className="flex items-start justify-between gap-3 border-b px-4 py-3">
        <div className="flex min-w-0 items-start gap-2.5">
          <span className="mt-0.5 flex h-8 w-8 shrink-0 items-center justify-center rounded-md border bg-muted">
            {repo.local_path ? (
              <FolderOpen size={15} className="text-muted-foreground" />
            ) : (
              <CloudArrowDown size={15} className="text-muted-foreground" />
            )}
          </span>
          <div className="min-w-0">
            <p className="truncate font-semibold leading-tight">{repo.name}</p>
            <div className="mt-1 flex flex-wrap items-center gap-1.5">
              <Badge variant="outline" className="text-xs">
                {repo.local_path ? 'Local' : 'Remote'}
              </Badge>
              <Badge variant="secondary" className="text-xs">
                {repo.work_mode === 'pull_request' ? (
                  <GitPullRequest size={10} className="mr-1" />
                ) : (
                  <GitMerge size={10} className="mr-1" />
                )}
                {workModeLabel(repo.work_mode)}
              </Badge>
            </div>
          </div>
        </div>
        <Button
          size="sm"
          variant="outline"
          onClick={() => onEdit(repo)}
          className="shrink-0 cursor-pointer"
        >
          <PencilSimple size={13} className="mr-1.5" />
          Edit
        </Button>
      </div>
      <div className="grid gap-3 px-4 py-3 sm:grid-cols-2">
        <div className="min-w-0">
          <p className="mb-0.5 text-xs text-muted-foreground">Remote URL</p>
          <p className="break-all font-mono text-xs">{repo.remote_url}</p>
        </div>
        <div className="min-w-0">
          <p className="mb-0.5 text-xs text-muted-foreground">Local path</p>
          <p className="break-all font-mono text-xs">{repo.local_path ?? 'Not configured'}</p>
        </div>
        <div>
          <p className="mb-0.5 text-xs text-muted-foreground">Work mode</p>
          <p className="text-xs">{workModeLabel(repo.work_mode)}</p>
        </div>
        <div>
          <p className="mb-0.5 text-xs text-muted-foreground">PR Provider</p>
          <p className="text-xs capitalize">{provider}</p>
        </div>
        <div>
          <p className="mb-0.5 text-xs text-muted-foreground">SCM credential</p>
          <p
            className={`text-xs ${
              repo.pr_provider_status?.has_credential
                ? 'text-green-600 dark:text-green-400'
                : 'text-muted-foreground'
            }`}
          >
            {credentialStatus}
          </p>
        </div>
      </div>
    </div>
  )
}

export function ReposTab({
  project,
  projectId,
  isUpdatingPrimaryRepo,
  onUpdatePrimaryRepo,
}: {
  project?: Project
  projectId: string
  isUpdatingPrimaryRepo: boolean
  onUpdatePrimaryRepo: (repoId: string | null) => void
}) {
  const isAdmin = useAuthStore((s) => Boolean(s.user?.is_admin))
  const userId = useAuthStore((s) => s.user?.id)
  const membersQuery = useMembersQuery(projectId)
  const reposQuery = useReposQuery(projectId)
  const createRepo = useCreateRepo(projectId)
  const daemonsQuery = useDaemonsQuery(isAdmin)
  const daemons = daemonsQuery.data?.items ?? []
  const [selectedDaemonId, setSelectedDaemonId] = useState<string | undefined>(undefined)
  const activeDaemonId = daemons.length === 1 ? daemons[0]?.id : selectedDaemonId

  useEffect(() => {
    if (!selectedDaemonId) return
    if (daemons.length > 1 && daemons.some((daemon) => daemon.id === selectedDaemonId)) return
    setSelectedDaemonId(undefined)
  }, [daemons, selectedDaemonId])

  const [repoDialogOpen, setRepoDialogOpen] = useState(false)
  const [repoForm, setRepoForm] = useState<RepoFormState>(emptyRepoForm)
  const [editingRepoId, setEditingRepoId] = useState<string | null>(null)
  const [primaryRepoDraft, setPrimaryRepoDraft] = useState(project?.primary_repo_id ?? '')

  useEffect(() => {
    setPrimaryRepoDraft(project?.primary_repo_id ?? '')
  }, [project?.primary_repo_id])

  // Inline branch edit state
  const [branchEditMode, setBranchEditMode] = useState(false)
  const [branchDraft, setBranchDraft] = useState('')

  const repos = reposQuery.data?.items ?? []
  const canManageRepo =
    isAdmin ||
    Boolean(project?.owner_id && project.owner_id === userId) ||
    Boolean(
      membersQuery.data?.some(
        (member) => member.user_id === userId && ['owner', 'admin'].includes(member.role),
      ),
    )
  const primaryRepo = repos.find((repo) => repo.id === project?.primary_repo_id)
  const additionalRepos = repos.filter((repo) => repo.id !== project?.primary_repo_id)
  const branchEditorDaemonId = daemons.find((daemon) => daemon.status === 'online')?.id
  const branchEditorDaemon = branchEditorDaemonId
    ? daemons.find((daemon) => daemon.id === branchEditorDaemonId)
    : undefined
  const showBranchEditorUnavailable =
    !branchEditorDaemonId || branchEditorDaemon?.status !== 'online'

  const updateRepo = useUpdateRepo(editingRepoId ?? primaryRepo?.id ?? '', projectId)
  const syncRepo = useSyncRepo(primaryRepo?.id ?? '', projectId)

  const handleSyncRepo = () => {
    if (!primaryRepo?.local_path) {
      toast.error('Sync requires a repo with a local path')
      return
    }
    syncRepo.mutate(undefined, {
      onError: (error) => toast.error(settingsErrorMessage(error, 'Repository sync failed')),
      onSuccess: () => toast.success('Repository synced'),
    })
  }

  const branchQuery = useQuery({
    queryKey: qk.branches(primaryRepo?.local_path ?? '', branchEditorDaemonId ?? ''),
    queryFn: () => listBranches(primaryRepo!.local_path!, branchEditorDaemonId!),
    enabled:
      branchEditMode &&
      Boolean(primaryRepo?.local_path) &&
      Boolean(branchEditorDaemonId) &&
      !showBranchEditorUnavailable,
  })
  const branchOptions = (branchQuery.data?.branches ?? []).map((b) => ({ value: b, label: b }))

  const openCreateLocalRepo = () => {
    setEditingRepoId(null)
    setRepoForm({ ...emptyRepoForm, source_mode: 'local' })
    setRepoDialogOpen(true)
  }

  const openCreateRemoteRepo = () => {
    setEditingRepoId(null)
    setRepoForm({ ...emptyRepoForm, source_mode: 'remote' })
    setRepoDialogOpen(true)
  }

  const openEditRepo = (repo: Repo) => {
    setEditingRepoId(repo.id)
    setRepoForm(repoFormFromRepo(repo))
    setRepoDialogOpen(true)
  }

  const startBranchEdit = () => {
    setBranchDraft(primaryRepo?.default_branch ?? '')
    setBranchEditMode(true)
  }

  const cancelBranchEdit = () => {
    setBranchEditMode(false)
    setBranchDraft('')
  }

  const saveBranch = (value: string) => {
    const trimmed = value.trim()
    if (!trimmed) {
      toast.error('Branch name cannot be empty')
      return
    }
    if (trimmed === primaryRepo?.default_branch) {
      cancelBranchEdit()
      return
    }
    setEditingRepoId(null)
    updateRepo.mutate(
      { default_branch: trimmed },
      {
        onError: (error) => toast.error(settingsErrorMessage(error, 'Failed to update branch')),
        onSuccess: () => {
          setBranchEditMode(false)
          setBranchDraft('')
        },
      },
    )
  }

  const formToCreateRequest = (nextForm: RepoFormState): CreateRepoRequest | null => {
    const localPath = nextForm.local_path.trim()
    const remoteUrlInput = nextForm.remote_url.trim()
    if (nextForm.source_mode === 'local' && !localPath) {
      toast.error('Local repo path is required')
      return null
    }
    if (nextForm.source_mode === 'remote' && !remoteUrlInput) {
      toast.error('Remote URL is required')
      return null
    }
    const remoteUrl =
      nextForm.source_mode === 'local' ? remoteUrlInput || localPath : remoteUrlInput
    const defaultBranch = nextForm.default_branch.trim() || 'main'
    const pollingIntervalInput = nextForm.pr_polling_interval_seconds.trim()
    const pollingInterval = Number(pollingIntervalInput)
    if (
      nextForm.work_mode === 'pull_request' &&
      (!pollingIntervalInput || !Number.isInteger(pollingInterval) || pollingInterval < 1)
    ) {
      toast.error('Polling interval must be 1 or greater')
      return null
    }
    return {
      remote_url: remoteUrl,
      name: nextForm.name.trim() || null,
      local_path: nextForm.source_mode === 'local' ? localPath : null,
      default_branch: defaultBranch,
      work_mode: nextForm.work_mode,
      pr_provider: nextForm.work_mode === 'pull_request' ? nextForm.pr_provider : null,
      pr_provider_config: repoPrProviderConfigFromForm(nextForm),
    }
  }

  const formToUpdateRequest = (nextForm: RepoFormState): UpdateRepoRequest | null => {
    const localPath = nextForm.local_path.trim()
    const remoteUrlInput = nextForm.remote_url.trim()
    if (nextForm.source_mode === 'local' && !localPath) {
      toast.error('Local repo path is required')
      return null
    }
    if (nextForm.source_mode === 'remote' && !remoteUrlInput) {
      toast.error('Remote URL is required')
      return null
    }
    const remoteUrl =
      nextForm.source_mode === 'local' ? remoteUrlInput || localPath : remoteUrlInput
    const defaultBranch = nextForm.default_branch.trim() || 'main'
    const pollingIntervalInput = nextForm.pr_polling_interval_seconds.trim()
    const pollingInterval = Number(pollingIntervalInput)
    if (
      nextForm.work_mode === 'pull_request' &&
      (!pollingIntervalInput || !Number.isInteger(pollingInterval) || pollingInterval < 1)
    ) {
      toast.error('Polling interval must be 1 or greater')
      return null
    }
    return {
      remote_url: remoteUrl,
      name: nextForm.name.trim() || null,
      local_path: nextForm.source_mode === 'local' ? localPath : null,
      default_branch: defaultBranch,
      work_mode: nextForm.work_mode,
      pr_provider: nextForm.work_mode === 'pull_request' ? nextForm.pr_provider : null,
      pr_provider_config:
        nextForm.work_mode === 'pull_request'
          ? {
              base_url:
                nextForm.pr_provider === 'github' ? null : nextForm.pr_base_url.trim() || null,
              polling_interval_seconds: pollingInterval,
              ...(nextForm.pr_provider === 'github' && nextForm.credential_id !== undefined
                ? { credential_id: nextForm.credential_id }
                : nextForm.pr_provider !== 'github'
                  ? { credential_id: null }
                  : {}),
            }
          : null,
    }
  }

  const submitRepo = async (nextForm = repoForm) => {
    if (editingRepoId) {
      const body = formToUpdateRequest(nextForm)
      if (!body) return
      try {
        await updateRepo.mutateAsync(body)
        toast.success('Repository updated')
        setRepoDialogOpen(false)
        setEditingRepoId(null)
        setRepoForm(emptyRepoForm)
      } catch (error) {
        toast.error(settingsErrorMessage(error, 'Repository update failed'))
      }
    } else {
      const body = formToCreateRequest(nextForm)
      if (!body) return
      try {
        await createRepo.mutateAsync(body)
        setRepoDialogOpen(false)
        setRepoForm(emptyRepoForm)
        toast.success('Repository saved')
      } catch (error) {
        toast.error(settingsErrorMessage(error, 'Repository creation failed'))
      }
    }
  }

  const workModeLabel = (mode: Repo['work_mode']) =>
    mode === 'pull_request' ? 'Pull request' : 'Direct merge'

  const isPending = editingRepoId ? updateRepo.isPending : createRepo.isPending

  return (
    <>
      <div className="mb-8">
        <h2 className="text-page font-semibold tracking-tight">Primary Repository</h2>
        <p className="mt-1 text-sm text-muted-foreground">
          Configure the repository Forge uses for new project tasks.
        </p>
      </div>

      {reposQuery.isError && (
        <ErrorBanner
          error={reposQuery.error}
          fallback="Repositories failed to load"
          onRetry={() => void reposQuery.refetch()}
        />
      )}

      <div className="mb-6 space-y-3 rounded-md border p-4">
        <div>
          <label htmlFor="project-primary-repository" className="text-sm font-medium">
            Primary repository for new Tasks
          </label>
          <p className="mt-1 text-xs text-muted-foreground">
            Changing this affects Tasks created after the update. Existing Tasks keep their repo
            set.
          </p>
        </div>
        <div className="flex flex-wrap items-center gap-2">
          <select
            id="project-primary-repository"
            aria-label="Primary repository for new Tasks"
            className="h-9 min-w-56 flex-1 rounded-md border bg-background px-2 text-sm"
            value={primaryRepoDraft}
            disabled={
              !canManageRepo || reposQuery.isLoading || reposQuery.isError || isUpdatingPrimaryRepo
            }
            onChange={(event) => setPrimaryRepoDraft(event.target.value)}
          >
            <option value="">No primary repository</option>
            {repos.map((repo) => (
              <option key={repo.id} value={repo.id}>
                {repo.name}
              </option>
            ))}
          </select>
          <Button
            size="sm"
            variant="outline"
            disabled={
              !canManageRepo ||
              !project ||
              reposQuery.isLoading ||
              reposQuery.isError ||
              isUpdatingPrimaryRepo ||
              primaryRepoDraft === (project.primary_repo_id ?? '')
            }
            onClick={() => onUpdatePrimaryRepo(primaryRepoDraft || null)}
          >
            {isUpdatingPrimaryRepo ? 'Saving…' : 'Save primary repository'}
          </Button>
        </div>
      </div>

      <div className="mb-4 flex flex-wrap items-center justify-between gap-3">
        <div>
          <h3 className="text-sm font-semibold">Project repositories</h3>
          <p className="mt-1 text-xs text-muted-foreground">
            Manage the primary repository and additional repositories for this Project.
          </p>
        </div>
        <DropdownMenu>
          <DropdownMenuTrigger className="inline-flex h-9 items-center gap-2 rounded-md bg-primary px-3 text-sm font-medium text-primary-foreground shadow-sm transition-colors hover:bg-primary/90 focus-visible:outline-none focus-visible:ring-1 focus-visible:ring-ring">
            <FolderOpen size={14} />
            Add repository
            <CaretDown size={12} />
          </DropdownMenuTrigger>
          <DropdownMenuContent align="end">
            {daemons.length > 0 ? (
              <DropdownMenuItem onClick={openCreateLocalRepo}>
                <FolderOpen size={14} className="mr-2" />
                Add local repository
              </DropdownMenuItem>
            ) : null}
            <DropdownMenuItem onClick={openCreateRemoteRepo}>
              <CloudArrowDown size={14} className="mr-2" />
              Add remote repository
            </DropdownMenuItem>
          </DropdownMenuContent>
        </DropdownMenu>
      </div>

      {reposQuery.isLoading ? (
        <div className="rounded-md border p-4 space-y-3">
          <div className="flex items-start justify-between">
            <div className="space-y-2 flex-1">
              <Skeleton className="h-4 w-36" />
              <Skeleton className="h-3 w-64" />
            </div>
            <Skeleton className="h-8 w-16 rounded-md" />
          </div>
          <div className="grid gap-2 sm:grid-cols-2">
            <Skeleton className="h-3 w-32" />
            <Skeleton className="h-3 w-24" />
          </div>
        </div>
      ) : primaryRepo ? (
        <>
          <div className="rounded-md border">
            {/* Header */}
            <div className="flex items-start justify-between gap-3 border-b px-4 py-3">
              <div className="flex min-w-0 items-center gap-2.5">
                <span className="flex h-8 w-8 shrink-0 items-center justify-center rounded-md border bg-muted">
                  {primaryRepo.local_path ? (
                    <FolderOpen size={15} className="text-muted-foreground" />
                  ) : (
                    <CloudArrowDown size={15} className="text-muted-foreground" />
                  )}
                </span>
                <div className="min-w-0">
                  <div className="flex flex-wrap items-center gap-2">
                    <p className="truncate font-semibold leading-tight">{primaryRepo.name}</p>
                    <Badge variant="outline" className="text-xs">
                      Primary
                    </Badge>
                  </div>
                  <div className="mt-0.5 flex flex-wrap items-center gap-1.5">
                    <Badge variant="outline" className="text-xs">
                      {primaryRepo.local_path ? 'Local' : 'Remote'}
                    </Badge>
                    <Badge variant="secondary" className="text-xs">
                      {primaryRepo.work_mode === 'pull_request' ? (
                        <GitPullRequest size={10} className="mr-1" />
                      ) : (
                        <GitMerge size={10} className="mr-1" />
                      )}
                      {workModeLabel(primaryRepo.work_mode)}
                    </Badge>
                  </div>
                </div>
              </div>
              <div className="flex shrink-0 items-center gap-1.5">
                {primaryRepo.local_path ? (
                  <Button
                    size="sm"
                    variant="outline"
                    onClick={handleSyncRepo}
                    disabled={syncRepo.isPending}
                    title="Pull then push the local repository"
                    className="cursor-pointer"
                  >
                    {syncRepo.isPending ? (
                      <CircleNotch size={13} className="mr-1.5 animate-spin" />
                    ) : (
                      <ArrowsClockwise size={13} className="mr-1.5" />
                    )}
                    {syncRepo.isPending ? 'Syncing…' : 'Sync'}
                  </Button>
                ) : null}
                <Button
                  size="sm"
                  variant="outline"
                  onClick={() => openEditRepo(primaryRepo)}
                  className="cursor-pointer"
                >
                  <PencilSimple size={13} className="mr-1.5" />
                  Edit
                </Button>
              </div>
            </div>

            {/* Metadata rows */}
            <div className="divide-y">
              <div className="flex items-start gap-3 px-4 py-2.5">
                <LinkSimple size={14} className="mt-0.5 shrink-0 text-muted-foreground" />
                <div className="min-w-0">
                  <p className="mb-0.5 text-xs text-muted-foreground">Remote URL</p>
                  <p className="break-all font-mono text-xs">{primaryRepo.remote_url}</p>
                </div>
              </div>

              {primaryRepo.local_path ? (
                <div className="flex items-start gap-3 px-4 py-2.5">
                  <FolderOpen size={14} className="mt-0.5 shrink-0 text-muted-foreground" />
                  <div className="min-w-0">
                    <p className="mb-0.5 text-xs text-muted-foreground">Local path</p>
                    <p className="break-all font-mono text-xs">{primaryRepo.local_path}</p>
                  </div>
                </div>
              ) : null}

              {/* Default branch — inline editable */}
              <div className="flex items-center gap-3 px-4 py-2.5">
                <GitBranch size={14} className="shrink-0 text-muted-foreground" />
                <div>
                  <p className="mb-0.5 text-xs text-muted-foreground">Default branch</p>
                  {branchEditMode ? (
                    <div className="flex items-center gap-1.5">
                      {showBranchEditorUnavailable ? (
                        <div className="rounded-md border border-red-500/30 bg-red-500/10 px-3 py-1.5 text-xs text-red-700 dark:text-red-300">
                          {productTerm('runtime')} offline — branches unavailable
                        </div>
                      ) : branchQuery.isFetching ? (
                        <CircleNotch size={14} className="animate-spin text-muted-foreground" />
                      ) : branchOptions.length > 0 ? (
                        <ComboSelect
                          value={branchDraft}
                          options={branchOptions}
                          allowCustom
                          isLoading={updateRepo.isPending}
                          className="h-7 w-44 text-xs"
                          onChange={(value) => {
                            if (value) saveBranch(value)
                          }}
                        />
                      ) : (
                        <>
                          <Input
                            autoFocus
                            className="h-7 w-36 px-2 font-mono text-xs"
                            value={branchDraft}
                            onChange={(e) => setBranchDraft(e.target.value)}
                            onKeyDown={(e) => {
                              if (e.key === 'Enter') saveBranch(branchDraft)
                              if (e.key === 'Escape') cancelBranchEdit()
                            }}
                          />
                          <Button
                            size="icon"
                            variant="ghost"
                            className="h-7 w-7"
                            disabled={updateRepo.isPending}
                            onClick={() => saveBranch(branchDraft)}
                          >
                            <Check size={12} />
                          </Button>
                        </>
                      )}
                      <Button
                        size="icon"
                        variant="ghost"
                        className="h-7 w-7"
                        onClick={cancelBranchEdit}
                      >
                        <X size={12} />
                      </Button>
                    </div>
                  ) : (
                    <button
                      type="button"
                      className="group/branch flex cursor-pointer items-center gap-1.5"
                      onClick={startBranchEdit}
                    >
                      <span className="font-mono text-xs">{primaryRepo.default_branch}</span>
                      <PencilSimple
                        size={11}
                        className="text-muted-foreground opacity-0 transition-opacity duration-150 group-hover/branch:opacity-100"
                      />
                    </button>
                  )}
                </div>
              </div>
            </div>

            {/* PR provider section */}
            {primaryRepo.work_mode === 'pull_request' ? (
              <div className="border-t bg-muted/20 px-4 py-3">
                <p className="mb-2 text-xs font-medium text-muted-foreground uppercase tracking-wide">
                  PR Provider
                </p>
                <div className="flex flex-wrap gap-3">
                  <div className="flex items-center gap-1.5">
                    <Badge variant="outline" className="text-xs capitalize">
                      {primaryRepo.pr_provider_status?.provider_type ??
                        primaryRepo.pr_provider ??
                        'Not configured'}
                    </Badge>
                  </div>
                  <div className="flex items-center gap-1.5 text-xs text-muted-foreground">
                    <Key size={12} />
                    <span
                      className={
                        primaryRepo.pr_provider_status?.has_credential
                          ? 'text-green-600 dark:text-green-400'
                          : 'text-destructive'
                      }
                    >
                      {primaryRepo.pr_provider_status?.has_credential
                        ? 'Credential connected'
                        : 'No credential'}
                    </span>
                  </div>
                  {primaryRepo.pr_provider_status ? (
                    <>
                      <div className="flex items-center gap-1.5 text-xs text-muted-foreground">
                        <Timer size={12} />
                        <span>
                          Polls every {primaryRepo.pr_provider_status.polling_interval_seconds}s
                        </span>
                      </div>
                    </>
                  ) : null}
                </div>
              </div>
            ) : null}
          </div>
          {primaryRepo.work_mode === 'pull_request' && canManageRepo ? (
            <RepoPrFeedbackPolicyEditor key={primaryRepo.id} repoId={primaryRepo.id} />
          ) : null}
          <IntegrationsTab embedded projectId={projectId} repoRemoteUrl={primaryRepo.remote_url} />
        </>
      ) : repos.length > 0 ? (
        <div className="rounded-md border border-dashed p-4 text-sm text-muted-foreground">
          No primary repository is selected. Choose one above to use it for new Tasks.
        </div>
      ) : (
        <div className="rounded-lg border border-dashed p-8 text-center">
          <div className="mx-auto mb-4 flex h-11 w-11 items-center justify-center rounded-full bg-muted">
            <Database size={20} className="text-muted-foreground" />
          </div>
          <p className="font-medium">No repository configured</p>
          <p className="mt-1.5 text-sm text-muted-foreground">
            Add a repository before creating project tasks.
          </p>
        </div>
      )}
      {additionalRepos.length > 0 ? (
        <section
          className="mt-6 space-y-3"
          aria-label={primaryRepo ? 'Additional repositories' : 'Project repositories'}
        >
          <h3 className="text-sm font-semibold">
            {primaryRepo ? 'Additional repositories' : 'Project repositories'}
          </h3>
          {additionalRepos.map((repo) => (
            <AdditionalRepoCard
              key={repo.id}
              repo={repo}
              onEdit={openEditRepo}
              workModeLabel={workModeLabel}
            />
          ))}
        </section>
      ) : null}

      <RepoDialog
        form={repoForm}
        open={repoDialogOpen}
        pending={isPending}
        daemons={daemons}
        daemonId={activeDaemonId}
        onDaemonChange={setSelectedDaemonId}
        title={
          editingRepoId
            ? 'Edit Repository'
            : repoForm.source_mode === 'remote'
              ? 'Add Remote Repository'
              : 'Add Local Repository'
        }
        onOpenChange={(open) => {
          setRepoDialogOpen(open)
          if (!open) setEditingRepoId(null)
        }}
        onSubmit={submitRepo}
        onUpdate={setRepoForm}
      />
    </>
  )
}
