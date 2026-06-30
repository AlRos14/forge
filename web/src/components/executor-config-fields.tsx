import { CollapsibleSection } from '@/components/ui/collapsible-section'
import { Checkbox } from '@/components/ui/checkbox'
import { Input } from '@/components/ui/input'
import { Select } from '@/components/ui/select'
import { Textarea } from '@/components/ui/textarea'
import { cn } from '@/lib/cn'

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

export function safeParseConfig(raw: string): Record<string, unknown> {
  try {
    const parsed = JSON.parse(raw)
    if (parsed && typeof parsed === 'object' && !Array.isArray(parsed)) return parsed as Record<string, unknown>
  } catch { /* ignore */ }
  return {}
}

export function setConfigField(json: string, key: string, value: unknown): string {
  const cfg = safeParseConfig(json)
  if (value === '' || value === undefined || value === null || value === false) delete cfg[key]
  else cfg[key] = value
  return JSON.stringify(cfg, null, 2)
}

export function getStr(cfg: Record<string, unknown>, key: string): string {
  const v = cfg[key]; return typeof v === 'string' ? v : ''
}
export function getBool(cfg: Record<string, unknown>, key: string): boolean { return cfg[key] === true }
export function getNum(cfg: Record<string, unknown>, key: string): string {
  const v = cfg[key]; return v !== undefined && v !== null ? String(v) : ''
}
export function getStrArray(cfg: Record<string, unknown>, key: string): string {
  const v = cfg[key]; return Array.isArray(v) ? (v as string[]).join(', ') : ''
}
export function getEnvString(cfg: Record<string, unknown>): string {
  const v = cfg['env']
  if (!v || typeof v !== 'object' || Array.isArray(v)) return ''
  return Object.entries(v as Record<string, string>).map(([k, val]) => `${k}=${val}`).join('\n')
}
export function parseEnvString(raw: string): Record<string, string> | undefined {
  if (!raw.trim()) return undefined
  const result: Record<string, string> = {}
  for (const line of raw.split('\n')) {
    const eq = line.indexOf('=')
    if (eq > 0) result[line.slice(0, eq).trim()] = line.slice(eq + 1)
  }
  return Object.keys(result).length > 0 ? result : undefined
}

// ---------------------------------------------------------------------------
// Field wrapper
// ---------------------------------------------------------------------------

export function ConfigField({
  label,
  hint,
  className,
  children,
}: {
  label: string
  hint?: string
  className?: string
  children: React.ReactNode
}) {
  return (
    <label className={cn('block space-y-1', className)}>
      <span className="text-xs font-medium text-foreground/80">{label}</span>
      {children}
      {hint && <p className="mt-0.5 text-xs text-muted-foreground">{hint}</p>}
    </label>
  )
}

// ---------------------------------------------------------------------------
// Executor-specific field groups
// NOTE: model, reasoning_effort, and permission_policy are handled by the
// top-level agent form selectors — these components only render fields that
// are unique to each executor type's extended config.
// ---------------------------------------------------------------------------

export type ExecFieldProps = {
  cfg: Record<string, unknown>
  onChange: (key: string, value: unknown) => void
}

export function ClaudeCodeFields({ cfg, onChange }: ExecFieldProps) {
  return (
    <div className="grid grid-cols-2 gap-3">
      <ConfigField label="Approvals">
        <Select
          value={getStr(cfg, 'approvals') || ''}
          placeholder="Default"
          options={[
            { value: 'auto', label: 'Auto' },
            { value: 'full', label: 'Full' },
          ]}
          onChange={(v) => onChange('approvals', v || undefined)}
        />
      </ConfigField>
      <ConfigField label="Agent">
        <Input value={getStr(cfg, 'agent')} onChange={(e) => onChange('agent', e.target.value || undefined)} placeholder="default" />
      </ConfigField>
      <div className="col-span-2 flex flex-wrap gap-x-5 gap-y-2 pt-1">
        <label className="flex cursor-pointer items-center gap-2 text-sm select-none">
          <Checkbox className="h-3.5 w-3.5" checked={getBool(cfg, 'plan')} onChange={(e) => onChange('plan', e.target.checked || undefined)} />
          Plan mode
        </label>
        <label className="flex cursor-pointer items-center gap-2 text-sm select-none">
          <Checkbox className="h-3.5 w-3.5" checked={getBool(cfg, 'claude_code_router')} onChange={(e) => onChange('claude_code_router', e.target.checked || undefined)} />
          Router
        </label>
        <label className="flex cursor-pointer items-center gap-2 text-sm select-none">
          <Checkbox className="h-3.5 w-3.5" checked={getBool(cfg, 'disable_api_key')} onChange={(e) => onChange('disable_api_key', e.target.checked || undefined)} />
          Disable API key
        </label>
        <label className="flex cursor-pointer items-center gap-2 text-sm text-destructive select-none">
          <Checkbox className="h-3.5 w-3.5" checked={getBool(cfg, 'dangerously_skip_permissions')} onChange={(e) => onChange('dangerously_skip_permissions', e.target.checked || undefined)} />
          Skip permissions
        </label>
      </div>
    </div>
  )
}

