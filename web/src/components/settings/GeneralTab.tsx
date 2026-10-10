import { SettingsSection } from '@/components/settings/SettingsSection'
import { Button } from '@/components/ui/button'
import { Input } from '@/components/ui/input'
import { Skeleton } from '@/components/ui/skeleton'

interface GeneralTabProps {
  projectIsLoading: boolean
  canSave: boolean
  isSaving: boolean
  paused: boolean
  pausedAt?: string | null
  pausePending: boolean
  name: string
  onNameChange: (value: string) => void
  onTogglePaused: () => void
  onSave: () => void
}

export function GeneralTab({
  projectIsLoading,
  canSave,
  isSaving,
  paused,
  pausedAt,
  pausePending,
  name,
  onNameChange,
  onTogglePaused,
  onSave,
}: GeneralTabProps) {
  return (
    <>
      <div className="mb-8">
        <h2 className="text-page font-semibold tracking-tight">General</h2>
        <p className="mt-1 text-sm text-muted-foreground">Project identity and availability.</p>
      </div>
      {projectIsLoading ? (
        <div className="space-y-4">
          <Skeleton className="h-10 w-full" />
          <Skeleton className="h-16 w-full" />
        </div>
      ) : (
        <>
          <SettingsSection title="Project name" description="Shown in the project switcher.">
            <Input
              id="project-name"
              className="max-w-xs"
              value={name}
              onChange={(event) => onNameChange(event.target.value)}
            />
          </SettingsSection>
          <SettingsSection
            title="Project availability"
            description="Pause dispatch for this project without changing its tasks."
          >
            <div className="flex items-center gap-3">
              <Button
                size="sm"
                variant="outline"
                disabled={pausePending || !canSave}
                onClick={onTogglePaused}
              >
                {pausePending ? 'Saving…' : paused ? 'Resume project' : 'Pause project'}
              </Button>
              {paused && pausedAt ? (
                <span className="text-xs text-muted-foreground">
                  Paused {new Date(pausedAt).toLocaleString()}
                </span>
              ) : null}
            </div>
          </SettingsSection>
          <div className="flex justify-end py-6">
            <Button disabled={isSaving || !canSave} onClick={onSave}>
              {isSaving ? 'Saving…' : 'Save'}
            </Button>
          </div>
        </>
      )}
    </>
  )
}
