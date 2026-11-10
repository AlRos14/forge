import { useState } from 'react'
import { Check, CopySimple } from '@phosphor-icons/react'
import { toast } from 'sonner'
import { Button } from '@/components/ui/button'

export function CopyReferenceButton({ value, label }: { value: string; label: string }) {
  const [copied, setCopied] = useState(false)

  const copy = async () => {
    try {
      await navigator.clipboard.writeText(value)
      setCopied(true)
      window.setTimeout(() => setCopied(false), 1600)
    } catch {
      toast.error(`Could not copy ${label.toLowerCase()}`)
    }
  }

  return (
    <Button
      type="button"
      size="icon-sm"
      variant="ghost"
      aria-label={`Copy ${label}`}
      title={`Copy ${label}`}
      onClick={() => void copy()}
    >
      {copied ? (
        <Check size={14} aria-hidden="true" />
      ) : (
        <CopySimple size={14} aria-hidden="true" />
      )}
    </Button>
  )
}
