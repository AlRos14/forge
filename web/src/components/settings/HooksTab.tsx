import { Fragment, useState } from 'react'
import { productTerm } from '@/lib/i18n'
import {
  ArrowDown,
  ArrowRight,
  ArrowUp,
  Plus,
  Trash,
} from '@phosphor-icons/react'
import { toast } from 'sonner'
import { useTestProjectLifecycleHook } from '@/api/hooks'
import { ProjectHooksSection } from '@/components/settings/ProjectHooksSection'
import {
  BUILTIN_PLUGINS,
  LIFECYCLE_EVENTS,
} from '@/components/settings/project-settings-utils'
import { Badge } from '@/components/ui/badge'
import { Button } from '@/components/ui/button'
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from '@/components/ui/dialog'
import { ShellEditor } from '@/components/ui/code-editor'
import { Input } from '@/components/ui/input'
import { Label } from '@/components/ui/label'
import { Skeleton } from '@/components/ui/skeleton'
import { Switch } from '@/components/ui/switch'
import { Tooltip } from '@/components/ui/tooltip'
import { cn } from '@/lib/cn'
import type { LifecycleEvent, LifecycleHookDef, LifecycleHooks, Project } from '@/types/generated'

interface HooksTabProps {
  project?: Project
  projectId: string
  projectIsLoading: boolean
  canSave: boolean
  isSaving: boolean
  lifecycleHooks: LifecycleHooks
  onLifecycleHooksChange: (hooks: LifecycleHooks) => void
  onSave: () => void
}

