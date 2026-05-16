import { useEffect, useRef, useState } from 'react'
import { CaretDown, Check, Crown, MagnifyingGlass, Plus, Spinner, Users, X } from '@phosphor-icons/react'
import { toast } from 'sonner'
import {
  useAddMember,
  useMembersQuery,
  useRemoveMember,
  useUpdateMemberRole,
  useUserSearch,
} from '@/api/hooks'
import { Avatar } from '@/components/ui/avatar'
import { Button } from '@/components/ui/button'
import {
  Dialog,
  DialogContent,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from '@/components/ui/dialog'
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuItem,
  DropdownMenuTrigger,
} from '@/components/ui/dropdown-menu'
import { Input } from '@/components/ui/input'
import { Label } from '@/components/ui/label'
import { Skeleton } from '@/components/ui/skeleton'
import { getApiErrorMessage } from '@/lib/api-error'
import type { MemberRole, ProjectMemberResponse, UserSearchResult } from '@/types/generated'

const ROLES: MemberRole[] = ['owner', 'admin', 'member', 'viewer']

const ROLE_LABELS: Record<MemberRole, string> = {
  owner: 'Owner',
  admin: 'Admin',
  member: 'Member',
  viewer: 'Viewer',
}

const ROLE_DESCRIPTIONS: Record<MemberRole, string> = {
  owner: 'Full control, can manage all members',
  admin: 'Manage members and project settings',
  member: 'Create tasks and manage agents',
  viewer: 'Read-only access',
}

function roleBadgeClass(role: MemberRole): string {
  switch (role) {
    case 'owner':
      return 'bg-primary/10 text-primary border-primary/20'
    case 'admin':
      return 'bg-blue-100 text-blue-700 border-blue-200 dark:bg-blue-900/30 dark:text-blue-400 dark:border-blue-700'
    case 'member':
      return 'bg-emerald-100 text-emerald-700 border-emerald-200 dark:bg-emerald-900/30 dark:text-emerald-400 dark:border-emerald-700'
    case 'viewer':
      return 'bg-muted text-muted-foreground border-border'
    default:
      return 'bg-muted text-muted-foreground border-border'
  }
}

function RoleBadge({ role }: { role: MemberRole }) {
  return (
    <span
      className={`inline-flex w-[68px] shrink-0 items-center justify-center rounded-sm border px-2 py-0.5 text-[11px] font-medium ${roleBadgeClass(role)}`}
    >
      {role === 'owner' && <Crown size={9} className="mr-1" weight="fill" />}
      {ROLE_LABELS[role]}
    </span>
  )
}

function RoleMenuItems({
  selected,
  onSelect,
}: {
  selected: MemberRole
  onSelect: (role: MemberRole) => void
}) {
  return (
    <>
      {ROLES.map((role) => (
        <DropdownMenuItem
          key={role}
          onClick={() => onSelect(role)}
          className="flex cursor-pointer items-center gap-2.5 py-2"
        >
          <RoleBadge role={role} />
          <span className="flex-1 truncate text-xs text-muted-foreground">
            {ROLE_DESCRIPTIONS[role]}
          </span>
          <Check
            size={14}
            weight="bold"
            className={`shrink-0 text-primary ${selected === role ? 'opacity-100' : 'opacity-0'}`}
          />
        </DropdownMenuItem>
      ))}
    </>
  )
}

