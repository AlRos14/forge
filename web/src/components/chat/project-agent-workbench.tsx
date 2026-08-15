import { useEffect, useId, useRef, useState, type ReactNode } from 'react'
import { Link } from '@tanstack/react-router'
import {
  ArrowUpRight,
  CheckCircle,
  ClipboardText,
  FileText,
  Flag,
  Scales,
  WarningCircle,
} from '@phosphor-icons/react'
import { useCreateTask, useProjectOverviewQuery, useProjectQuery, useUpdateProject } from '@/api/hooks'
import { Button } from '@/components/ui/button'
import { Card } from '@/components/ui/card'
import { Input } from '@/components/ui/input'
import { Label } from '@/components/ui/label'
import { Select } from '@/components/ui/select'
import { Textarea } from '@/components/ui/textarea'
import { ErrorPanel, LoadingPanel, SectionKicker, StateBadge } from '@/features/federation/components'
import {
  useCreateWorkbenchDecision,
  useCreateWorkbenchDocument,
  useCreateWorkbenchMilestone,
} from '@/features/project-workbench/hooks'
import { isApiStatus } from '@/lib/api-error'
import { useAuthStore } from '@/stores/auth'
import type { ProjectDocumentKind } from '@/types/generated'

type EditorState = 'idle' | 'dirty' | 'saving' | 'saved' | 'conflict' | 'error'

function count(value: number | bigint | undefined): number {
  return typeof value === 'bigint' ? Number(value) : (value ?? 0)
}

function InlineState({ state }: { state: EditorState }) {
  const icon =
    state === 'saved' ? (
      <CheckCircle size={13} aria-hidden />
    ) : state === 'conflict' || state === 'error' ? (
      <WarningCircle size={13} aria-hidden />
    ) : null
  return (
    <span className="inline-flex items-center gap-1 font-mono text-micro uppercase tracking-[0.08em] text-muted-foreground" role="status">
      {icon}
      {state}
    </span>
  )
}

function EditorFailure({ state, record }: { state: EditorState; record: string }) {
  if (state === 'conflict') {
    return (
      <p className="text-xs text-warning" role="alert">
        This Project changed elsewhere. Reload its current version before saving the {record}.
      </p>
    )
  }
  if (state === 'error') {
    return (
      <p className="text-xs text-destructive" role="alert">
        The {record} was not saved. Check the fields and retry.
      </p>
    )
  }
  return null
}

