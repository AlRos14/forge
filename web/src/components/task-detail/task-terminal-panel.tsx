import { FitAddon } from '@xterm/addon-fit'
import { Terminal } from '@xterm/xterm'
import '@xterm/xterm/css/xterm.css'
import { useCallback, useEffect, useMemo, useRef, useState } from 'react'
import { toast } from 'sonner'
import {
  createTerminalSession,
  getTerminalAvailability,
  issueTerminalAttachToken,
  listTerminalSessions,
  resizeTerminalSession,
  terminalWebSocketUrl,
  terminateTerminalSession,
} from '@/api/terminals'
import { Button } from '@/components/ui/button'
import { Skeleton } from '@/components/ui/skeleton'
import { getApiErrorMessage } from '@/lib/api-error'
import { cn } from '@/lib/cn'
import { productTerm } from '@/lib/i18n'
import { useAuthStore } from '@/stores/auth'
import type {
  TerminalAvailability,
  TerminalServerFrame,
  TerminalSessionResponse,
  TerminalSessionStatus,
} from '@/types/generated'

type TerminalConnectionStatus =
  | 'idle'
  | 'connecting'
  | 'connected'
  | 'disconnected'
  | 'exited'
  | 'terminated'
  | 'error'

type TaskTerminalPanelProps = {
  taskId: string
  className?: string
}

const liveSessionStatuses = new Set<TerminalSessionStatus>(['starting', 'running'])

function isLiveSession(session: TerminalSessionResponse | null): boolean {
  return Boolean(session && liveSessionStatuses.has(session.status))
}

function terminalStatusLabel(status: TerminalConnectionStatus): string {
  switch (status) {
    case 'connecting':
      return 'Connecting'
    case 'connected':
      return 'Connected'
    case 'disconnected':
      return 'Disconnected'
    case 'exited':
      return 'Exited'
    case 'terminated':
      return 'Terminated'
    case 'error':
      return 'Error'
    case 'idle':
    default:
      return 'Idle'
  }
}

function availabilityBlockMessage(availability: TerminalAvailability): string | null {
  if (!availability.enabled) return 'Terminal access is disabled in server settings.'
  if (!availability.workspace_ready) return 'Task workspace not ready.'
  if (availability.active_execution) {
    return `A managed ${productTerm('run').toLowerCase()} is running; terminal disabled until it finishes.`
  }
  if (!availability.daemon_reachable) {
    return `Workspace ${productTerm('runtime').toLowerCase()} unavailable.`
  }
  return null
}

