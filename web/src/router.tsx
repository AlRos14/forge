import { lazy, Suspense } from 'react'
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
import type { ExecutionViewerMode } from '@/components/execution-viewer'
import type { AccountTab } from '@/pages/AccountPage'
import type { ForgeSettingsTab } from '@/pages/ForgeSettingsPage'
import type { ProjectSettingsTab } from '@/pages/ProjectSettingsPage'
import type { TaskDetailTab } from '@/pages/TaskDetailPage'
import type { TaskListSortBy, TaskListSortOrder } from '@/pages/TaskListPage'
import type { PaginatedResponse, Project, TaskLifecycleState } from '@/types/generated'
import { useAuthStore } from '@/stores/auth'

const AccountPage = lazy(() =>
  import('@/pages/AccountPage').then((module) => ({ default: module.AccountPage })),
)
const AgentsPage = lazy(() =>
  import('@/pages/FederatedAgentsPage').then((module) => ({
    default: module.FederatedAgentsPage,
  })),
)
const BoardPage = lazy(() =>
  import('@/features/board/BoardPage').then((module) => ({ default: module.BoardPage })),
)
const DaemonsPage = lazy(() =>
  import('@/pages/DaemonsPage').then((module) => ({ default: module.DaemonsPage })),
)
const ExecutionDetailPage = lazy(() =>
  import('@/pages/ExecutionDetailPage').then((module) => ({ default: module.ExecutionDetailPage })),
)
const ForgeSettingsPage = lazy(() =>
  import('@/pages/ForgeSettingsPage').then((module) => ({ default: module.ForgeSettingsPage })),
)
const LoginPage = lazy(() =>
  import('@/pages/LoginPage').then((module) => ({ default: module.LoginPage })),
)
const OAuthAuthorizePage = lazy(() =>
  import('@/pages/OAuthAuthorizePage').then((module) => ({ default: module.OAuthAuthorizePage })),
)
const OperationsPage = lazy(() =>
  import('@/pages/OperationsPage').then((module) => ({ default: module.OperationsPage })),
)
const ProjectSettingsPage = lazy(() =>
  import('@/pages/ProjectSettingsPage').then((module) => ({
    default: module.ProjectSettingsPage,
  })),
)
const ProjectReleasePage = lazy(() =>
  import('@/pages/ProjectReleasePage').then((module) => ({
    default: module.ProjectReleasePage,
  })),
)
const RegisterPage = lazy(() =>
  import('@/pages/RegisterPage').then((module) => ({ default: module.RegisterPage })),
)
const TaskDetailPage = lazy(() =>
  import('@/pages/TaskDetailPage').then((module) => ({ default: module.TaskDetailPage })),
)
const TaskListPage = lazy(() =>
  import('@/pages/TaskListPage').then((module) => ({ default: module.TaskListPage })),
)

const accountTabs = new Set<AccountTab>(['profile', 'tokens'])
const forgeSettingsTabs = new Set<ForgeSettingsTab>(['server', 'agent', 'paths'])
const projectSettingsTabs = new Set<ProjectSettingsTab>([
  'general',
  'repos',
  'members',
  'mcp',
  'hooks',
  'danger',
])
const taskDetailTabs = new Set<TaskDetailTab>([
  'overview',
  'executions',
  'review',
  'terminal',
  'history',
])

function isAccountTab(value: string | undefined): value is AccountTab {
  return value !== undefined && accountTabs.has(value as AccountTab)
}

function isForgeSettingsTab(value: string | undefined): value is ForgeSettingsTab {
  return value !== undefined && forgeSettingsTabs.has(value as ForgeSettingsTab)
}

function isProjectSettingsTab(value: string | undefined): value is ProjectSettingsTab {
  return value !== undefined && projectSettingsTabs.has(value as ProjectSettingsTab)
}