function ProjectEditingRail({ projectId }: { projectId: string }) {
  const user = useAuthStore((state) => state.user)
  const project = useProjectQuery(projectId)
  const overview = useProjectOverviewQuery(projectId)
  const updateProject = useUpdateProject()
  const createTask = useCreateTask(projectId)
  const projectVersion = project.data?.version ?? 0
  const createDocument = useCreateWorkbenchDocument(projectId, user, projectVersion)
  const createDecision = useCreateWorkbenchDecision(projectId, user, projectVersion)
  const createMilestone = useCreateWorkbenchMilestone(projectId, user, projectVersion)
  const [name, setName] = useState('')
  const [projectState, setProjectState] = useState<EditorState>('idle')
  const [taskTitle, setTaskTitle] = useState('')
  const [taskDescription, setTaskDescription] = useState('')
  const [taskState, setTaskState] = useState<EditorState>('idle')
  const [documentTitle, setDocumentTitle] = useState('')
  const [documentKind, setDocumentKind] = useState<ProjectDocumentKind>('delivery_brief')
  const [documentState, setDocumentState] = useState<EditorState>('idle')
  const [decisionQuestion, setDecisionQuestion] = useState('')
  const [decisionOptions, setDecisionOptions] = useState('')
  const [decisionRationale, setDecisionRationale] = useState('')
  const [decisionState, setDecisionState] = useState<EditorState>('idle')
  const [milestoneName, setMilestoneName] = useState('')
  const [milestoneOutcome, setMilestoneOutcome] = useState('')
  const [milestoneState, setMilestoneState] = useState<EditorState>('idle')
  const [receipt, setReceipt] = useState<{ id: string; label: string }>()
  const projectWriteInFlight = useRef(false)
  const taskWriteInFlight = useRef(false)
  const documentWriteInFlight = useRef(false)
  const decisionWriteInFlight = useRef(false)
  const milestoneWriteInFlight = useRef(false)

  useEffect(() => {
    if (!project.data || projectState === 'dirty') return
    setName(project.data.name)
  }, [project.data, projectState])

  async function saveProject(event: React.FormEvent<HTMLFormElement>) {
    event.preventDefault()
    if (projectWriteInFlight.current) return
    if (!project.data || !name.trim()) return
    projectWriteInFlight.current = true
    setProjectState('saving')
    try {
      const updated = await updateProject.mutateAsync({
        projectId,
        body: { version: project.data.version, name: name.trim() },
      })
      setProjectState('saved')
      setReceipt({ id: updated.id, label: `Project metadata saved at version ${updated.version}` })
    } catch (error) {
      setProjectState(isApiStatus(error, 409) ? 'conflict' : 'error')
    } finally {
      projectWriteInFlight.current = false
    }
  }

  async function addTask(event: React.FormEvent<HTMLFormElement>) {
    event.preventDefault()
    if (taskWriteInFlight.current) return
    if (!taskTitle.trim()) return
    taskWriteInFlight.current = true
    setTaskState('saving')
    try {
      const task = await createTask.mutateAsync({
        title: taskTitle.trim(),
        description: taskDescription.trim() || null,
      })
      setTaskTitle('')
      setTaskDescription('')
      setTaskState('saved')
      setReceipt({ id: task.id, label: `Task created: ${task.title}` })
    } catch (error) {
      setTaskState(isApiStatus(error, 409) ? 'conflict' : 'error')
    } finally {
      taskWriteInFlight.current = false
    }
  }

  async function addDocument(event: React.FormEvent<HTMLFormElement>) {
    event.preventDefault()
    if (documentWriteInFlight.current) return
    if (!documentTitle.trim()) return
    documentWriteInFlight.current = true
    setDocumentState('saving')
    try {
      const document = await createDocument.mutateAsync({
        title: documentTitle.trim(),
        kind: documentKind,
      })
      setDocumentTitle('')
      setDocumentState('saved')
      setReceipt({ id: document.id, label: `Project artifact created: ${document.title}` })
    } catch (error) {
      setDocumentState(isApiStatus(error, 409) ? 'conflict' : 'error')
    } finally {
      documentWriteInFlight.current = false
    }
  }

  async function addDecision(event: React.FormEvent<HTMLFormElement>) {
    event.preventDefault()
    if (decisionWriteInFlight.current) return
    const options = decisionOptions
      .split('\n')
      .map((option) => option.trim())
      .filter(Boolean)
    if (!decisionQuestion.trim() || options.length < 2) return
    decisionWriteInFlight.current = true
    setDecisionState('saving')
    try {
      const decision = await createDecision.mutateAsync({
        question: decisionQuestion.trim(),
        options,
        rationale: decisionRationale.trim() || null,
      })
      setDecisionQuestion('')
      setDecisionOptions('')
      setDecisionRationale('')
      setDecisionState('saved')
      setReceipt({ id: decision.id, label: `Decision candidate created: ${decision.question}` })
    } catch (error) {
      setDecisionState(isApiStatus(error, 409) ? 'conflict' : 'error')
    } finally {
      decisionWriteInFlight.current = false
    }
  }

  async function addMilestone(event: React.FormEvent<HTMLFormElement>) {
    event.preventDefault()
    if (milestoneWriteInFlight.current) return
    if (!milestoneName.trim() || !milestoneOutcome.trim()) return
    milestoneWriteInFlight.current = true
    setMilestoneState('saving')
    try {
      const milestone = await createMilestone.mutateAsync({
        name: milestoneName.trim(),
        outcome: milestoneOutcome.trim(),
      })
      setMilestoneName('')
      setMilestoneOutcome('')
      setMilestoneState('saved')
      setReceipt({ id: milestone.id, label: `Milestone created: ${milestone.canonical_id}` })
    } catch (error) {
      setMilestoneState(isApiStatus(error, 409) ? 'conflict' : 'error')
    } finally {
      milestoneWriteInFlight.current = false
    }
  }

  if (project.isLoading || overview.isLoading) return <LoadingPanel label="Loading Project records" />
  if (project.isError || overview.isError) {
    return (
      <ErrorPanel
        title="Project records unavailable"
        description="The conversation remains intact. Retry before editing Project records."
        onRetry={() => {
          void project.refetch()
          void overview.refetch()
        }}
      />
    )
  }

  const counts = overview.data?.task_counts

  return (
    <aside className="min-h-0 overflow-y-auto border-l border-border-subtle bg-muted/10 p-4" aria-label="Project editing rail">
      <div className="space-y-4">
        <div>
          <SectionKicker>Project records</SectionKicker>
          <h2 className="mt-1 text-base font-semibold text-foreground">Edit beside the conversation</h2>
          <p className="mt-1 text-xs leading-5 text-muted-foreground">
            These controls call the same typed Project, artifact, Decision, milestone, and Task
            services as their canonical pages. Repository access remains unavailable outside a
            Task execution.
          </p>
        </div>

        {receipt ? (
          <Card className="border-success/30 bg-success/5 p-3" role="status">
            <div className="flex items-start gap-2">
              <CheckCircle size={16} className="mt-0.5 text-success" aria-hidden />
              <div className="min-w-0">
                <p className="text-xs font-semibold text-foreground">Durable receipt</p>
                <p className="mt-1 break-words text-xs text-muted-foreground">{receipt.label}</p>
              </div>
            </div>
          </Card>
        ) : null}

        <Card className="p-4">
          <form onSubmit={saveProject} className="space-y-3">
            <div className="flex items-center justify-between gap-2">
              <SectionKicker>Project metadata</SectionKicker>
              <InlineState state={projectState} />
            </div>
            <div className="space-y-2">
              <Label htmlFor="workbench-project-name">Project name</Label>
              <Input
                id="workbench-project-name"
                value={name}
                onChange={(event) => {
                  setName(event.target.value)
                  setProjectState('dirty')
                }}
              />
            </div>
            <EditorFailure state={projectState} record="Project metadata" />
            <Button type="submit" size="sm" disabled={projectState !== 'dirty' || updateProject.isPending}>
              Save metadata
            </Button>
          </form>
        </Card>

        <Card className="p-4">
          <form onSubmit={addDocument} className="space-y-3">
            <div className="flex items-center justify-between gap-2">
              <SectionKicker>Project artifact</SectionKicker>
              <InlineState state={documentState} />
            </div>
            <div className="space-y-2">
              <Label htmlFor="workbench-document-title">Title</Label>
              <Input
                id="workbench-document-title"
                value={documentTitle}
                onChange={(event) => {
                  setDocumentTitle(event.target.value)
                  setDocumentState('dirty')
                }}
                placeholder="A durable Project document"
              />
            </div>
            <div className="space-y-2">
              <Label htmlFor="workbench-document-kind">Kind</Label>
              <Select
                id="workbench-document-kind"
                value={documentKind}
                options={[
                  { value: 'delivery_brief', label: 'Delivery brief' },
                  { value: 'product_spec', label: 'Product spec' },
                  { value: 'design', label: 'Design' },
                  { value: 'architecture', label: 'Architecture' },
                  { value: 'execution_plan', label: 'Execution plan' },
                  { value: 'research', label: 'Research' },
                ]}
                onChange={(value) => {
                  setDocumentKind(value as ProjectDocumentKind)
                  setDocumentState('dirty')
                }}
              />
            </div>
            <EditorFailure state={documentState} record="Project artifact" />
            <Button
              type="submit"
              size="sm"
              disabled={!documentTitle.trim() || createDocument.isPending || !user}
            >
              Create artifact
            </Button>
          </form>
        </Card>

        <Card className="p-4">
          <form onSubmit={addDecision} className="space-y-3">
            <div className="flex items-center justify-between gap-2">
              <SectionKicker>Decision candidate</SectionKicker>
              <InlineState state={decisionState} />
            </div>
            <div className="space-y-2">
              <Label htmlFor="workbench-decision-question">Question</Label>
              <Input
                id="workbench-decision-question"
                value={decisionQuestion}
                onChange={(event) => {
                  setDecisionQuestion(event.target.value)
                  setDecisionState('dirty')
                }}
                placeholder="What must the Project decide?"
              />
            </div>
            <div className="space-y-2">
              <Label htmlFor="workbench-decision-options">Options (one per line)</Label>
              <Textarea
                id="workbench-decision-options"
                value={decisionOptions}
                onChange={(event) => {
                  setDecisionOptions(event.target.value)
                  setDecisionState('dirty')
                }}
                rows={3}
                placeholder={'Option A\nOption B'}
              />
            </div>
            <div className="space-y-2">
              <Label htmlFor="workbench-decision-rationale">Context (optional)</Label>
              <Textarea
                id="workbench-decision-rationale"
                value={decisionRationale}
                onChange={(event) => {
                  setDecisionRationale(event.target.value)
                  setDecisionState('dirty')
                }}
                rows={2}
              />
            </div>
            <EditorFailure state={decisionState} record="Decision candidate" />
            <Button
              type="submit"
              size="sm"
              disabled={
                !decisionQuestion.trim() ||
                decisionOptions.split('\n').filter((option) => option.trim()).length < 2 ||
                createDecision.isPending ||
                !user
              }
            >
              Create candidate
            </Button>
          </form>
        </Card>

        <Card className="p-4">
          <form onSubmit={addMilestone} className="space-y-3">
            <div className="flex items-center justify-between gap-2">
              <SectionKicker>Milestone draft</SectionKicker>
              <InlineState state={milestoneState} />
            </div>
            <div className="space-y-2">
              <Label htmlFor="workbench-milestone-name">Name</Label>
              <Input
                id="workbench-milestone-name"
                value={milestoneName}
                onChange={(event) => {
                  setMilestoneName(event.target.value)
                  setMilestoneState('dirty')
                }}
                placeholder="A bounded delivery checkpoint"
              />
            </div>
            <div className="space-y-2">
              <Label htmlFor="workbench-milestone-outcome">Outcome</Label>
              <Textarea
                id="workbench-milestone-outcome"
                value={milestoneOutcome}
                onChange={(event) => {
                  setMilestoneOutcome(event.target.value)
                  setMilestoneState('dirty')
                }}
                rows={3}
                placeholder="The observable result that closes this milestone"
              />
            </div>
            <EditorFailure state={milestoneState} record="Milestone draft" />
            <Button
              type="submit"
              size="sm"
              disabled={
                !milestoneName.trim() ||
                !milestoneOutcome.trim() ||
                createMilestone.isPending ||
                !user
              }
            >
              Create milestone draft
            </Button>
          </form>
        </Card>

        <Card className="p-4">
          <form onSubmit={addTask} className="space-y-3">
            <div className="flex items-center justify-between gap-2">
              <SectionKicker>New Task</SectionKicker>
              <InlineState state={taskState} />
            </div>
            <div className="space-y-2">
              <Label htmlFor="workbench-task-title">Title</Label>
              <Input
                id="workbench-task-title"
                value={taskTitle}
                onChange={(event) => {
                  setTaskTitle(event.target.value)
                  setTaskState('dirty')
                }}
                placeholder="A finite unit of work"
              />
            </div>
            <div className="space-y-2">
              <Label htmlFor="workbench-task-description">Description</Label>
              <Textarea
                id="workbench-task-description"
                value={taskDescription}
                onChange={(event) => {
                  setTaskDescription(event.target.value)
                  setTaskState('dirty')
                }}
                rows={3}
                placeholder="Outcome, boundaries, and acceptance evidence"
              />
            </div>
            <EditorFailure state={taskState} record="Task" />
            <Button type="submit" size="sm" disabled={!taskTitle.trim() || createTask.isPending}>
              Create Task
            </Button>
          </form>
        </Card>

        <Card className="p-4">
          <div className="flex items-center justify-between gap-2">
            <SectionKicker>Current state</SectionKicker>
            <StateBadge status={overview.data?.projection_state ?? 'current'} />
          </div>
          <div className="mt-3 grid grid-cols-3 gap-2">
            {[
              ['Tasks', count(counts?.total)],
              ['Active', count(counts?.active)],
              ['Blocked', count(counts?.blocked)],
            ].map(([label, value]) => (
              <div key={label} className="rounded-md border border-border-subtle bg-muted/30 p-2 text-center">
                <p className="text-base font-semibold text-foreground">{value}</p>
                <p className="text-micro text-muted-foreground">{label}</p>
              </div>
            ))}
          </div>
        </Card>

        <div className="grid gap-2">
          {[
            { label: 'Milestones', section: 'milestones', icon: Flag },
            { label: 'Decisions', section: 'decisions', icon: Scales },
            { label: 'Documents & artifacts', section: 'documents', icon: FileText },
            { label: 'Evidence & readiness', section: 'evidence', icon: ClipboardText },
          ].map(({ label, section, icon: Icon }) => (
            <Link
              key={section}
              to="/projects/$projectId/overview"
              params={{ projectId }}
              hash={section}
              className="flex items-center justify-between rounded-md border border-border-subtle bg-card px-3 py-2 text-xs font-medium text-foreground transition-colors hover:border-primary/40 hover:text-primary"
            >
              <span className="flex items-center gap-2"><Icon size={14} aria-hidden />{label}</span>
              <ArrowUpRight size={13} aria-hidden />
            </Link>
          ))}
        </div>
      </div>
    </aside>
  )
}

