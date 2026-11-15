import { useState, type FormEvent } from 'react'
import { toast } from 'sonner'
import {
  useCreateValidationCheck,
  useMembersQuery,
  useProjectValidationChecksQuery,
  useReposQuery,
  useRetireValidationCheck,
} from '@/api/hooks'
import { SettingsSection } from '@/components/settings/SettingsSection'
import { canManageValidationCatalog } from '@/components/task-detail/task-verification'
import { Badge } from '@/components/ui/badge'
import { Button } from '@/components/ui/button'
import { Input } from '@/components/ui/input'
import { Label } from '@/components/ui/label'
import { Textarea } from '@/components/ui/textarea'
import { getApiErrorMessage, isApiStatus } from '@/lib/api-error'
import { useAuthStore } from '@/stores/auth'

export function ValidationChecksTab({ projectId }: { projectId: string }) {
  const user = useAuthStore((state) => state.user)
  const membersQuery = useMembersQuery(projectId)
  const reposQuery = useReposQuery(projectId)
  const checksQuery = useProjectValidationChecksQuery(projectId)
  const createCheck = useCreateValidationCheck(projectId)
  const retireCheck = useRetireValidationCheck(projectId)
  const [checkKey, setCheckKey] = useState('')
  const [command, setCommand] = useState('')
  const [timeoutSeconds, setTimeoutSeconds] = useState('600')
  const [repoId, setRepoId] = useState('')
  const myRole = membersQuery.data?.find((member) => member.user_id === user?.id)?.role
  const canManage = canManageValidationCatalog(myRole) || Boolean(user?.is_admin)
  const items = checksQuery.data?.items ?? []
  const timeout = Number(timeoutSeconds)
  const canSubmit =
    canManage &&
    checkKey.trim().length > 0 &&
    command.trim().length > 0 &&
    Number.isInteger(timeout) &&
    timeout > 0

  async function submit(event: FormEvent<HTMLFormElement>) {
    event.preventDefault()
    if (!canSubmit) return
    try {
      const created = await createCheck.mutateAsync({
        repo_id: repoId || null,
        check_key: checkKey.trim(),
        command: command.trim(),
        timeout_seconds: timeout,
        idempotency_key: crypto.randomUUID(),
      })
      toast.success(`Published ${created.check_key} version ${created.version}`)
      setCommand('')
    } catch (error) {
      toast.error(getApiErrorMessage(error, 'Validation check could not be published'))
    }
  }

  async function retire(id: string) {
    try {
      await retireCheck.mutateAsync({ checkId: id, body: { expected_state: 'active' } })
      toast.success('Validation check retired')
    } catch (error) {
      toast.error(
        isApiStatus(error, 409)
          ? 'This check was already retired. The list was refreshed.'
          : getApiErrorMessage(error, 'Validation check could not be retired'),
      )
    }
  }

  return (
    <SettingsSection
      title="Validation checks"
      description="Versioned commands the coordinator can run on an exact commit. Publishing a change creates a new version; older versions are kept."
    >
      {!canManage ? (
        <p role="status" className="mb-3 text-sm text-muted-foreground">
          Only Project owners and admins can publish or retire checks. You can read the catalog.
        </p>
      ) : null}
      {checksQuery.isLoading ? <p className="text-sm text-muted-foreground">Loading checks…</p> : null}
      {checksQuery.isError ? (
        <p role="alert" className="text-sm text-destructive">
          Validation checks could not be loaded.
        </p>
      ) : null}
      {!checksQuery.isLoading && !checksQuery.isError && items.length === 0 ? (
        <p className="text-sm text-muted-foreground">No validation checks are published.</p>
      ) : null}
      <ul className="space-y-2">
        {items.map((item) => (
          <li key={item.id} className="min-w-0 space-y-1 rounded-md border p-3 text-sm">
            <div className="flex flex-wrap items-center gap-2">
              <code className="font-semibold">
                {item.check_key}@{item.version}
              </code>
              <Badge variant={item.state === 'active' ? 'outline' : 'secondary'}>{item.state}</Badge>
              <span className="text-xs text-muted-foreground">
                {item.repo_id ? `Repo ${item.repo_id}` : 'Whole Project'} · timeout{' '}
                {item.timeout_seconds}s
              </span>
              {canManage && item.state === 'active' ? (
                <Button
                  size="sm"
                  variant="outline"
                  disabled={retireCheck.isPending}
                  onClick={() => void retire(item.id)}
                >
                  Retire
                </Button>
              ) : null}
            </div>
            <code className="block break-all text-xs text-muted-foreground">{item.command}</code>
            <p className="break-all text-xs text-muted-foreground">digest {item.definition_digest}</p>
          </li>
        ))}
      </ul>
      {canManage ? (
        <form onSubmit={(event) => void submit(event)} className="mt-4 space-y-3 rounded-md border p-3">
          <p className="text-sm font-medium">Publish a new version</p>
          <div className="space-y-1">
            <Label htmlFor="validation-check-key">Check key</Label>
            <Input id="validation-check-key" value={checkKey} onChange={(e) => setCheckKey(e.target.value)} />
          </div>
          <div className="space-y-1">
            <Label htmlFor="validation-check-command">Command</Label>
            <Textarea id="validation-check-command" value={command} onChange={(e) => setCommand(e.target.value)} />
          </div>
          <div className="space-y-1">
            <Label htmlFor="validation-check-timeout">Timeout (seconds)</Label>
            <Input
              id="validation-check-timeout"
              inputMode="numeric"
              value={timeoutSeconds}
              onChange={(e) => setTimeoutSeconds(e.target.value)}
            />
          </div>
          <div className="space-y-1">
            <Label htmlFor="validation-check-repo">Repo (optional)</Label>
            <select
              id="validation-check-repo"
              className="h-10 w-full rounded-md border border-input bg-background px-3 text-ui"
              value={repoId}
              onChange={(e) => setRepoId(e.target.value)}
            >
              <option value="">Whole Project</option>
              {(reposQuery.data?.items ?? []).map((repo) => (
                <option key={repo.id} value={repo.id}>
                  {repo.name}
                </option>
              ))}
            </select>
          </div>
          <Button type="submit" disabled={!canSubmit || createCheck.isPending}>
            {createCheck.isPending ? 'Publishing…' : 'Publish version'}
          </Button>
        </form>
      ) : null}
    </SettingsSection>
  )
}
