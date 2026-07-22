import { UserCircle } from '@phosphor-icons/react'
import { Avatar } from '@/components/ui/avatar'
import { Checkbox } from '@/components/ui/checkbox'
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuItem,
  DropdownMenuSeparator,
  DropdownMenuTrigger,
} from '@/components/ui/dropdown-menu'
import { cn } from '@/lib/cn'
import type { Agent } from '@/types/generated'

const MAX_VISIBLE = 5

export function AgentFilterGroup({
  agents,
  selectedAgentIds,
  onSelect,
  showUserOption = true,
}: {
  agents: Agent[]
  selectedAgentIds: string[]
  onSelect: (agentIds: string[]) => void
  showUserOption?: boolean
}) {
  if (agents.length === 0 && !showUserOption) return null

  const selectedAgentIdSet = new Set(selectedAgentIds)
  const visibleAgents = agents.slice(0, MAX_VISIBLE)
  const hiddenAgents = agents.slice(MAX_VISIBLE)
  const hasHiddenSelected = hiddenAgents.some((agent) => selectedAgentIdSet.has(agent.id))
  const userSelected = selectedAgentIdSet.has('user')

  const toggle = (id: string) => {
    const next = selectedAgentIdSet.has(id)
      ? selectedAgentIds.filter((a) => a !== id)
      : [...selectedAgentIds, id]
    onSelect(next)
  }

  return (
    <div className="flex items-center gap-1">
      {visibleAgents.map((agent) => {
        const isSelected = selectedAgentIdSet.has(agent.id)
        return (
          <button
            key={agent.id}
            type="button"
            title={`${agent.name}${isSelected ? ' · Click to deselect' : ' · Click to filter'}`}
            className={cn(
              'relative cursor-pointer rounded-md transition-all duration-150 focus:outline-none focus-visible:ring-2 focus-visible:ring-ring focus-visible:ring-offset-1 focus-visible:ring-offset-background',
              'ring-[2px] ring-background',
              isSelected
                ? 'z-20 ring-2 ring-primary ring-offset-1 ring-offset-background'
                : 'z-10 hover:z-20',
            )}
            onClick={() => toggle(agent.id)}
          >
            <Avatar name={agent.name} seed={agent.id} size="sm" className="h-6 w-6 text-[10px]" />
          </button>
        )
      })}

      {showUserOption && (
        <button
          type="button"
          title={userSelected ? 'Human · Click to deselect' : 'Human · Click to filter'}
          className={cn(
            'relative z-10 flex h-6 w-6 cursor-pointer items-center justify-center rounded-md transition-all duration-150 focus:outline-none focus-visible:ring-2 focus-visible:ring-ring focus-visible:ring-offset-1 focus-visible:ring-offset-background',
            userSelected
              ? 'z-20 bg-primary text-primary-foreground ring-2 ring-primary ring-offset-1 ring-offset-background'
              : 'bg-muted text-muted-foreground hover:z-20 hover:bg-accent hover:text-foreground',
          )}
          onClick={() => toggle('user')}
        >
          <UserCircle size={14} weight={userSelected ? 'fill' : 'regular'} />
        </button>
      )}

      {hiddenAgents.length > 0 && (
        <DropdownMenu>
          <DropdownMenuTrigger
            className={cn(
              'relative z-10 flex h-6 w-6 cursor-pointer items-center justify-center rounded-md text-[9px] font-bold transition-all duration-150',
              'ring-[2px] ring-background focus:outline-none focus-visible:ring-2 focus-visible:ring-ring',
              hasHiddenSelected
                ? 'z-20 bg-primary/15 text-primary ring-2 ring-primary ring-offset-1 ring-offset-background'
                : 'bg-muted text-muted-foreground hover:z-20 hover:bg-accent hover:text-foreground',
            )}
          >
            ···
          </DropdownMenuTrigger>
          <DropdownMenuContent className="w-52">
            {agents.map((agent) => {
              const isSelected = selectedAgentIdSet.has(agent.id)
              return (
                <DropdownMenuItem
                  key={agent.id}
                  className="gap-2 p-2"
                  keepOpen
                  onClick={() => toggle(agent.id)}
                >
                  <Checkbox
                    checked={isSelected}
                    onChange={() => {}}
                    className="pointer-events-none"
                  />
                  <Avatar name={agent.name} seed={agent.id} size="sm" />
                  <span className="min-w-0 flex-1 truncate text-sm">{agent.name}</span>
                </DropdownMenuItem>
              )
            })}
            {showUserOption && (
              <>
                <DropdownMenuSeparator />
                <DropdownMenuItem className="gap-2 p-2" keepOpen onClick={() => toggle('user')}>
                  <Checkbox
                    checked={userSelected}
                    onChange={() => {}}
                    className="pointer-events-none"
                  />
                  <div className="flex h-6 w-6 items-center justify-center rounded-md bg-muted text-muted-foreground">
                    <UserCircle size={14} />
                  </div>
                  <span className="min-w-0 flex-1 truncate text-sm">Human</span>
                </DropdownMenuItem>
              </>
            )}
          </DropdownMenuContent>
        </DropdownMenu>
      )}
    </div>
  )
}