function MemberRow({
  member,
  ownerCount,
  onRoleChange,
  onRemove,
  roleChangePending,
  removePending,
}: {
  member: ProjectMemberResponse
  ownerCount: number
  onRoleChange: (role: MemberRole) => void
  onRemove: () => void
  roleChangePending: boolean
  removePending: boolean
}) {
  const [confirmRemove, setConfirmRemove] = useState(false)
  const isLastOwner = member.role === 'owner' && ownerCount <= 1
  const label = member.display_name ?? member.email

  return (
    <div className="flex items-center gap-3 rounded-lg border border-border-subtle bg-card px-4 py-3">
      <Avatar name={label} seed={member.user_id} size="sm" className="shrink-0" />
      <div className="min-w-0 flex-1">
        {member.display_name && (
          <p className="truncate text-sm font-medium text-foreground">{member.display_name}</p>
        )}
        <p className="truncate text-xs text-muted-foreground">{member.email}</p>
      </div>

      <DropdownMenu>
        <DropdownMenuTrigger
          disabled={roleChangePending}
          className="flex cursor-pointer items-center gap-1 rounded-md px-2 py-1 transition-colors hover:bg-accent focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring disabled:opacity-50"
        >
          <RoleBadge role={member.role} />
          <CaretDown size={12} className="text-muted-foreground" />
        </DropdownMenuTrigger>
        <DropdownMenuContent align="end" className="w-72">
          <RoleMenuItems
            selected={member.role}
            onSelect={(role) => { if (role !== member.role) onRoleChange(role) }}
          />
        </DropdownMenuContent>
      </DropdownMenu>

      <div className="shrink-0">
        {isLastOwner ? (
          <div className="h-7 w-7" />
        ) : confirmRemove ? (
          <div className="flex items-center gap-1.5">
            <button
              type="button"
              className="cursor-pointer rounded px-2 py-1 text-xs text-muted-foreground transition-colors hover:bg-accent"
              onClick={() => setConfirmRemove(false)}
            >
              Cancel
            </button>
            <button
              type="button"
              disabled={removePending}
              className="cursor-pointer rounded bg-destructive px-2 py-1 text-xs font-medium text-destructive-foreground transition-opacity hover:opacity-90 disabled:opacity-50"
              onClick={onRemove}
            >
              Remove
            </button>
          </div>
        ) : (
          <button
            type="button"
            className="cursor-pointer rounded p-1.5 text-muted-foreground transition-colors hover:bg-destructive/10 hover:text-destructive focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring"
            aria-label="Remove member"
            onClick={() => setConfirmRemove(true)}
          >
            <X size={14} />
          </button>
        )}
      </div>
    </div>
  )
}

