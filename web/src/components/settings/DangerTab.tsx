import { Trash } from '@phosphor-icons/react'
import { Button } from '@/components/ui/button'
import { SettingsSection } from '@/components/settings/SettingsSection'

export function DangerTab({ onDeleteClick }: { onDeleteClick: () => void }) {
  return (
    <>
      <div className="mb-8">
        <h2 className="text-page font-semibold tracking-tight text-red-300">Danger zone</h2>
        <p className="mt-1 text-sm text-muted-foreground">Irreversible actions on this project.</p>
      </div>
      <SettingsSection
        title="Delete project"
        description="This will permanently delete the project and all associated data."
        danger
      >
        <Button variant="destructive" onClick={onDeleteClick}>
          <Trash size={14} className="mr-1.5" />
          Delete Project
        </Button>
      </SettingsSection>
    </>
  )
}
