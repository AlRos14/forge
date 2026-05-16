import { Button } from '@/components/ui/button'
import { Input } from '@/components/ui/input'
import { Label } from '@/components/ui/label'

export function normalizeCiSteps(steps: string[]): string[] {
  return steps.map((step) => step.trim()).filter((step) => step.length > 0)
}

interface CiStepsEditorProps {
  steps: string[]
  onChange: (steps: string[]) => void
}

export function CiStepsEditor({ steps, onChange }: CiStepsEditorProps) {
  return (
    <div className="space-y-2 rounded-md border p-3">
      <Label>CI steps (optional)</Label>
      <div className="space-y-2">
        {steps.map((step, index) => (
          <div key={`ci-step-${index}`} className="flex items-center gap-2">
            <Input
              className="h-8"
              placeholder={`Step ${index + 1}`}
              value={step}
              onChange={(event) => {
                const next = [...steps]
                next[index] = event.target.value
                onChange(next)
              }}
            />
            <Button
              aria-label={`Remove CI step ${index + 1}`}
              className="h-8 px-2"
              size="sm"
              type="button"
              variant="ghost"
              onClick={() => onChange(steps.filter((_, candidateIndex) => candidateIndex !== index))}
            >
              x
            </Button>
          </div>
        ))}
      </div>
      <Button
        className="h-8"
        size="sm"
        type="button"
        variant="outline"
        onClick={() => onChange([...steps, ''])}
      >
        + Add step
      </Button>
    </div>
  )
}
