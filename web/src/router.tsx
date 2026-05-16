import {
  Outlet,
  createRootRouteWithContext,
  createRoute,
  createRouter,
  redirect,
  useNavigate,
  useRouterState,
} from '@tanstack/react-router'
import type { QueryClient } from '@tanstack/react-query'
import { useHotkeys } from 'react-hotkeys-hook'
import { apiFetch } from '@/api/client'
import { qk } from '@/api/query-keys'
import { useSSE } from '@/api/sse'
import { AppShell } from '@/components/app-shell'
import { KanbanBoard } from '@/components/kanban-board'
import { AgentsPage } from '@/pages/AgentsPage'
import { DaemonsPage } from '@/pages/DaemonsPage'
import { OperationsPage } from '@/pages/OperationsPage'
import { ExecutionDetailPage } from '@/pages/ExecutionDetailPage'
import { ChatPage } from '@/pages/ChatPage'
import { ProjectSettingsPage, isProjectSettingsTab } from '@/pages/ProjectSettingsPage'
import { ForgeSettingsPage, isForgeSettingsTab } from '@/pages/ForgeSettingsPage'
import { AccountPage, isAccountTab } from '@/pages/AccountPage'
import { TaskDetailPage, isTaskDetailTab } from '@/pages/TaskDetailPage'
import type { ExecutionViewerMode } from '@/components/execution-viewer'
import { TaskListPage, type TaskListSortBy, type TaskListSortOrder } from '@/pages/TaskListPage'
import type { PaginatedResponse, Project } from '@/types/generated'
import { LoginPage } from '@/pages/LoginPage'
import { OAuthAuthorizePage } from '@/pages/OAuthAuthorizePage'
import { RegisterPage } from '@/pages/RegisterPage'
import { useAuthStore } from '@/stores/auth'

type RouterContext = {
  queryClient: QueryClient
}

const PUBLIC_PATHS = ['/login', '/register', '/oauth/authorize/consent']

function requireServerAdmin() {
  const { user } = useAuthStore.getState()
  if (!user?.is_admin) {
    throw redirect({ to: '/' })
  }
}

const rootRoute = createRootRouteWithContext<RouterContext>()({
  component: RootRouteComponent,
  beforeLoad: ({ location }) => {
    const { accessToken } = useAuthStore.getState()
    if (!accessToken && !PUBLIC_PATHS.includes(location.pathname)) {
      throw redirect({
        to: '/login',
        search: { redirect: location.pathname !== '/' ? location.pathname : undefined },
      })
    }
  },
})

function RootRouteComponent() {
  const queryClient = rootRoute.useRouteContext({ select: (ctx) => ctx.queryClient })
  const accessToken = useAuthStore((s) => s.accessToken)
  const pathname = useRouterState({ select: (s) => s.location.pathname })
  useSSE(queryClient, accessToken)

  if (PUBLIC_PATHS.includes(pathname)) {
    return <Outlet />
  }

  return (
    <AppShell>
      <Outlet />
    </AppShell>
  )
}

const indexRoute = createRoute({
  getParentRoute: () => rootRoute,
  path: '/',
  beforeLoad: async ({ context }) => {
    const projects = await context.queryClient.ensureQueryData({
      queryKey: qk.projects,
      queryFn: () => apiFetch<PaginatedResponse<Project>>('/projects'),
    })
    throw redirect({
      to: '/projects/$projectId/board',
      params: { projectId: projects.items[0]?.id ?? 'default' },
    })
  },
})

type BoardRouteSearch = {
  agentIds?: string
  priorityMax?: number
  priorityMin?: number
  q?: string
  task?: string
  blockedOnly?: boolean
  includeCancelled?: boolean
  includeArchived?: boolean
}

function parseOptionalNumber(value: unknown): number | undefined {
  if (typeof value === 'number' && Number.isFinite(value)) return value
  if (typeof value !== 'string' || value === '') return undefined
  const parsed = Number(value)
  return Number.isFinite(parsed) ? parsed : undefined
}

function parseOptionalBool(value: unknown): boolean | undefined {
  if (value === true || value === 'true') return true
  return undefined
}

const boardRoute = createRoute({
  getParentRoute: () => rootRoute,
  path: '/projects/$projectId/board',
  validateSearch: (search: Record<string, unknown>): BoardRouteSearch => ({
    agentIds: typeof search.agentIds === 'string' ? search.agentIds : undefined,
    priorityMax: parseOptionalNumber(search.priorityMax),
    priorityMin: parseOptionalNumber(search.priorityMin),
    q: typeof search.q === 'string' ? search.q : undefined,
    task: typeof search.task === 'string' ? search.task : undefined,
    blockedOnly: parseOptionalBool(search.blockedOnly),
    includeCancelled: parseOptionalBool(search.includeCancelled),
    includeArchived: parseOptionalBool(search.includeArchived),
  }),
  component: BoardRouteComponent,
})