export function TaskTerminalPanel({ taskId, className }: TaskTerminalPanelProps) {
  const currentUserId = useAuthStore((state) => state.user?.id)
  const terminalHostRef = useRef<HTMLDivElement | null>(null)
  const terminalRef = useRef<Terminal | null>(null)
  const fitAddonRef = useRef<FitAddon | null>(null)
  const wsRef = useRef<WebSocket | null>(null)
  const inputDisposableRef = useRef<{ dispose: () => void } | null>(null)
  const resizeObserverRef = useRef<ResizeObserver | null>(null)
  const resizeTimerRef = useRef<number | null>(null)
  const currentSessionIdRef = useRef<string | null>(null)
  const inputDisabledRef = useRef(false)
  const reattachPromiseRef = useRef<Promise<void> | null>(null)

  const [availability, setAvailability] = useState<TerminalAvailability | null>(null)
  const [sessions, setSessions] = useState<TerminalSessionResponse[]>([])
  const [activeSession, setActiveSession] = useState<TerminalSessionResponse | null>(null)
  const [isLoading, setIsLoading] = useState(true)
  const [isStarting, setIsStarting] = useState(false)
  const [isReattaching, setIsReattaching] = useState(false)
  const [isTerminating, setIsTerminating] = useState(false)
  const [showExistingSessions, setShowExistingSessions] = useState(false)
  const [connectionStatus, setConnectionStatus] = useState<TerminalConnectionStatus>('idle')
  const [errorMessage, setErrorMessage] = useState<string | null>(null)

  const queueFitAndResize = useCallback((sessionId?: string | null) => {
    if (resizeTimerRef.current !== null) {
      window.clearTimeout(resizeTimerRef.current)
    }
    resizeTimerRef.current = window.setTimeout(() => {
      resizeTimerRef.current = null
      const terminal = terminalRef.current
      const fitAddon = fitAddonRef.current
      if (!terminal || !fitAddon) return

      try {
        fitAddon.fit()
      } catch {
        return
      }

      const targetSessionId = sessionId ?? currentSessionIdRef.current
      if (!targetSessionId) return

      void resizeTerminalSession(targetSessionId, {
        rows: terminal.rows,
        cols: terminal.cols,
      })
        .then((session) => {
          setActiveSession(session)
          setSessions((current) =>
            current.map((item) => (item.id === session.id ? session : item)),
          )
        })
        .catch(() => {
          // Resize failures are non-fatal; the WebSocket stream can keep running.
        })
    }, 150)
  }, [])

  const ensureTerminal = useCallback(() => {
    if (terminalRef.current) return terminalRef.current
    if (!terminalHostRef.current) return null

    const terminal = new Terminal({
      convertEol: true,
      cursorBlink: true,
      fontFamily:
        'ui-monospace, SFMono-Regular, Menlo, Monaco, Consolas, "Liberation Mono", monospace',
      fontSize: 13,
      scrollback: 20_000,
      theme: {
        background: '#050505',
        foreground: '#d4d4d4',
        cursor: '#f97316',
        selectionBackground: '#374151',
      },
    })
    const fitAddon = new FitAddon()
    terminal.loadAddon(fitAddon)
    terminal.open(terminalHostRef.current)

    inputDisposableRef.current = terminal.onData((data) => {
      const ws = wsRef.current
      if (inputDisabledRef.current || !ws || ws.readyState !== WebSocket.OPEN) return
      ws.send(JSON.stringify({ type: 'input', data: btoa(data) }))
    })

    if (typeof ResizeObserver !== 'undefined') {
      const resizeObserver = new ResizeObserver(() => queueFitAndResize())
      resizeObserver.observe(terminalHostRef.current)
      resizeObserverRef.current = resizeObserver
    }

    terminalRef.current = terminal
    fitAddonRef.current = fitAddon
    queueFitAndResize()
    return terminal
  }, [queueFitAndResize])

  const refreshTerminalState = useCallback(async () => {
    const [nextAvailability, nextSessions] = await Promise.all([
      getTerminalAvailability(taskId),
      listTerminalSessions(taskId),
    ])
    setAvailability(nextAvailability)
    setSessions(nextSessions)
    setActiveSession((current) => {
      if (!current) return current
      return nextSessions.find((session) => session.id === current.id) ?? current
    })
  }, [taskId])

  useEffect(() => {
    let cancelled = false
    setIsLoading(true)
    setErrorMessage(null)
    void refreshTerminalState()
      .catch((error) => {
        if (!cancelled) {
          setErrorMessage(getApiErrorMessage(error, 'Unable to load terminal state.'))
        }
      })
      .finally(() => {
        if (!cancelled) setIsLoading(false)
      })
    return () => {
      cancelled = true
    }
  }, [refreshTerminalState])

  useEffect(() => {
    return () => {
      if (resizeTimerRef.current !== null) {
        window.clearTimeout(resizeTimerRef.current)
      }
      resizeObserverRef.current?.disconnect()
      inputDisposableRef.current?.dispose()
      wsRef.current?.close()
      terminalRef.current?.dispose()
      resizeObserverRef.current = null
      inputDisposableRef.current = null
      wsRef.current = null
      terminalRef.current = null
      fitAddonRef.current = null
      currentSessionIdRef.current = null
    }
  }, [])

  const runningSession = useMemo(
    () =>
      sessions.find(
        (session) =>
          session.status === 'running' &&
          (!currentUserId || session.created_by_user_id === currentUserId),
      ) ?? null,
    [currentUserId, sessions],
  )
  const limitReachedForTask = Boolean(
    availability && availability.session_count_for_task >= availability.max_sessions_per_task,
  )
  const limitReachedForUser = Boolean(
    availability && availability.session_count_for_user >= availability.max_sessions_per_user,
  )
  const limitReached = limitReachedForTask || limitReachedForUser
  const canStartNewSession = Boolean(availability?.can_create && !limitReached)
  const blockMessage = availability ? availabilityBlockMessage(availability) : null

  const connectToSession = useCallback(
    async (session: TerminalSessionResponse, attachToken?: string) => {
      setErrorMessage(null)
      const terminal = ensureTerminal()
      if (!terminal) {
        setErrorMessage('Terminal surface failed to initialize.')
        return
      }

      wsRef.current?.close()
      inputDisabledRef.current = false
      currentSessionIdRef.current = session.id
      setActiveSession(session)
      setConnectionStatus('connecting')
      queueFitAndResize(session.id)

      let token: string
      if (attachToken !== undefined) {
        token = attachToken
      } else {
        const response = await issueTerminalAttachToken(session.id)
        token = response.attach_token
      }
      const ws = new WebSocket(terminalWebSocketUrl(session.id, token))
      wsRef.current = ws

      ws.onopen = () => {
        setConnectionStatus('connected')
        queueFitAndResize(session.id)
      }
      ws.onmessage = (event) => {
        try {
          const frame = JSON.parse(String(event.data)) as TerminalServerFrame
          if (frame.type === 'output') {
            terminal.write(atob(frame.data))
            return
          }
          if (frame.type === 'exit') {
            inputDisabledRef.current = true
            setConnectionStatus('exited')
            setActiveSession((current) =>
              current
                ? {
                    ...current,
                    status: 'exited',
                    exit_code: frame.exit_code,
                    exit_signal: frame.signal,
                    exit_reason: frame.reason,
                  }
                : current,
            )
            terminal.write(
              `\r\n[terminal exited${
                frame.exit_code == null ? '' : ` with code ${frame.exit_code}`
              }]\r\n`,
            )
            return
          }
          if (frame.type === 'error') {
            inputDisabledRef.current = true
            setConnectionStatus('error')
            setErrorMessage(frame.message)
            terminal.write(`\r\n[terminal error: ${frame.message}]\r\n`)
          }
        } catch (error) {
          inputDisabledRef.current = true
          setConnectionStatus('error')
          setErrorMessage(getApiErrorMessage(error, 'Invalid terminal frame.'))
        }
      }
      ws.onerror = () => {
        inputDisabledRef.current = true
        setConnectionStatus('error')
        setErrorMessage('Terminal WebSocket connection failed.')
      }
      ws.onclose = () => {
        setConnectionStatus((current) =>
          current === 'exited' || current === 'terminated' || current === 'error'
            ? current
            : 'disconnected',
        )
      }
    },
    [ensureTerminal, queueFitAndResize],
  )

  const handleStart = async () => {
    setIsStarting(true)
    setErrorMessage(null)
    try {
      const terminal = ensureTerminal()
      const rows = terminal?.rows ?? 24
      const cols = terminal?.cols ?? 80
      const result = await createTerminalSession(taskId, { rows, cols })
      setSessions((current) => [result.session, ...current])
      await connectToSession(result.session, result.attach.attach_token)
      void refreshTerminalState()
    } catch (error) {
      const message = getApiErrorMessage(error, 'Terminal session failed to start.')
      setErrorMessage(message)
      toast.error(message)
    } finally {
      setIsStarting(false)
    }
  }

  const handleReattach = async (session: TerminalSessionResponse | null = runningSession) => {
    if (!session || reattachPromiseRef.current) return
    setIsReattaching(true)
    setErrorMessage(null)
    const reattachPromise = connectToSession(session)
    reattachPromiseRef.current = reattachPromise
    try {
      await reattachPromise
    } catch (error) {
      const message = getApiErrorMessage(error, 'Terminal session failed to reattach.')
      setErrorMessage(message)
      toast.error(message)
    } finally {
      if (reattachPromiseRef.current === reattachPromise) {
        reattachPromiseRef.current = null
      }
      setIsReattaching(false)
    }
  }

  const handleTerminate = async () => {
    if (!activeSession) return
    setIsTerminating(true)
    setErrorMessage(null)
    try {
      const session = await terminateTerminalSession(activeSession.id, 'terminated from web terminal')
      inputDisabledRef.current = true
      wsRef.current?.close()
      setConnectionStatus('terminated')
      setActiveSession(session)
      setSessions((current) =>
        current.map((item) => (item.id === session.id ? session : item)),
      )
      terminalRef.current?.write('\r\n[terminal terminated]\r\n')
      void refreshTerminalState()
    } catch (error) {
      const message = getApiErrorMessage(error, 'Terminal termination failed.')
      setErrorMessage(message)
      toast.error(message)
    } finally {
      setIsTerminating(false)
    }
  }

  const handleViewExistingSessions = async () => {
    setShowExistingSessions(true)
    try {
      const allSessions = await listTerminalSessions(taskId, { includeEnded: true })
      setSessions(allSessions)
    } catch (error) {
      setErrorMessage(getApiErrorMessage(error, 'Unable to load terminal sessions.'))
    }
  }

  if (isLoading) {
    return (
      <div className={cn('space-y-4 p-6', className)}>
        <Skeleton className="h-8 w-52" />
        <Skeleton className="h-[360px] w-full" />
      </div>
    )
  }

  if (blockMessage) {
    return (
      <div className={cn('p-6', className)}>
        <div className="rounded-lg border border-dashed bg-muted/20 px-4 py-6 text-sm text-muted-foreground">
          {blockMessage}
        </div>
      </div>
    )
  }

  return (
    <div className={cn('flex h-full min-h-[520px] flex-col gap-4 p-6', className)}>
      <div className="flex flex-wrap items-center justify-between gap-3 rounded-lg border bg-background px-4 py-3">
        <div className="min-w-0 space-y-1">
          <p className="text-sm font-semibold text-foreground">Terminal</p>
          <p className="font-mono text-xs text-muted-foreground">
            {activeSession ? activeSession.id : 'No session attached.'}
          </p>
        </div>
        <div className="flex flex-wrap items-center gap-2">
          <span className="rounded-full border px-2 py-0.5 font-mono text-micro font-semibold uppercase tracking-[0.8px] text-muted-foreground">
            {terminalStatusLabel(connectionStatus)}
          </span>
          {runningSession && connectionStatus !== 'connected' ? (
            <Button
              disabled={isReattaching || isStarting}
              size="sm"
              variant="outline"
              onClick={() => {
                void handleReattach()
              }}
            >
              Reattach to running session
            </Button>
          ) : null}
          {canStartNewSession ? (
            <Button disabled={isStarting || isReattaching} size="sm" onClick={handleStart}>
              Start new session
            </Button>
          ) : null}
          {isLiveSession(activeSession) ? (
            <Button
              disabled={isTerminating}
              size="sm"
              variant="destructive"
              onClick={handleTerminate}
            >
              Terminate
            </Button>
          ) : null}
        </div>
      </div>

      {limitReached ? (
        <div className="flex flex-wrap items-center justify-between gap-3 rounded-lg border border-amber-200 bg-amber-50 px-4 py-3 text-sm text-amber-950 dark:border-amber-900/60 dark:bg-amber-950/30 dark:text-amber-100">
          <span>
            {limitReachedForTask
              ? 'Terminal session limit reached for this task.'
              : 'Terminal session limit reached for this user.'}
          </span>
          <Button size="sm" variant="outline" onClick={handleViewExistingSessions}>
            View existing sessions
          </Button>
        </div>
      ) : !availability?.can_create ? (
        <div className="rounded-lg border border-dashed bg-muted/20 px-4 py-3 text-sm text-muted-foreground">
          {availability?.reason ?? 'Terminal cannot be started right now.'}
        </div>
      ) : null}

      {errorMessage ? (
        <div className="rounded-lg border border-destructive/30 bg-destructive/10 px-4 py-3 text-sm text-destructive">
          {errorMessage}
        </div>
      ) : null}

      <div className="flex min-h-[360px] flex-1 flex-col overflow-hidden rounded-lg border bg-black">
        <div ref={terminalHostRef} className="min-h-[360px] flex-1 overflow-hidden p-2" />
      </div>

      {showExistingSessions ? (
        <div className="rounded-lg border bg-background">
          <div className="border-b px-4 py-2">
            <p className="text-sm font-semibold text-foreground">Existing sessions</p>
          </div>
          <div className="divide-y">
            {sessions.length > 0 ? (
              sessions.map((session) => (
                <div
                  key={session.id}
                  className="flex flex-wrap items-center justify-between gap-3 px-4 py-3 text-sm"
                >
                  <div className="min-w-0">
                    <p className="truncate font-mono text-xs text-foreground">{session.id}</p>
                    <p className="text-xs text-muted-foreground">
                      {session.status.replace(/_/g, ' ')} · {session.rows}x{session.cols}
                    </p>
                  </div>
                  {session.status === 'running' &&
                  (!currentUserId || session.created_by_user_id === currentUserId) ? (
                    <Button
                      size="sm"
                      variant="outline"
                      onClick={() => {
                        void handleReattach(session)
                      }}
                    >
                      Reattach
                    </Button>
                  ) : null}
                </div>
              ))
            ) : (
              <p className="px-4 py-3 text-sm text-muted-foreground">No terminal sessions yet.</p>
            )}
          </div>
        </div>
      ) : null}
    </div>
  )
}
