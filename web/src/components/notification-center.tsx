import { useMemo, useState, useEffect } from 'react'
import { Bell, Gear, Check, BellSimpleRinging } from '@phosphor-icons/react'
import { useNavigate } from '@tanstack/react-router'
import { toast } from 'sonner'
import {
  useMarkAllNotificationsRead,
  useMarkNotificationRead,
  useNotificationsQuery,
  useUnreadNotificationsCountQuery,
} from '@/api/hooks'
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuSeparator,
  DropdownMenuTrigger,
} from '@/components/ui/dropdown-menu'
import { Switch } from '@/components/ui/switch'
import {
  isProjectMuted,
  loadNotificationPreferences,
  saveNotificationPreferences,
  type NotificationPreferences,
} from '@/lib/notification-preferences'
import { shouldSuppressToast } from '@/lib/notification-toast-suppression'
import type { NotificationResponse } from '@/types/generated'

type NotificationEventDetail = {
  entity_id?: string
  notification_id?: string
  project_id?: string
  task_id?: string | null
  event_type?: string
  title?: string
  body?: string
}

function severityFor(eventType: string | undefined): 'success' | 'warning' | 'error' {
  if (eventType === 'task.done' || eventType === 'review.passed') return 'success'
  if (eventType === 'task.failed' || eventType === 'task.recovery_required') return 'error'
  return 'warning'
}

function relativeTime(iso: string): string {
  const date = new Date(iso)
  const diffMs = date.getTime() - Date.now()
  const rtf = new Intl.RelativeTimeFormat(undefined, { numeric: 'auto' })
  const sec = Math.round(diffMs / 1000)
  if (Math.abs(sec) < 60) return rtf.format(sec, 'second')
  const min = Math.round(sec / 60)
  if (Math.abs(min) < 60) return rtf.format(min, 'minute')
  const hour = Math.round(min / 60)
  if (Math.abs(hour) < 24) return rtf.format(hour, 'hour')
  const day = Math.round(hour / 24)
  return rtf.format(day, 'day')
}

function playNotificationSound() {
  try {
    const context = new AudioContext()
    const oscillator = context.createOscillator()
    const gain = context.createGain()
    oscillator.type = 'sine'
    oscillator.frequency.setValueAtTime(880, context.currentTime)
    gain.gain.setValueAtTime(0.06, context.currentTime)
    gain.gain.exponentialRampToValueAtTime(0.0001, context.currentTime + 0.12)
    oscillator.connect(gain)
    gain.connect(context.destination)
    oscillator.start()
    oscillator.stop(context.currentTime + 0.12)
  } catch {
    // noop
  }
}

