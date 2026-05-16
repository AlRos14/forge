import { useMemo } from 'react'
import { Sparkle } from '@phosphor-icons/react'
import { ComboSelect, type ComboSelectOption } from '@/components/ui/combo-select'
import { Label } from '@/components/ui/label'
import { Skeleton } from '@/components/ui/skeleton'
import { cn } from '@/lib/cn'
import type { DiscoveredModelOption } from '@/hooks/useDiscoveredOptions'

function modelLabel(model: DiscoveredModelOption): string {
  return model.provider ? `${model.displayName} (${model.provider})` : model.displayName
}

export function ModelSelector({
  id,
  models,
  recentModelIds,
  value,
  disabled,
  isLoading,
  hasError,
  className,
  onChange,
}: {
  id: string
  models: DiscoveredModelOption[]
  recentModelIds: string[]
  value: string | null
  disabled?: boolean
  isLoading?: boolean
  hasError?: boolean
  className?: string
  onChange: (modelId: string | null) => void
}) {
  const options = useMemo<ComboSelectOption[]>(() => {
    const modelById = new Map(models.map((m) => [m.id, m]))
    const recentModels = recentModelIds
      .map((mid) => modelById.get(mid))
      .filter((m): m is DiscoveredModelOption => Boolean(m))
    const otherModels = models.filter((m) => !recentModels.some((r) => r.id === m.id))

    return [
      ...recentModels.map<ComboSelectOption>((m) => ({
        value: m.id,
        label: modelLabel(m),
        group: 'Recent',
      })),
      ...otherModels.map<ComboSelectOption>((m) => ({
        value: m.id,
        label: modelLabel(m),
        group: recentModels.length > 0 ? 'All models' : '',
      })),
    ]
  }, [models, recentModelIds])

  const placeholder = hasError ? 'Could not load options' : 'Default (agent setting)'

  return (
    <div className={cn('min-w-0 space-y-1', className)}>
      <Label htmlFor={id} className="flex items-center gap-1.5">
        <Sparkle size={12} />
        Model
      </Label>
      {isLoading ? (
        <Skeleton className="h-9 w-full" />
      ) : (
        <ComboSelect
          id={id}
          value={value}
          options={options}
          placeholder={placeholder}
          allowCustom={!hasError}
          disabled={disabled}
          onChange={onChange}
        />
      )}
    </div>
  )
}