function BoardRouteComponent() {
  const { projectId } = boardRoute.useParams()
  useHotkeys('c', () => window.dispatchEvent(new CustomEvent('forge:create-task')), {
    enableOnFormTags: false,
  })
  useHotkeys('/', () => window.dispatchEvent(new CustomEvent('forge:focus-board-search')), {
    enableOnFormTags: false,
    preventDefault: true,
  })
  return <KanbanBoard projectId={projectId} />
}

type TaskListRouteSearch = {
  sort_by: TaskListSortBy
  sort_order: TaskListSortOrder
  agentIds?: string
  blockedOnly?: boolean
  includeCancelled?: boolean
  includeArchived?: boolean
  priorityMin?: number
  priorityMax?: number
}

const taskListSortByValues = new Set<TaskListSortBy>([
  'title',
  'status',
  'agent',
  'priority',
  'task_type',
  'updated_at',
])

const taskListRoute = createRoute({
  getParentRoute: () => rootRoute,
  path: '/projects/$projectId/tasks',
  validateSearch: (search: Record<string, unknown>): TaskListRouteSearch => ({
    sort_by:
      typeof search.sort_by === 'string' &&
      taskListSortByValues.has(search.sort_by as TaskListSortBy)
        ? (search.sort_by as TaskListSortBy)
        : 'updated_at',
    sort_order: search.sort_order === 'asc' ? 'asc' : 'desc',
    agentIds: typeof search.agentIds === 'string' ? search.agentIds : undefined,
    blockedOnly: parseOptionalBool(search.blockedOnly),
    includeCancelled: parseOptionalBool(search.includeCancelled),
    includeArchived: parseOptionalBool(search.includeArchived),
    priorityMin: parseOptionalNumber(search.priorityMin),
    priorityMax: parseOptionalNumber(search.priorityMax),
  }),
  component: TaskListRouteComponent,
})

function TaskListRouteComponent() {
  const { projectId } = taskListRoute.useParams()
  const search = taskListRoute.useSearch()
  const navigate = useNavigate({ from: '/projects/$projectId/tasks' })

  const agentIds = search.agentIds ? search.agentIds.split(',').filter(Boolean) : []

  const setFilter = (patch: { agentIds?: string[]; blockedOnly?: boolean; includeCancelled?: boolean; includeArchived?: boolean; priorityMin?: number; priorityMax?: number }) => {
    void navigate({
      search: (prev) => ({
        ...prev,
        agentIds: 'agentIds' in patch
          ? (patch.agentIds && patch.agentIds.length > 0 ? patch.agentIds.join(',') : undefined)
          : prev.agentIds,
        blockedOnly: 'blockedOnly' in patch ? patch.blockedOnly || undefined : prev.blockedOnly,
        includeCancelled: 'includeCancelled' in patch ? patch.includeCancelled || undefined : prev.includeCancelled,
        includeArchived: 'includeArchived' in patch ? patch.includeArchived || undefined : prev.includeArchived,
        priorityMin: 'priorityMin' in patch ? patch.priorityMin : prev.priorityMin,
        priorityMax: 'priorityMax' in patch ? patch.priorityMax : prev.priorityMax,
      }),
    })
  }

  return (
    <TaskListPage
      projectId={projectId}
      sortBy={search.sort_by}
      sortOrder={search.sort_order}
      agentIds={agentIds}
      blockedOnly={search.blockedOnly ?? false}
      includeCancelled={search.includeCancelled ?? false}
      includeArchived={search.includeArchived ?? false}
      priorityMin={search.priorityMin}
      priorityMax={search.priorityMax}
      onSortChange={(sortBy, sortOrder) => {
        void navigate({ search: (prev) => ({ ...prev, sort_by: sortBy, sort_order: sortOrder }) })
      }}
      onFilterChange={setFilter}
    />
  )
}

const taskDetailRoute = createRoute({
  getParentRoute: () => rootRoute,
  path: '/tasks/$taskId',
  component: TaskDetailRouteComponent,
})

function TaskDetailRouteComponent() {
  const { taskId } = taskDetailRoute.useParams()
  return <TaskDetailPage taskId={taskId} />
}

