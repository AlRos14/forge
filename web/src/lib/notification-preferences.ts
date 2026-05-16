const KEY = 'forge.notification.preferences.v1'

export type NotificationPreferences = {
  browserEnabled: boolean
  soundEnabled: boolean
  mutedProjectIds: string[]
}

export const DEFAULT_NOTIFICATION_PREFERENCES: NotificationPreferences = {
  browserEnabled: false,
  soundEnabled: false,
  mutedProjectIds: [],
}

export function loadNotificationPreferences(): NotificationPreferences {
  try {
    const raw = localStorage.getItem(KEY)
    if (!raw) return DEFAULT_NOTIFICATION_PREFERENCES
    const parsed = JSON.parse(raw) as Partial<NotificationPreferences>
    return {
      browserEnabled: Boolean(parsed.browserEnabled),
      soundEnabled: Boolean(parsed.soundEnabled),
      mutedProjectIds: Array.isArray(parsed.mutedProjectIds)
        ? parsed.mutedProjectIds.filter((id): id is string => typeof id === 'string')
        : [],
    }
  } catch {
    return DEFAULT_NOTIFICATION_PREFERENCES
  }
}

export function saveNotificationPreferences(preferences: NotificationPreferences): void {
  localStorage.setItem(KEY, JSON.stringify(preferences))
}

export function isProjectMuted(preferences: NotificationPreferences, projectId?: string): boolean {
  if (!projectId) return false
  return preferences.mutedProjectIds.includes(projectId)
}