export function NotificationCenter({ projectId }: { projectId?: string }) {
  const navigate = useNavigate()
  const [showPreferences, setShowPreferences] = useState(false)
  const [preferences, setPreferences] = useState<NotificationPreferences>(() =>
    loadNotificationPreferences(),
  )

  const unread = useUnreadNotificationsCountQuery(projectId)
  const notifications = useNotificationsQuery(projectId)
  const markRead = useMarkNotificationRead()
  const markAll = useMarkAllNotificationsRead()

  useEffect(() => {
    const onCreated = (event: Event) => {
      const detail = (event as CustomEvent<NotificationEventDetail>).detail
      const targetProject = detail.project_id
      const targetTask = detail.task_id

      if (isProjectMuted(preferences, targetProject)) return

      if (!shouldSuppressToast(targetTask)) {
        const message = detail.body ?? detail.event_type ?? 'New notification'
        const severity = severityFor(detail.event_type)
        const action = detail.task_id
          ? {
              label: 'Open',
              onClick: () => {
                void navigate({ to: '/tasks/$taskId', params: { taskId: detail.task_id! } })
              },
            }
          : undefined

        if (severity === 'success') {
          toast.success(detail.title ?? 'Task updated', { description: message, action })
        } else if (severity === 'error') {
          toast.error(detail.title ?? 'Task failed', { description: message, action })
        } else {
          toast.warning(detail.title ?? 'Task updated', { description: message, action })
        }
      }

      if (preferences.soundEnabled) {
        playNotificationSound()
      }

      if (
        preferences.browserEnabled &&
        document.hidden &&
        typeof Notification !== 'undefined' &&
        Notification.permission === 'granted'
      ) {
        const notification = new Notification(detail.title ?? 'Forge notification', {
          body: detail.body ?? detail.event_type ?? 'Task update',
          tag: detail.notification_id ?? detail.entity_id,
        })
        notification.onclick = () => {
          window.focus()
          if (detail.task_id) {
            void navigate({ to: '/tasks/$taskId', params: { taskId: detail.task_id } })
          }
          notification.close()
        }
      }
    }

    window.addEventListener('forge:notification-created', onCreated)
    return () => window.removeEventListener('forge:notification-created', onCreated)
  }, [navigate, preferences])

  const unreadCount = unread.data?.count ?? 0
  const items = notifications.data?.items ?? []

  const mutedForCurrentProject = useMemo(
    () => isProjectMuted(preferences, projectId),
    [preferences, projectId],
  )

  const setAndPersistPreferences = (next: NotificationPreferences) => {
    setPreferences(next)
    saveNotificationPreferences(next)
  }

  const toggleBrowser = async (enabled: boolean) => {
    if (enabled && typeof Notification !== 'undefined') {
      if (Notification.permission === 'default') {
        const permission = await Notification.requestPermission()
        if (permission !== 'granted') {
          setAndPersistPreferences({ ...preferences, browserEnabled: false })
          return
        }
      }
      if (Notification.permission === 'denied') {
        setAndPersistPreferences({ ...preferences, browserEnabled: false })
        return
      }
    }
    setAndPersistPreferences({ ...preferences, browserEnabled: enabled })
  }

  const toggleMuteProject = (enabled: boolean) => {
    if (!projectId) return
    const muted = new Set(preferences.mutedProjectIds)
    if (enabled) muted.add(projectId)
    else muted.delete(projectId)
    setAndPersistPreferences({ ...preferences, mutedProjectIds: Array.from(muted) })
  }

  const openTask = async (item: NotificationResponse) => {
    if (!item.read) {
      await markRead.mutateAsync(item.id)
    }
    if (item.task_id) {
      void navigate({ to: '/tasks/$taskId', params: { taskId: item.task_id } })
    }
  }

  return (
    <DropdownMenu>
      <DropdownMenuTrigger
        className="relative flex h-8 w-8 cursor-pointer items-center justify-center rounded-lg border border-input bg-card text-muted-foreground transition-colors hover:bg-muted hover:text-foreground"
        aria-label="Notifications"
      >
        <Bell size={16} />
        {unreadCount > 0 && (
          <span className="absolute -right-1 -top-1 min-w-4 rounded-full bg-primary px-1 text-center text-micro font-semibold text-primary-foreground">
            {unreadCount > 99 ? '99+' : unreadCount}
          </span>
        )}
      </DropdownMenuTrigger>
      <DropdownMenuContent align="end" className="w-[360px] p-0">
        <div className="flex items-center justify-between border-b px-3 py-2">
          <div className="text-sm font-semibold">Notifications</div>
          <div className="flex items-center gap-2">
            <button
              type="button"
              className="text-xs text-muted-foreground hover:text-foreground"
              onClick={() => setShowPreferences((v) => !v)}
            >
              <Gear size={14} className="inline" />
            </button>
            <button
              type="button"
              className="text-xs text-muted-foreground hover:text-foreground disabled:opacity-50"
              disabled={unreadCount === 0 || markAll.isPending}
              onClick={() => {
                void markAll.mutateAsync(projectId)
              }}
            >
              Mark all read
            </button>
          </div>
        </div>

        {showPreferences && (
          <div className="space-y-3 border-b px-3 py-3 text-xs">
            <div className="flex items-center justify-between">
              <span className="flex items-center gap-1">
                <BellSimpleRinging size={14} /> Browser notifications
              </span>
              <Switch
                checked={preferences.browserEnabled}
                onChange={(event) => {
                  void toggleBrowser(event.target.checked)
                }}
              />
            </div>
            <div className="flex items-center justify-between">
              <span>Sound</span>
              <Switch
                checked={preferences.soundEnabled}
                onChange={(event) =>
                  setAndPersistPreferences({
                    ...preferences,
                    soundEnabled: event.target.checked,
                  })
                }
              />
            </div>
            <div className="flex items-center justify-between">
              <span>Mute this project</span>
              <Switch
                checked={mutedForCurrentProject}
                disabled={!projectId}
                onChange={(event) => toggleMuteProject(event.target.checked)}
              />
            </div>
          </div>
        )}

        <div className="max-h-96 overflow-y-auto p-1">
          {items.length === 0 ? (
            <div className="px-3 py-8 text-center text-sm text-muted-foreground">
              No notifications yet
            </div>
          ) : (
            items.map((item) => (
              <button
                key={item.id}
                type="button"
                className={`w-full rounded-md px-3 py-2 text-left text-sm hover:bg-accent ${
                  item.read ? 'opacity-80' : 'bg-primary/5'
                }`}
                onClick={() => {
                  void openTask(item)
                }}
              >
                <div className="flex items-start justify-between gap-2">
                  <div className="font-medium">{item.title}</div>
                  {!item.read && <Check size={13} className="text-primary" />}
                </div>
                {item.body && <div className="mt-1 text-xs text-muted-foreground">{item.body}</div>}
                <div className="mt-1 text-[11px] text-muted-foreground">{relativeTime(item.created_at)}</div>
              </button>
            ))
          )}
        </div>
        <DropdownMenuSeparator />
      </DropdownMenuContent>
    </DropdownMenu>
  )
}