export function CodexFields({ cfg, onChange }: ExecFieldProps) {
  return (
    <div className="grid grid-cols-2 gap-3">
      <ConfigField label="Sandbox">
        <Input value={getStr(cfg, 'sandbox')} onChange={(e) => onChange('sandbox', e.target.value || undefined)} placeholder="danger-full-access" />
      </ConfigField>
      <ConfigField label="Ask for Approval">
        <Input value={getStr(cfg, 'ask_for_approval')} onChange={(e) => onChange('ask_for_approval', e.target.value || undefined)} placeholder="auto" />
      </ConfigField>
      <ConfigField label="Base Instructions" className="col-span-2">
        <Textarea className="resize-none" rows={2} value={getStr(cfg, 'base_instructions')} onChange={(e) => onChange('base_instructions', e.target.value || undefined)} placeholder="System-level instructions for the agent..." />
      </ConfigField>
      <ConfigField label="Developer Instructions" className="col-span-2">
        <Textarea className="resize-none" rows={2} value={getStr(cfg, 'developer_instructions')} onChange={(e) => onChange('developer_instructions', e.target.value || undefined)} placeholder="Task-specific developer instructions..." />
      </ConfigField>
      <div className="col-span-2 pt-1">
        <label className="flex cursor-pointer items-center gap-2 text-sm select-none">
          <Checkbox className="h-3.5 w-3.5" checked={getBool(cfg, 'include_apply_patch_tool')} onChange={(e) => onChange('include_apply_patch_tool', e.target.checked || undefined)} />
          Include apply-patch tool
        </label>
      </div>
    </div>
  )
}

export function CursorFields({ cfg, onChange }: ExecFieldProps) {
  return (
    <div className="grid grid-cols-2 gap-3">
      <ConfigField label="Resume Session" className="col-span-2">
        <Input value={getStr(cfg, 'resume_session_id')} onChange={(e) => onChange('resume_session_id', e.target.value || undefined)} placeholder="session id" />
      </ConfigField>
      <div className="col-span-2 pt-1">
        <label className="flex cursor-pointer items-center gap-2 text-sm select-none">
          <Checkbox className="h-3.5 w-3.5" checked={getBool(cfg, 'force')} onChange={(e) => onChange('force', e.target.checked || undefined)} />
          Force file changes
        </label>
      </div>
    </div>
  )
}

export function OpencodeFields({ cfg, onChange }: ExecFieldProps) {
  return (
    <div className="grid grid-cols-2 gap-3">
      <ConfigField label="Agent">
        <Input value={getStr(cfg, 'agent')} onChange={(e) => onChange('agent', e.target.value || undefined)} placeholder="default" />
      </ConfigField>
      <ConfigField label="Variant">
        <Input value={getStr(cfg, 'variant')} onChange={(e) => onChange('variant', e.target.value || undefined)} placeholder="default" />
      </ConfigField>
      <div className="col-span-2 flex gap-5 pt-1">
        <label className="flex cursor-pointer items-center gap-2 text-sm select-none">
          <Checkbox className="h-3.5 w-3.5" checked={getBool(cfg, 'auto_approve')} onChange={(e) => onChange('auto_approve', e.target.checked || undefined)} />
          Auto approve
        </label>
        <label className="flex cursor-pointer items-center gap-2 text-sm select-none">
          <Checkbox className="h-3.5 w-3.5" checked={getBool(cfg, 'auto_compact')} onChange={(e) => onChange('auto_compact', e.target.checked || undefined)} />
          Auto compact
        </label>
      </div>
    </div>
  )
}