function isTaskDetailTab(value: string | undefined): value is TaskDetailTab {
  return value !== undefined && taskDetailTabs.has(value as TaskDetailTab)
}

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

  const content = (
    <Suspense
      fallback={
        <div className="flex min-h-48 items-center justify-center" aria-label="Loading page" />
      }
    >
      <Outlet />
    </Suspense>
  )

  if (PUBLIC_PATHS.includes(pathname)) return content

  return <AppShell>{content}</AppShell>
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

const boardRoute = createRoute({
  getParentRoute: () => rootRoute,
  path: '/projects/$projectId/board',
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
  return <BoardPage projectId={projectId} />
}

type TaskListRouteSearch = {
  sort_by: TaskListSortBy
  sort_order: TaskListSortOrder
  lifecycle_state?: TaskLifecycleState
}

const taskListSortByValues = new Set<TaskListSortBy>([
  'title',
  'lifecycle_state',
  'priority',
  'task_type',
  'updated_at',
  'board_position',
])

const taskLifecycleStates = new Set<TaskLifecycleState>([
  'backlog', 'ready', 'active', 'blocked', 'ready_to_merge', 'merging', 'done', 'cancelled',
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
    lifecycle_state: typeof search.lifecycle_state === 'string' && taskLifecycleStates.has(search.lifecycle_state as TaskLifecycleState)
      ? search.lifecycle_state as TaskLifecycleState
      : undefined,
  }),
  component: TaskListRouteComponent,
})

function TaskListRouteComponent() {
  const { projectId } = taskListRoute.useParams()
  const search = taskListRoute.useSearch()
  const navigate = useNavigate({ from: '/projects/$projectId/tasks' })

  const setFilter = (lifecycleState?: TaskLifecycleState) => {
    void navigate({
      search: (prev) => ({
        ...prev,
        lifecycle_state: lifecycleState,
      }),
    })
  }

  return (
    <TaskListPage
      projectId={projectId}
      sortBy={search.sort_by}
      sortOrder={search.sort_order}
      lifecycleState={search.lifecycle_state}
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

function parseOptionalBool(value: unknown): boolean | undefined {
  if (value === true || value === 'true') return true
  if (value === false || value === 'false') return false
  return undefined
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
  return (
    <ExecutionDetailPage taskId={taskId} executionId={executionId} viewerMode={view ?? 'chat'} />
  )
}

const agentsRoute = createRoute({
  getParentRoute: () => rootRoute,
  path: '/agents',
  validateSearch: (
    search: Record<string, unknown>,
  ): {
    tab?: string
    project?: string
    provider?: string
    status?: string
    authorization?: string
    identity?: string
  } => ({
    tab: typeof search.tab === 'string' ? search.tab : undefined,
    project: typeof search.project === 'string' ? search.project : undefined,
    provider: typeof search.provider === 'string' ? search.provider : undefined,
    status: typeof search.status === 'string' ? search.status : undefined,
    authorization:
      typeof search.authorization === 'string' ? search.authorization : undefined,
    identity: typeof search.identity === 'string' ? search.identity : undefined,
  }),
  component: AgentsRouteComponent,
})

function AgentsRouteComponent() {
  return <AgentsPage />
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

const projectReleaseRoute = createRoute({
  getParentRoute: () => rootRoute,
  path: '/projects/$projectId/releases/$releaseId',
  component: ProjectReleaseRouteComponent,
})

function ProjectReleaseRouteComponent() {
  const { projectId, releaseId } = projectReleaseRoute.useParams()
  return <ProjectReleasePage projectId={projectId} releaseId={releaseId} />
}

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
  validateSearch: (
    search: Record<string, unknown>,
  ): { redirect: string | undefined; redirect_params?: string } => ({
    redirect: typeof search.redirect === 'string' ? search.redirect : undefined,
    redirect_params:
      typeof search.redirect_params === 'string' ? search.redirect_params : undefined,
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
  taskDetailRoute,
  taskDetailTabRoute,
  executionDetailRoute,
  agentsRoute,
  daemonsRoute,
  daemonDetailRoute,
  operationsRoute,
  projectReleaseRoute,
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