export function ProjectAgentWorkbench({
  projectId,
  children,
}: {
  projectId?: string
  children: ReactNode
}) {
  const [mobilePane, setMobilePane] = useState<'conversation' | 'project'>('conversation')
  const tabId = useId().replaceAll(':', '')
  if (!projectId) return <>{children}</>

  return (
    <div className="flex min-h-0 flex-1 flex-col">
      <div className="grid grid-cols-2 border-b border-border-subtle p-1 lg:hidden" role="tablist" aria-label="Workspace pane">
        {(['conversation', 'project'] as const).map((pane) => (
          <button
            key={pane}
            type="button"
            role="tab"
            id={`${tabId}-${pane}-tab`}
            aria-controls={`${tabId}-${pane}-panel`}
            aria-selected={mobilePane === pane}
            tabIndex={mobilePane === pane ? 0 : -1}
            className={`rounded-md px-3 py-2 text-xs font-medium ${mobilePane === pane ? 'bg-ember-surface text-primary' : 'text-muted-foreground'}`}
            onClick={() => setMobilePane(pane)}
            onKeyDown={(event) => {
              const nextPane =
                event.key === 'ArrowRight' || event.key === 'End'
                  ? 'project'
                  : event.key === 'ArrowLeft' || event.key === 'Home'
                    ? 'conversation'
                    : null
              if (!nextPane) return
              event.preventDefault()
              setMobilePane(nextPane)
              const tablist = event.currentTarget.parentElement
              window.requestAnimationFrame(() => {
                tablist
                  ?.querySelector<HTMLButtonElement>(`#${tabId}-${nextPane}-tab`)
                  ?.focus()
              })
            }}
          >
            {pane === 'conversation' ? 'Conversation' : 'Project'}
          </button>
        ))}
      </div>
      <div className="grid min-h-0 flex-1 lg:grid-cols-[minmax(0,1fr)_minmax(320px,38%)]">
        <div
          id={`${tabId}-conversation-panel`}
          role="tabpanel"
          aria-labelledby={`${tabId}-conversation-tab`}
          className={`min-h-0 ${mobilePane === 'conversation' ? 'flex' : 'hidden'} flex-col lg:flex`}
        >
          {children}
        </div>
        <div
          id={`${tabId}-project-panel`}
          role="tabpanel"
          aria-labelledby={`${tabId}-project-tab`}
          className={`${mobilePane === 'project' ? 'block' : 'hidden'} min-h-0 lg:block`}
        >
          <ProjectEditingRail projectId={projectId} />
        </div>
      </div>
    </div>
  )
}
