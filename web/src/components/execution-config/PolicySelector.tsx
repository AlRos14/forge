import { FastForward, HandPalm, ListChecks } from '@phosphor-icons/react'
import { Label } from '@/components/ui/label'
import { Select } from '@/components/ui/select'
import { cn } from '@/lib/cn'

const policyOptions = [
  {
    id: 'auto',
    label: 'Auto',
    description: 'Run without pausing for routine decisions.',
    Icon: FastForward,
  },
  {
    id: 'supervised',
    label: 'Supervised',
    description: 'Ask before risky operations.',
    Icon: HandPalm,
  },
  {
    id: 'plan',
    label: 'Plan',
    description: 'Plan first, then wait for approval.',
    Icon: ListChecks,
  },
]

export function PolicySelector({
  id,
  value,
  disabled,
  className,
  onChange,
}: {
  id: string
  value: string | null
  disabled?: boolean
  className?: string
  onChange: (policy: string | null) => void
}) {
  const selectedPolicy = policyOptions.find((policy) => policy.id === value)

  return (
    <div className={cn('min-w-0 space-y-1', className)}>
      <Label htmlFor={id} className="flex items-center gap-1.5">
        {selectedPolicy ? <selectedPolicy.Icon size={12} /> : <FastForward size={12} />}
        Policy
      </Label>
      <Select
        id={id}
        value={value ?? ''}
        disabled={disabled}
        className="h-9 text-xs"
        title={selectedPolicy?.description ?? 'Use profile default'}
        placeholder="Default"
        options={policyOptions.map((policy) => ({
          value: policy.id,
          label: `${policy.label} — ${policy.description}`,
        }))}
        onChange={(v) => onChange(v || null)}
      />
    </div>
  )
}