export function HooksTab({
  project,
  projectId,
  projectIsLoading,
  canSave,
  isSaving,
  lifecycleHooks,
  onLifecycleHooksChange,
  onSave,
}: HooksTabProps) {
  const testHook = useTestProjectLifecycleHook(projectId)
  const [activeTab, setActiveTab] = useState<LifecycleEvent>('before_work')
  const [addingScriptFor, setAddingScriptFor] = useState<LifecycleEvent | null>(null)
  const [newScriptCommand, setNewScriptCommand] = useState('')
  const [newScriptTimeout, setNewScriptTimeout] = useState('30')
  const [newScriptBlocking, setNewScriptBlocking] = useState(false)
  const [testTaskId, setTestTaskId] = useState('')
  const [lastTestResult, setLastTestResult] = useState<{
    event: LifecycleEvent
    hookIndex: number
    result: Awaited<ReturnType<typeof testHook.mutateAsync>>
  } | null>(null)

  const updateEventHooks = (
    event: LifecycleEvent,
    update: (hooks: LifecycleHookDef[]) => LifecycleHookDef[],
  ) => {
    const next = { ...lifecycleHooks }
    const updated = update([...(lifecycleHooks[event] ?? [])])
    if (updated.length > 0) {
      next[event] = updated
    } else {
      delete next[event]
    }
    onLifecycleHooksChange(next)
  }

  const addScriptHook = (event: LifecycleEvent) => {
    const command = newScriptCommand.trim()
    const timeout = Number(newScriptTimeout.trim())
    if (!command) {
      toast.error('Script command is required')
      return
    }
    if (!Number.isInteger(timeout) || timeout < 1) {
      toast.error('Timeout must be 1 or greater')
      return
    }
    updateEventHooks(event, (hooks) => [
      ...hooks,
      {
        type: 'script',
        command,
        timeout_seconds: timeout,
        blocking: event === 'before_work' && newScriptBlocking,
      },
    ])
    setAddingScriptFor(null)
    setNewScriptCommand('')
    setNewScriptTimeout('30')
    setNewScriptBlocking(false)
  }

  const updateScriptHook = (
    event: LifecycleEvent,
    index: number,
    update: (hook: Extract<LifecycleHookDef, { type: 'script' }>) => LifecycleHookDef,
  ) => {
    updateEventHooks(event, (hooks) =>
      hooks.map((hook, hookIndex) => {
        if (hookIndex !== index || hook.type !== 'script') return hook
        return update(hook)
      }),
    )
  }

  const removeHook = (event: LifecycleEvent, index: number) => {
    updateEventHooks(event, (hooks) => hooks.filter((_, i) => i !== index))
  }

  const togglePlugin = (event: LifecycleEvent, pluginName: string) => {
    updateEventHooks(event, (hooks) => {
      const index = hooks.findIndex((hook) => hook.type === 'plugin' && hook.name === pluginName)
      if (index === -1) {
        return [...hooks, { type: 'plugin', name: pluginName, enabled: true, config: null }]
      }
      return hooks.map((hook, i) =>
        i === index && hook.type === 'plugin' ? { ...hook, enabled: !hook.enabled } : hook,
      )
    })
  }

  const moveHook = (event: LifecycleEvent, index: number, direction: -1 | 1) => {
    updateEventHooks(event, (hooks) => {
      const nextIndex = index + direction
      if (nextIndex < 0 || nextIndex >= hooks.length) return hooks
      const next = [...hooks]
      const [hook] = next.splice(index, 1)
      next.splice(nextIndex, 0, hook)
      return next
    })
  }

  const resetAddScriptDialog = () => {
    setAddingScriptFor(null)
    setNewScriptCommand('')
    setNewScriptTimeout('30')
    setNewScriptBlocking(false)
  }

  const runHookTest = async (event: LifecycleEvent, hookIndex: number) => {
    const taskId = testTaskId.trim()
    if (!taskId) {
      toast.error('Task ID is required to test lifecycle hooks')
      return
    }
    try {
      const result = await testHook.mutateAsync({ task_id: taskId, event, hook_index: hookIndex })
      setLastTestResult({ event, hookIndex, result })
      toast.success('Lifecycle hook test completed')
    } catch (error) {
      toast.error(error instanceof Error ? error.message : 'Lifecycle hook test failed')
    }
  }

  return (
    <>
      <ProjectHooksSection
        project={project}
        projectId={projectId}
        projectIsLoading={projectIsLoading}
      />

      <div className="mb-8">
        <h2 className="text-page font-semibold tracking-tight">Lifecycle Hooks</h2>
        <p className="mt-1 text-sm text-muted-foreground">
          Scripts and plugins that fire at key task lifecycle points. Hooks are non-blocking by
          default; before-work scripts can be marked required to stop agent dispatch when they fail.
        </p>
      </div>
      {projectIsLoading ? (
        <Skeleton className="h-64 w-full" />
      ) : (
        <>
          {/* Event tab selector */}
          <div className="flex items-center gap-0.5 overflow-x-auto rounded-lg border bg-muted/40 p-1">
            {LIFECYCLE_EVENTS.map((event, i) => {
              const hookCount = (lifecycleHooks[event.key] ?? []).filter(
                (h) => h.type === 'script' || (h.type === 'plugin' && h.enabled),
              ).length
              const isActive = activeTab === event.key
              return (
                <Fragment key={event.key}>
                  <button
                    type="button"
                    onClick={() => setActiveTab(event.key)}
                    className={cn(
                      'flex shrink-0 cursor-pointer items-center gap-1.5 rounded-md px-3 py-2 text-left transition-colors duration-150 focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring',
                      isActive
                        ? 'bg-background text-foreground shadow-sm'
                        : 'text-muted-foreground hover:bg-background/60 hover:text-foreground',
                    )}
                  >
                    <span className="whitespace-nowrap text-xs font-medium">{event.label}</span>
                    {hookCount > 0 && (
                      <Badge
                        variant={isActive ? 'default' : 'secondary'}
                        className="h-4 px-1 text-micro"
                      >
                        {hookCount}
                      </Badge>
                    )}
                  </button>
                  {i < LIFECYCLE_EVENTS.length - 1 && (
                    <ArrowRight
                      size={12}
                      className="mx-0.5 shrink-0 text-muted-foreground/30"
                      aria-hidden
                    />
                  )}
                </Fragment>
              )
            })}
          </div>

          {/* Active event panel */}
          {LIFECYCLE_EVENTS.map((event) => {
            if (activeTab !== event.key) return null
            const hooks = lifecycleHooks[event.key] ?? []
            const scriptHooks = hooks
              .map((hook, i) => ({ hook, index: i }))
              .filter(({ hook }) => hook.type === 'script')
            const supportedPlugins = BUILTIN_PLUGINS.filter((p) =>
              p.supportedEvents.includes(event.key),
            )
            return (
              <div key={event.key} className="space-y-5">
                <p className="text-sm text-muted-foreground">{event.description}</p>
                <div className="space-y-2 rounded-md border p-3">
                  <Label htmlFor={`hook-test-task-${event.key}`}>Task ID for hook test</Label>
                  <Input
                    id={`hook-test-task-${event.key}`}
                    placeholder="Enter a task id in this project"
                    value={testTaskId}
                    onChange={(e) => setTestTaskId(e.target.value)}
                  />
                    <p className="text-xs text-muted-foreground">
                    Tests run hook scripts with production hook context but do not transition task{' '}
                    {productTerm('phase').toLowerCase()} or launch agent{' '}
                    {productTerm('run', 0).toLowerCase()}.
                  </p>
                </div>

                {supportedPlugins.length > 0 && (
                  <div className="space-y-2">
                    <p className="text-xs font-semibold uppercase tracking-wider text-muted-foreground">
                      Built-in Plugins
                    </p>
                    <div className="space-y-2">
                      {supportedPlugins.map((plugin) => {
                        const hook = hooks.find(
                          (h) => h.type === 'plugin' && h.name === plugin.name,
                        )
                        const enabled = hook?.type === 'plugin' ? hook.enabled : false
                        return (
                          <div
                            key={plugin.name}
                            className="flex items-center gap-3 rounded-md border p-3"
                          >
                            <div className="min-w-0 flex-1">
                              <p className="text-sm font-medium">{plugin.label}</p>
                              <p className="mt-0.5 text-xs text-muted-foreground">
                                {plugin.description}
                              </p>
                            </div>
                            <Switch
                              checked={enabled}
                              aria-label={`Toggle ${plugin.label}`}
                              onChange={() => togglePlugin(event.key, plugin.name)}
                            />
                          </div>
                        )
                      })}
                    </div>
                  </div>
                )}

                <div className="space-y-2">
                  <div className="flex items-center justify-between">
                    <p className="text-xs font-semibold uppercase tracking-wider text-muted-foreground">
                      Script Hooks
                    </p>
                    <div className="flex items-center gap-1.5">
                      <Button
                        size="sm"
                        variant="default"
                        className="h-7 text-xs"
                        disabled={isSaving || !canSave}
                        onClick={onSave}
                      >
                        {isSaving ? 'Saving…' : 'Save'}
                      </Button>
                      <Button
                        size="sm"
                        variant="outline"
                        className="h-7 gap-1.5 text-xs"
                        onClick={() => {
                          setAddingScriptFor(event.key)
                          setNewScriptCommand('')
                          setNewScriptTimeout('30')
                          setNewScriptBlocking(false)
                        }}
                      >
                        <Plus size={12} weight="bold" aria-hidden />
                        Add Script
                      </Button>
                    </div>
                  </div>

                  {scriptHooks.length === 0 ? (
                    <div className="rounded-md border border-dashed p-4 text-center text-sm text-muted-foreground">
                      No scripts configured
                    </div>
                  ) : (
                    <div className="space-y-2">
                      {scriptHooks.map(({ hook, index }) => {
                        if (hook.type !== 'script') return null
                        return (
                          <div
                            key={index}
                            className="flex items-start gap-2 rounded-md border p-3"
                          >
                            <div className="min-w-0 flex-1 space-y-3">
                              <div className="space-y-1.5">
                                <Label>Command</Label>
                                <ShellEditor
                                  value={hook.command}
                                  minHeight="80px"
                                  onChange={(v) =>
                                    updateScriptHook(event.key, index, (current) => ({
                                      ...current,
                                      command: v,
                                    }))
                                  }
                                />
                              </div>
                              <div className="flex flex-wrap items-center gap-4">
                                <label
                                  className="flex items-center gap-1.5 text-xs text-muted-foreground"
                                  htmlFor={`script-${event.key}-${index}-timeout`}
                                >
                                  Timeout
                                  <Input
                                    id={`script-${event.key}-${index}-timeout`}
                                    type="number"
                                    min={1}
                                    className="h-6 w-16 px-1.5 text-xs"
                                    value={hook.timeout_seconds}
                                    onChange={(e) => {
                                      const timeout = Number(e.target.value)
                                      updateScriptHook(event.key, index, (current) => ({
                                        ...current,
                                        timeout_seconds:
                                          Number.isInteger(timeout) && timeout > 0
                                            ? timeout
                                            : current.timeout_seconds,
                                      }))
                                    }}
                                  />
                                  <span>s</span>
                                </label>
                                {event.key === 'before_work' && (
                                  <label className="flex items-center gap-1.5 text-xs text-muted-foreground">
                                    <Switch
                                      checked={hook.blocking}
                                      aria-label="Require script before dispatch"
                                      onChange={() =>
                                        updateScriptHook(event.key, index, (current) => ({
                                          ...current,
                                          blocking: !current.blocking,
                                        }))
                                      }
                                    />
                                    Require before dispatch
                                  </label>
                                )}
                              </div>
                            </div>
                            <div className="flex shrink-0 items-center">
                              <Tooltip content="Test script">
                                <Button
                                  size="sm"
                                  variant="ghost"
                                  className="h-7 px-2 text-xs"
                                  disabled={testHook.isPending}
                                  onClick={() => void runHookTest(event.key, index)}
                                >
                                  Test
                                </Button>
                              </Tooltip>
                              <Tooltip content="Move up">
                                <Button
                                  size="sm"
                                  variant="ghost"
                                  className="h-7 w-7 p-0"
                                  disabled={index === 0}
                                  aria-label="Move up"
                                  onClick={() => moveHook(event.key, index, -1)}
                                >
                                  <ArrowUp size={13} aria-hidden />
                                </Button>
                              </Tooltip>
                              <Tooltip content="Move down">
                                <Button
                                  size="sm"
                                  variant="ghost"
                                  className="h-7 w-7 p-0"
                                  disabled={index === hooks.length - 1}
                                  aria-label="Move down"
                                  onClick={() => moveHook(event.key, index, 1)}
                                >
                                  <ArrowDown size={13} aria-hidden />
                                </Button>
                              </Tooltip>
                              <Tooltip content="Remove">
                                <Button
                                  size="sm"
                                  variant="ghost"
                                  className="h-7 w-7 p-0 text-destructive hover:bg-destructive/10 hover:text-destructive"
                                  aria-label="Remove script"
                                  onClick={() => removeHook(event.key, index)}
                                >
                                  <Trash size={13} aria-hidden />
                                </Button>
                              </Tooltip>
                            </div>
                          </div>
                        )
                      })}
                    </div>
                  )}
                </div>
              </div>
            )
          })}
          {lastTestResult ? (
            <div className="mt-6 space-y-2 rounded-md border p-3">
              <p className="text-sm font-semibold">
                Last hook test: {lastTestResult.event} #{lastTestResult.hookIndex}
              </p>
              <p className="text-xs text-muted-foreground">
                status={lastTestResult.result.status} exit={String(lastTestResult.result.exit_code)} timeout=
                {String(lastTestResult.result.timeout)} duration={lastTestResult.result.duration_ms}ms
              </p>
              <p className="text-xs">
                <span className="font-semibold">working_dir:</span> {lastTestResult.result.working_dir}
              </p>
              <p className="text-xs">
                <span className="font-semibold">hook_log_path:</span>{' '}
                {lastTestResult.result.hook_log_path ?? '—'}
              </p>
              <p className="text-xs">
                <span className="font-semibold">environment_preview:</span>{' '}
                {Object.entries(lastTestResult.result.environment_preview)
                  .map(([k, v]) => `${k}=${v}`)
                  .join(' ')}
              </p>
              <div className="space-y-1">
                <p className="text-xs font-semibold">stdout</p>
                <pre className="max-h-40 overflow-auto rounded bg-muted p-2 text-xs">
                  {lastTestResult.result.stdout || '∅'}
                </pre>
              </div>
              <div className="space-y-1">
                <p className="text-xs font-semibold">stderr</p>
                <pre className="max-h-40 overflow-auto rounded bg-muted p-2 text-xs">
                  {lastTestResult.result.stderr || '∅'}
                </pre>
              </div>
            </div>
          ) : null}

          <Dialog
            open={addingScriptFor !== null}
            onOpenChange={(open) => {
              if (!open) resetAddScriptDialog()
            }}
          >
            <DialogContent>
              <DialogHeader>
                <DialogTitle>Add Script Hook</DialogTitle>
                <DialogDescription>
                  {addingScriptFor
                    ? `Runs when "${LIFECYCLE_EVENTS.find((e) => e.key === addingScriptFor)?.label}" fires. Paste a shell command or snippet here; it does not need to reference a script file. Forge runs it via bash -lc in the task worktree with a 30s timeout by default.`
                    : ''}
                </DialogDescription>
              </DialogHeader>
              <div className="space-y-4 py-2">
                <div className="space-y-2">
                  <Label>Command</Label>
                  <ShellEditor
                    value={newScriptCommand}
                    minHeight="112px"
                    onChange={setNewScriptCommand}
                  />
                  <p className="text-xs text-muted-foreground">
                    Inline shell snippets are supported.
                  </p>
                </div>
                <div className="space-y-2">
                  <Label htmlFor="lifecycle-dialog-timeout">Timeout (seconds)</Label>
                  <Input
                    id="lifecycle-dialog-timeout"
                    type="number"
                    min={1}
                    value={newScriptTimeout}
                    onChange={(e) => setNewScriptTimeout(e.target.value)}
                  />
                </div>
                {addingScriptFor === 'before_work' ? (
                  <label className="flex items-center gap-2 text-sm">
                    <Switch
                      checked={newScriptBlocking}
                      aria-label="Require script before dispatch"
                      onChange={() => setNewScriptBlocking((v) => !v)}
                    />
                    Require this script before agent dispatch
                  </label>
                ) : null}
                <div className="rounded-md bg-muted px-3 py-2.5">
                  <p className="mb-1 text-xs font-medium text-muted-foreground">
                    Available env vars
                  </p>
                  <p className="font-mono text-xs leading-relaxed text-muted-foreground">
                    FORGE_EVENT · FORGE_TASK_ID · FORGE_TASK_TITLE · FORGE_TASK_STATUS ·
                    FORGE_PROJECT_ID · FORGE_REPO_PATH · FORGE_WORKTREE_PATH
                  </p>
                </div>
              </div>
              <DialogFooter>
                <Button variant="outline" onClick={resetAddScriptDialog}>
                  Cancel
                </Button>
                <Button onClick={() => addingScriptFor && addScriptHook(addingScriptFor)}>
                  Add Script
                </Button>
              </DialogFooter>
            </DialogContent>
          </Dialog>
        </>
      )}
    </>
  )
}