export function ShellFields({ cfg, onChange }: ExecFieldProps) {
  return (
    <div className="grid grid-cols-2 gap-3">
      <ConfigField label="Command" className="col-span-2">
        <Input value={getStr(cfg, 'command')} onChange={(e) => onChange('command', e.target.value || undefined)} placeholder="/usr/bin/bash" />
      </ConfigField>
      <ConfigField label="Args" hint="Comma-separated">
        <Input
          value={getStrArray(cfg, 'args')}
          onChange={(e) => onChange('args', e.target.value ? e.target.value.split(',').map((s) => s.trim()).filter(Boolean) : undefined)}
          placeholder="-c, -x"
        />
      </ConfigField>
      <ConfigField label="Timeout (seconds)">
        <Input
          type="number"
          value={getNum(cfg, 'timeout_seconds')}
          onChange={(e) => onChange('timeout_seconds', e.target.value ? Number(e.target.value) : undefined)}
          placeholder="300"
          min={1}
        />
      </ConfigField>
    </div>
  )
}

// ---------------------------------------------------------------------------
// Command overrides (shared across all executor types)
// ---------------------------------------------------------------------------

export function CommandOverridesFields({ cfg, onChange }: ExecFieldProps) {
  const hasValues = Boolean(cfg['base_command_override'] || cfg['additional_params'] || cfg['env'])
  return (
    <CollapsibleSection
      title="Command Overrides"
      defaultOpen={hasValues}
      badge={hasValues ? 'set' : undefined}
      contentClassName="grid grid-cols-2 gap-3"
    >
      <ConfigField label="Base Command Override" className="col-span-2">
        <Input
          value={getStr(cfg, 'base_command_override')}
          onChange={(e) => onChange('base_command_override', e.target.value || undefined)}
          placeholder="/custom/path/to/cli"
        />
      </ConfigField>
      <ConfigField label="Additional Params" className="col-span-2" hint="Comma-separated">
        <Input
          value={getStrArray(cfg, 'additional_params')}
          onChange={(e) => onChange('additional_params', e.target.value ? e.target.value.split(',').map((s) => s.trim()).filter(Boolean) : undefined)}
          placeholder="--verbose, --timeout 60"
        />
      </ConfigField>
      <ConfigField label="Environment Variables" className="col-span-2" hint="One KEY=VALUE per line">
        <Textarea
          className="resize-none font-mono text-xs"
          rows={3}
          value={getEnvString(cfg)}
          onChange={(e) => onChange('env', parseEnvString(e.target.value))}
          placeholder={'API_KEY=abc123\nDEBUG=true'}
          spellCheck={false}
        />
      </ConfigField>
    </CollapsibleSection>
  )
}

// ---------------------------------------------------------------------------
// Dispatcher
// ---------------------------------------------------------------------------

export function ExecutorConfigFields({ executorType, cfg, onChange }: { executorType: string } & ExecFieldProps) {
  const props = { cfg, onChange }
  if (executorType === 'claude_code') return <ClaudeCodeFields {...props} />
  if (executorType === 'codex') return <CodexFields {...props} />
  if (executorType === 'cursor') return <CursorFields {...props} />
  if (executorType === 'opencode') return <OpencodeFields {...props} />
  if (executorType === 'shell') return <ShellFields {...props} />
  return null
}

// ---------------------------------------------------------------------------
// Advanced raw JSON toggle
// ---------------------------------------------------------------------------

export function AdvancedJsonField({
  value,
  onChange,
  rows = 8,
  label = 'Raw JSON',
  defaultOpen = false,
}: {
  value: string
  onChange: (v: string) => void
  rows?: number
  label?: string
  defaultOpen?: boolean
}) {
  return (
    <CollapsibleSection title={label} defaultOpen={defaultOpen}>
      <Textarea
        className="resize-none font-mono text-xs"
        rows={rows}
        value={value}
        onChange={(e) => onChange(e.target.value)}
        spellCheck={false}
      />
    </CollapsibleSection>
  )
}