const taskDetailTabRoute = createRoute({
  getParentRoute: () => rootRoute,
  path: '/tasks/$taskId/$tab',
  beforeLoad: ({ params }) => {
    if (!isTaskDetailTab(params.tab)) {
      throw redirect({
        to: '/tasks/$taskId',
        params: { taskId: params.taskId },
      })
    }
  },
  component: TaskDetailTabRouteComponent,
})

function TaskDetailTabRouteComponent() {
  const { taskId, tab } = taskDetailTabRoute.useParams()
  return <TaskDetailPage taskId={taskId} initialTab={isTaskDetailTab(tab) ? tab : 'overview'} />
}

type ExecutionDetailRouteSearch = {
  followUp?: boolean
  view?: ExecutionViewerMode
}

const executionDetailRoute = createRoute({
  getParentRoute: () => rootRoute,
  path: '/tasks/$taskId/executions/$executionId',
  validateSearch: (search: Record<string, unknown>): ExecutionDetailRouteSearch => ({
    followUp: parseOptionalBool(search.followUp),
    view: search.view === 'raw' ? 'raw' : undefined,
  }),
  component: ExecutionDetailRouteComponent,
})

function ExecutionDetailRouteComponent() {
  const { taskId, executionId } = executionDetailRoute.useParams()
  const { view } = executionDetailRoute.useSearch()
  return <ExecutionDetailPage taskId={taskId} executionId={executionId} viewerMode={view ?? 'chat'} />
}

const agentsRoute = createRoute({
  getParentRoute: () => rootRoute,
  path: '/agents',
  component: AgentsRouteComponent,
})

function AgentsRouteComponent() {
  return <AgentsPage />
}

const agentCreateRoute = createRoute({
  getParentRoute: () => rootRoute,
  path: '/agents/new',
  component: AgentCreateRouteComponent,
})

function AgentCreateRouteComponent() {
  return <AgentsPage mode="create" />
}

const agentDetailRoute = createRoute({
  getParentRoute: () => rootRoute,
  path: '/agents/$agentId',
  component: AgentDetailRouteComponent,
})

function AgentDetailRouteComponent() {
  const { agentId } = agentDetailRoute.useParams()
  return <AgentsPage selectedAgentId={agentId} />
}

const agentEditRoute = createRoute({
  getParentRoute: () => rootRoute,
  path: '/agents/$agentId/edit',
  component: AgentEditRouteComponent,
})

function AgentEditRouteComponent() {
  const { agentId } = agentEditRoute.useParams()
  return <AgentsPage selectedAgentId={agentId} mode="edit" />
}

const daemonsRoute = createRoute({
  getParentRoute: () => rootRoute,
  path: '/daemons',
  beforeLoad: requireServerAdmin,
  component: DaemonsPage,
})

const daemonDetailRoute = createRoute({
  getParentRoute: () => rootRoute,
  path: '/daemons/$daemonId',
  beforeLoad: requireServerAdmin,
  component: DaemonDetailRouteComponent,
})

function DaemonDetailRouteComponent() {
  const { daemonId } = daemonDetailRoute.useParams()
  return <DaemonsPage selectedDaemonId={daemonId} />
}

const operationsRoute = createRoute({
  getParentRoute: () => rootRoute,
  path: '/operations',
  beforeLoad: requireServerAdmin,
  component: OperationsPage,
})

const projectSettingsRoute = createRoute({
  getParentRoute: () => rootRoute,
  path: '/projects/$projectId/settings',
  component: ProjectSettingsRouteComponent,
})

function ProjectSettingsRouteComponent() {
  const { projectId } = projectSettingsRoute.useParams()
  return <ProjectSettingsPage projectId={projectId} />
}

const projectSettingsTabRoute = createRoute({
  getParentRoute: () => rootRoute,
  path: '/projects/$projectId/settings/$tab',
  beforeLoad: ({ params }) => {
    if (params.tab === 'integrations') {
      throw redirect({
        to: '/projects/$projectId/settings/$tab',
        params: { projectId: params.projectId, tab: 'repos' },
      })
    }
    if (!isProjectSettingsTab(params.tab)) {
      throw redirect({
        to: '/projects/$projectId/settings',
        params: { projectId: params.projectId },
      })
    }
  },
  component: ProjectSettingsTabRouteComponent,
})

function ProjectSettingsTabRouteComponent() {
  const { projectId, tab } = projectSettingsTabRoute.useParams()
  return (
    <ProjectSettingsPage
      projectId={projectId}
      initialTab={isProjectSettingsTab(tab) ? tab : 'general'}
    />
  )
}

