import { Input } from '@/components/ui/input'
import { Label } from '@/components/ui/label'
import { cliCommandPlaceholder } from './harness-command'

export function HarnessLaunchFields({
  idPrefix,
  executorType,
  cliCommand,
  onCliCommandChange,
  codexHome,
  onCodexHomeChange,
}: {
  idPrefix: string
  executorType: string
  cliCommand: string
  onCliCommandChange: (value: string) => void
  codexHome?: string
  onCodexHomeChange?: (value: string) => void
}) {
  return (
    <div className="space-y-4 sm:col-span-2">
      {executorType === 'codex' && onCodexHomeChange ? (
        <div className="space-y-2">
          <Label htmlFor={`${idPrefix}-codex-home`}>Codex home</Label>
          <Input
            id={`${idPrefix}-codex-home`}
            value={codexHome ?? ''}
            onChange={(event) => onCodexHomeChange(event.target.value)}
            placeholder="~/.codex2"
            spellCheck={false}
            autoComplete="off"
          />
          <p className="text-xs text-muted-foreground">
            Auth directory for this agent (<code>CODEX_HOME</code>). Leave empty for the default{' '}
            <code>~/.codex</code>. A second Codex login lives in a different directory — this is
            what Refresh quota uses.
          </p>
        </div>
      ) : null}
      <div className="space-y-2">
        <Label htmlFor={`${idPrefix}-cli-command`}>CLI command</Label>
        <Input
          id={`${idPrefix}-cli-command`}
          value={cliCommand}
          onChange={(event) => onCliCommandChange(event.target.value)}
          placeholder={cliCommandPlaceholder(executorType)}
          spellCheck={false}
          autoComplete="off"
        />
        <p className="text-xs text-muted-foreground">
          Optional. A PATH binary, an absolute path, or a shell alias from
          interactive bash (for example <code>codex2</code> →{' '}
          <code>CODEX_HOME=~/.codex-plus2 codex</code>). Forge resolves the alias
          and runs the real CLI with that environment.
        </p>
      </div>
    </div>
  )
}