function AddMemberDialog({
  open,
  onOpenChange,
  projectId,
}: {
  open: boolean
  onOpenChange: (v: boolean) => void
  projectId: string
}) {
  const [query, setQuery] = useState('')
  const [debouncedQuery, setDebouncedQuery] = useState('')
  const [selected, setSelected] = useState<UserSearchResult | null>(null)
  const [role, setRole] = useState<MemberRole>('member')
  const [error, setError] = useState('')
  const [showDropdown, setShowDropdown] = useState(false)
  const inputRef = useRef<HTMLInputElement>(null)
  const addMember = useAddMember(projectId)
  const searchResult = useUserSearch(debouncedQuery)

  useEffect(() => {
    const t = setTimeout(() => setDebouncedQuery(query), 300)
    return () => clearTimeout(t)
  }, [query])

  useEffect(() => {
    if (open) {
      setQuery('')
      setDebouncedQuery('')
      setSelected(null)
      setRole('member')
      setError('')
      setShowDropdown(false)
    }
  }, [open])

  function handleClose(v: boolean) {
    onOpenChange(v)
  }

  function selectUser(user: UserSearchResult) {
    setSelected(user)
    setQuery(user.display_name ?? user.email)
    setShowDropdown(false)
    setError('')
  }

  function clearSelection() {
    setSelected(null)
    setQuery('')
    setDebouncedQuery('')
    setError('')
    setTimeout(() => inputRef.current?.focus(), 0)
  }

  async function handleSubmit(e: React.FormEvent) {
    e.preventDefault()
    if (!selected) {
      setError('Select a user from the search results')
      return
    }
    setError('')
    try {
      await addMember.mutateAsync({ user_id: selected.id, role })
      toast.success(`${selected.display_name ?? selected.email} added as ${ROLE_LABELS[role]}`)
      handleClose(false)
    } catch (err) {
      setError(getApiErrorMessage(err, 'Failed to add member'))
    }
  }

  const results = searchResult.data ?? []
  const showResults = showDropdown && debouncedQuery.trim().length >= 2

  return (
    <Dialog open={open} onOpenChange={handleClose}>
      <DialogContent>
        <form onSubmit={(e) => { void handleSubmit(e) }}>
          <DialogHeader>
            <DialogTitle>Add member</DialogTitle>
          </DialogHeader>
          <div className="mt-3 mb-2 space-y-3">
            <div className="space-y-1.5">
              <Label htmlFor="member-search">Search by email or name</Label>
              <div className="relative">
                {selected ? (
                  <div className="flex items-center gap-2 rounded-md border border-input bg-muted/40 px-3 py-2">
                    <Avatar
                      name={selected.display_name ?? selected.email}
                      seed={selected.id}
                      size="xs"
                      className="shrink-0"
                    />
                    <div className="min-w-0 flex-1">
                      {selected.display_name && (
                        <p className="truncate text-sm font-medium">{selected.display_name}</p>
                      )}
                      <p className="truncate text-xs text-muted-foreground">{selected.email}</p>
                    </div>
                    <button
                      type="button"
                      className="cursor-pointer rounded p-0.5 text-muted-foreground transition-colors hover:text-foreground"
                      onClick={clearSelection}
                      aria-label="Clear selection"
                    >
                      <X size={13} />
                    </button>
                  </div>
                ) : (
                  <>
                    <div className="pointer-events-none absolute left-2.5 top-1/2 -translate-y-1/2 text-muted-foreground">
                      {searchResult.isFetching ? (
                        <Spinner size={14} className="animate-spin" />
                      ) : (
                        <MagnifyingGlass size={14} />
                      )}
                    </div>
                    <Input
                      ref={inputRef}
                      id="member-search"
                      value={query}
                      onChange={(e) => {
                        setQuery(e.target.value)
                        setShowDropdown(true)
                        setError('')
                      }}
                      onFocus={() => setShowDropdown(true)}
                      onBlur={() => setTimeout(() => setShowDropdown(false), 150)}
                      placeholder="Search by email or name…"
                      autoFocus
                      className="pl-8"
                    />
                    {showResults && (
                      <div className="absolute left-0 right-0 top-full z-50 mt-1 overflow-hidden rounded-md border border-border bg-popover shadow-md">
                        {searchResult.isFetching && results.length === 0 ? (
                          <div className="px-3 py-2.5 text-sm text-muted-foreground">
                            Searching…
                          </div>
                        ) : results.length === 0 ? (
                          <div className="px-3 py-2.5 text-sm text-muted-foreground">
                            No users found
                          </div>
                        ) : (
                          results.map((user) => (
                            <button
                              key={user.id}
                              type="button"
                              className="flex w-full cursor-pointer items-center gap-2.5 px-3 py-2 text-left transition-colors hover:bg-accent"
                              onMouseDown={() => selectUser(user)}
                            >
                              <Avatar
                                name={user.display_name ?? user.email}
                                seed={user.id}
                                size="xs"
                                className="shrink-0"
                              />
                              <div className="min-w-0">
                                {user.display_name && (
                                  <p className="truncate text-sm font-medium">{user.display_name}</p>
                                )}
                                <p className="truncate text-xs text-muted-foreground">{user.email}</p>
                              </div>
                            </button>
                          ))
                        )}
                      </div>
                    )}
                  </>
                )}
              </div>
            </div>
            <div className="flex flex-col gap-1.5">
              <Label>Role</Label>
              <DropdownMenu>
                <DropdownMenuTrigger className="flex w-full cursor-pointer items-center justify-between rounded-md border border-input bg-transparent px-3 py-2 text-sm shadow-sm transition-colors hover:bg-accent focus-visible:outline-none focus-visible:ring-1 focus-visible:ring-ring">
                  <span className="flex items-center gap-2.5">
                    <RoleBadge role={role} />
                    <span className="truncate text-xs text-muted-foreground">{ROLE_DESCRIPTIONS[role]}</span>
                  </span>
                  <CaretDown size={12} className="shrink-0 text-muted-foreground" />
                </DropdownMenuTrigger>
                <DropdownMenuContent className="w-[var(--radix-dropdown-menu-trigger-width)]">
                  <RoleMenuItems selected={role} onSelect={setRole} />
                </DropdownMenuContent>
              </DropdownMenu>
            </div>
            {error && <p className="text-sm text-destructive">{error}</p>}
          </div>
          <DialogFooter>
            <button
              type="button"
              className="cursor-pointer rounded-md border px-3 py-1.5 text-sm transition-colors hover:bg-accent"
              onClick={() => handleClose(false)}
            >
              Cancel
            </button>
            <Button type="submit" disabled={addMember.isPending || !selected}>
              {addMember.isPending ? 'Adding…' : 'Add member'}
            </Button>
          </DialogFooter>
        </form>
      </DialogContent>
    </Dialog>
  )
}

