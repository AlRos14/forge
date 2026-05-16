import type { Icon } from '@phosphor-icons/react'
import { Key, User } from '@phosphor-icons/react'
import { Link } from '@tanstack/react-router'
import { AccessTokensTab } from '@/components/settings/AccessTokensTab'
import { ProfileTab } from '@/components/settings/ProfileTab'
import { cn } from '@/lib/cn'
import { useAuthStore } from '@/stores/auth'

export type AccountTab = 'profile' | 'tokens'

const TABS: Array<{ id: AccountTab; label: string; icon: Icon }> = [
  { id: 'profile', label: 'Profile', icon: User },
  { id: 'tokens', label: 'Access Tokens', icon: Key },
]

export function isAccountTab(value: string | undefined): value is AccountTab {
  return TABS.some((tab) => tab.id === value)
}

export function AccountPage({ initialTab = 'profile' }: { initialTab?: AccountTab }) {
  const user = useAuthStore((s) => s.user)
  const label = user?.display_name ?? user?.email ?? 'Account'

  return (
    <div className="flex h-[calc(100vh-7rem)] gap-0 overflow-hidden rounded-xl border border-border-subtle bg-card shadow-card">
      {/* Sidebar */}
      <aside className="flex w-56 shrink-0 flex-col border-r bg-background">
        <div className="border-b px-4 py-3">
          <p className="font-mono text-micro font-semibold uppercase tracking-[1px] text-muted-foreground">
            Account
          </p>
          <div className="mt-1.5 flex items-center gap-2">
            <div className="flex h-6 w-6 shrink-0 items-center justify-center rounded-full bg-primary/10 text-xs font-semibold text-primary">
              <User size={13} />
            </div>
            <p className="truncate text-sm font-medium text-foreground" title={label}>
              {label}
            </p>
          </div>
        </div>
        <nav className="flex flex-1 flex-col gap-0.5 p-2">
          {TABS.map((t) => {
            const TabIcon = t.icon
            return (
              <Link
                key={t.id}
                to={t.id === 'profile' ? '/account' : '/account/$tab'}
                params={{ tab: t.id }}
                className={cn(
                  'relative flex w-full cursor-pointer items-center gap-2.5 rounded-lg px-2.5 py-[7px] text-left text-[13px] leading-none font-medium transition-colors',
                  initialTab === t.id
                    ? 'bg-[var(--ember-surface)] text-sidebar-active-foreground before:absolute before:left-0 before:top-1/2 before:-translate-y-1/2 before:h-4 before:w-[3px] before:rounded-r-full before:bg-primary'
                    : 'text-sidebar-foreground hover:bg-accent/50 hover:text-foreground',
                )}
              >
                <TabIcon size={16} />
                {t.label}
              </Link>
            )
          })}
        </nav>
      </aside>

      {/* Content */}
      <div className="flex-1 overflow-y-auto px-8 py-6">
        <div className="max-w-[760px]">
          {initialTab === 'profile' && <ProfileTab />}
          {initialTab === 'tokens' && <AccessTokensTab />}
        </div>
      </div>
    </div>
  )
}
