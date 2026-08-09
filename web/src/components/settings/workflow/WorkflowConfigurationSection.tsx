import type { Dispatch, SetStateAction } from 'react'
import { Button } from '@/components/ui/button'
import { Input } from '@/components/ui/input'
import { Label } from '@/components/ui/label'
import { Textarea } from '@/components/ui/textarea'
import { SettingsSection } from '@/components/settings/SettingsSection'
import { productTerm } from '@/lib/i18n'
import type { WorkflowConfigField } from '@/types/generated'

export function WorkflowConfigurationSection({
  fields,
  configDraft,
  setConfigDraft,
  isSaving,
  onSave,
}: {
  fields: WorkflowConfigField[]
  configDraft: Record<string, string>
  setConfigDraft: Dispatch<SetStateAction<Record<string, string>>>
  isSaving: boolean
  onSave: () => void
}) {
  return (
    <SettingsSection
      title="Configuration"
      description={`${productTerm('runtime')} knobs read from the workflow definition while running this project's pipeline.`}
    >
      <div className="space-y-4">
        {fields.length === 0 && (
          <p className="rounded-md border px-3 py-2 text-xs text-muted-foreground">
            This workflow does not define any configurable fields.
          </p>
        )}
        {fields.length > 0 && (
          <div className="max-w-xl divide-y rounded-md border">
            {fields.map((field) => {
              const inputId = `workflow-config-${field.id}`
              const isText = field.value_type === 'text'
              return (
                <div
                  key={field.id}
                  className={
                    isText
                      ? 'space-y-2 px-3 py-3'
                      : 'grid gap-3 px-3 py-3 sm:grid-cols-[1fr_140px] sm:items-center'
                  }
                >
                  <div className="min-w-0">
                    <Label htmlFor={inputId}>{field.label}</Label>
                    {field.description && (
                      <p className="mt-1 text-xs leading-relaxed text-muted-foreground">
                        {field.description}
                      </p>
                    )}
                  </div>
                  {isText ? (
                    <Textarea
                      id={inputId}
                      rows={4}
                      value={configDraft[field.id] ?? ''}
                      onChange={(e) =>
                        setConfigDraft((draft) => ({
                          ...draft,
                          [field.id]: e.target.value,
                        }))
                      }
                    />
                  ) : (
                    <Input
                      id={inputId}
                      type="number"
                      min={field.min ?? undefined}
                      step={1}
                      value={configDraft[field.id] ?? ''}
                      onChange={(e) =>
                        setConfigDraft((draft) => ({
                          ...draft,
                          [field.id]: e.target.value,
                        }))
                      }
                    />
                  )}
                </div>
              )
            })}
          </div>
        )}
        <div className="flex justify-end">
          <Button disabled={isSaving || fields.length === 0} onClick={onSave}>
            {isSaving ? 'Saving...' : 'Save configuration'}
          </Button>
        </div>
      </div>
    </SettingsSection>
  )
}