const projectChatRoute = createRoute({
  getParentRoute: () => rootRoute,
  path: '/projects/$projectId/chat',
  component: ProjectChatRouteComponent,
})

function ProjectChatRouteComponent() {
  const { projectId } = projectChatRoute.useParams()
  return <ChatPage projectId={projectId} />
}

const projectChatConversationRoute = createRoute({
  getParentRoute: () => rootRoute,
  path: '/projects/$projectId/chat/$conversationId',
  component: ProjectChatConversationRouteComponent,
})

function ProjectChatConversationRouteComponent() {
  const { projectId, conversationId } = projectChatConversationRoute.useParams()
  return <ChatPage projectId={projectId} conversationId={conversationId} />
}

const forgeSettingsRoute = createRoute({
  getParentRoute: () => rootRoute,
  path: '/settings',
  beforeLoad: requireServerAdmin,
  component: ForgeSettingsPage,
})

const forgeSettingsTabRoute = createRoute({
  getParentRoute: () => rootRoute,
  path: '/settings/$tab',
  beforeLoad: ({ params }) => {
    requireServerAdmin()
    if (!isForgeSettingsTab(params.tab)) {
      throw redirect({ to: '/settings' })
    }
  },
  component: ForgeSettingsTabRouteComponent,
})

function ForgeSettingsTabRouteComponent() {
  const { tab } = forgeSettingsTabRoute.useParams()
  return <ForgeSettingsPage initialTab={isForgeSettingsTab(tab) ? tab : 'server'} />
}

const accountRoute = createRoute({
  getParentRoute: () => rootRoute,
  path: '/account',
  component: AccountPage,
})

const accountTabRoute = createRoute({
  getParentRoute: () => rootRoute,
  path: '/account/$tab',
  beforeLoad: ({ params }) => {
    if (!isAccountTab(params.tab)) {
      throw redirect({ to: '/account' })
    }
  },
  component: AccountTabRouteComponent,
})

function AccountTabRouteComponent() {
  const { tab } = accountTabRoute.useParams()
  return <AccountPage initialTab={isAccountTab(tab) ? tab : 'profile'} />
}

const loginRoute = createRoute({
  getParentRoute: () => rootRoute,
  path: '/login',
  validateSearch: (search: Record<string, unknown>): { redirect: string | undefined; redirect_params?: string } => ({
    redirect: typeof search.redirect === 'string' ? search.redirect : undefined,
    redirect_params: typeof search.redirect_params === 'string' ? search.redirect_params : undefined,
  }),
  beforeLoad: () => {
    const { accessToken } = useAuthStore.getState()
    if (accessToken) throw redirect({ to: '/' })
  },
  component: LoginPage,
})

const oauthAuthorizeRoute = createRoute({
  getParentRoute: () => rootRoute,
  path: '/oauth/authorize/consent',
  validateSearch: (raw) => raw as Partial<Record<string, string>>,
  component: OAuthAuthorizePage,
})

const registerRoute = createRoute({
  getParentRoute: () => rootRoute,
  path: '/register',
  beforeLoad: () => {
    const { accessToken } = useAuthStore.getState()
    if (accessToken) throw redirect({ to: '/' })
  },
  component: RegisterPage,
})

const routeTree = rootRoute.addChildren([
  loginRoute,
  oauthAuthorizeRoute,
  registerRoute,
  indexRoute,
  boardRoute,
  taskListRoute,
  projectChatRoute,
  projectChatConversationRoute,
  taskDetailRoute,
  taskDetailTabRoute,
  executionDetailRoute,
  agentsRoute,
  agentCreateRoute,
  agentEditRoute,
  agentDetailRoute,
  daemonsRoute,
  daemonDetailRoute,
  operationsRoute,
  projectSettingsRoute,
  projectSettingsTabRoute,
  forgeSettingsRoute,
  forgeSettingsTabRoute,
  accountRoute,
  accountTabRoute,
])

declare module '@tanstack/react-router' {
  interface Register {
    router: ReturnType<typeof createAppRouter>
  }
}

export function createAppRouter(queryClient: QueryClient) {
  return createRouter({
    routeTree,
    context: { queryClient },
    defaultPreload: 'intent',
    defaultErrorComponent: ({ error }) => (
      <div className="rounded-md border border-red-400 bg-red-50 p-4 text-sm text-red-900">
        {(error as Error).message}
      </div>
    ),
    defaultPendingComponent: () => <div className="text-sm text-muted-foreground">Loading...</div>,
    defaultNotFoundComponent: () => <div className="text-sm text-muted-foreground">Not found</div>,
  })
}
