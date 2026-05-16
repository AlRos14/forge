import { Plus, Robot } from '@phosphor-icons/react'
import { Button } from '@/components/ui/button'

export function EmptyPanel({ onCreate }: { onCreate: () => void }) {
  return (
    <div className="flex flex-1 items-center justify-center">
      <div className="text-center">
        <div className="mx-auto mb-4 flex h-12 w-12 items-center justify-center rounded-full bg-muted">
          <Robot size={24} className="text-muted-foreground" />
        </div>
        <p className="text-sm font-medium">Select an agent</p>
        <p className="mt-1 text-xs text-muted-foreground">Choose an agent from the list or create a new one</p>
        <Button className="mt-4" size="sm" variant="outline" onClick={onCreate}>
          <Plus size={14} className="mr-1.5" />
          New Agent
        </Button>
      </div>
    </div>
  )
}