export function MembersTab({ projectId }: { projectId: string }) {
  const membersQuery = useMembersQuery(projectId)
  const updateRole = useUpdateMemberRole(projectId)
  const removeMember = useRemoveMember(projectId)
  const [addOpen, setAddOpen] = useState(false)

  const members = membersQuery.data ?? []
  const ownerCount = members.filter((m) => m.role === 'owner').length

  function handleRoleChange(member: ProjectMemberResponse, role: MemberRole) {
    updateRole.mutate(
      { userId: member.user_id, body: { role } },
      {
        onSuccess: () => toast.success(`Role updated to ${ROLE_LABELS[role]}`),
        onError: (err) => toast.error(getApiErrorMessage(err, 'Failed to update role')),
      },
    )
  }

  function handleRemove(member: ProjectMemberResponse) {
    removeMember.mutate(member.user_id, {
      onSuccess: () => toast.success('Member removed'),
      onError: (err) => toast.error(getApiErrorMessage(err, 'Failed to remove member')),
    })
  }

  return (
    <>
      <div className="mb-8 flex items-start justify-between gap-4">
        <div>
          <h2 className="text-page font-semibold tracking-tight">Members</h2>
          <p className="mt-1 text-sm text-muted-foreground">
            Manage who has access to this project.
          </p>
        </div>
        <Button onClick={() => setAddOpen(true)}>
          <Plus size={14} className="mr-1.5" />
          Add member
        </Button>
      </div>

      {membersQuery.isLoading ? (
        <div className="space-y-2">
          <Skeleton className="h-14 w-full" />
          <Skeleton className="h-14 w-full" />
          <Skeleton className="h-14 w-full" />
        </div>
      ) : members.length === 0 ? (
        <div className="flex flex-col items-center justify-center rounded-xl border border-dashed border-border py-12 text-center">
          <Users size={28} className="mb-3 text-muted-foreground/50" />
          <p className="text-sm font-medium text-foreground">No members yet</p>
          <p className="mt-1 text-sm text-muted-foreground">Add members to collaborate on this project.</p>
          <Button className="mt-4" onClick={() => setAddOpen(true)}>
            <Plus size={14} className="mr-1.5" />
            Add member
          </Button>
        </div>
      ) : (
        <div className="space-y-2">
          {members.map((member) => (
            <MemberRow
              key={member.id}
              member={member}
              ownerCount={ownerCount}
              onRoleChange={(role) => handleRoleChange(member, role)}
              onRemove={() => handleRemove(member)}
              roleChangePending={updateRole.isPending}
              removePending={removeMember.isPending}
            />
          ))}
        </div>
      )}

      <AddMemberDialog open={addOpen} onOpenChange={setAddOpen} projectId={projectId} />
    </>
  )
}
