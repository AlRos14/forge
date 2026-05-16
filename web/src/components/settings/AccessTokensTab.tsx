import { useState } from 'react'
import { Check, Copy, Key, Plus, Trash, Warning } from '@phosphor-icons/react'
import { toast } from 'sonner'
import { useCreatePat, useDeletePat, usePatsQuery } from '@/api/hooks'
import { Button } from '@/components/ui/button'
import {
  Dialog,
  DialogContent,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from '@/components/ui/dialog'
import { Input } from '@/components/ui/input'
import { Label } from '@/components/ui/label'
import { Skeleton } from '@/components/ui/skeleton'
import { getApiErrorMessage } from '@/lib/api-error'
import type { TokenResponse } from '@/types/generated'

function formatRelativeTime(iso: string): string {
  const diff = Date.now() - new Date(iso).getTime()
  const minutes = Math.floor(diff / 60000)
  if (minutes < 1) return 'just now'
  if (minutes < 60) return `${minutes}m ago`
  const hours = Math.floor(minutes / 60)
  if (hours < 24) return `${hours}h ago`
  const days = Math.floor(hours / 24)
  if (days < 30) return `${days}d ago`
  return new Date(iso).toLocaleDateString()
}

function formatExpiry(iso: string | null): string {
  if (!iso) return 'Never'
  const date = new Date(iso)
  const now = new Date()
  if (date < now) return 'Expired'
  return date.toLocaleDateString()
}

const EXPIRY_OPTIONS = [
  { label: 'No expiry', value: '' },
  { label: '30 days', value: '30' },
  { label: '90 days', value: '90' },
  { label: '180 days', value: '180' },
  { label: '1 year', value: '365' },
]

function computeExpiryDate(days: string): string | null {
  if (!days) return null
  const d = new Date()
  d.setDate(d.getDate() + parseInt(days, 10))
  return d.toISOString()
}

function TokenRow({
  token,
  onDelete,
  deletePending,
}: {
  token: TokenResponse
  onDelete: () => void
  deletePending: boolean
}) {
  const [confirming, setConfirming] = useState(false)

  return (
    <div className="flex items-center gap-4 rounded-lg border border-border-subtle bg-card px-4 py-3">
      <div className="flex h-8 w-8 shrink-0 items-center justify-center rounded-lg bg-muted text-muted-foreground">
        <Key size={15} />
      </div>
      <div className="min-w-0 flex-1">
        <p className="truncate text-sm font-medium text-foreground">{token.name}</p>
        <p className="mt-0.5 font-mono text-xs text-muted-foreground">
          {token.prefix}...
        </p>
      </div>
      <div className="hidden shrink-0 text-right sm:block">
        <p className="text-xs text-muted-foreground">Last used</p>
        <p className="text-xs font-medium text-foreground">
          {token.last_used_at ? formatRelativeTime(token.last_used_at) : 'Never'}
        </p>
      </div>
      <div className="hidden shrink-0 text-right sm:block">
        <p className="text-xs text-muted-foreground">Expires</p>
        <p
          className={`text-xs font-medium ${token.expires_at && new Date(token.expires_at) < new Date() ? 'text-destructive' : 'text-foreground'}`}
        >
          {formatExpiry(token.expires_at)}
        </p>
      </div>
      <div className="shrink-0">
        {confirming ? (
          <div className="flex items-center gap-1.5">
            <button
              type="button"
              className="cursor-pointer rounded px-2 py-1 text-xs text-muted-foreground transition-colors hover:bg-accent hover:text-foreground"
              onClick={() => setConfirming(false)}
            >
              Cancel
            </button>
            <button
              type="button"
              disabled={deletePending}
              className="cursor-pointer rounded bg-destructive px-2 py-1 text-xs font-medium text-destructive-foreground transition-opacity hover:opacity-90 disabled:opacity-50"
              onClick={onDelete}
            >
              Delete
            </button>
          </div>
        ) : (
          <button
            type="button"
            className="cursor-pointer rounded p-1.5 text-muted-foreground transition-colors hover:bg-destructive/10 hover:text-destructive focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring"
            aria-label="Delete token"
            onClick={() => setConfirming(true)}
          >
            <Trash size={15} />
          </button>
        )}
      </div>
    </div>
  )
}

function CopyOnceDialog({
  token,
  open,
  onClose,
}: {
  token: string
  open: boolean
  onClose: () => void
}) {
  const [copied, setCopied] = useState(false)

  function handleCopy() {
    void navigator.clipboard.writeText(token).then(() => {
      setCopied(true)
      setTimeout(() => setCopied(false), 2000)
    })
  }

  return (
    <Dialog open={open} onOpenChange={(v) => { if (!v) onClose() }}>
      <DialogContent className="max-w-md">
        <DialogHeader>
          <DialogTitle>Personal access token created</DialogTitle>
        </DialogHeader>
        <div className="space-y-4">
          <div className="flex items-start gap-2.5 rounded-lg border border-amber-300 bg-amber-50 px-3.5 py-3 text-sm text-amber-800 dark:border-amber-700 dark:bg-amber-900/20 dark:text-amber-300">
            <Warning size={16} weight="fill" className="mt-0.5 shrink-0" />
            <span>Store this token now — it will not be shown again.</span>
          </div>
          <div>
            <Label className="mb-1.5 block text-xs text-muted-foreground">Your new token</Label>
            <div className="flex items-center gap-2">
              <code className="flex-1 overflow-x-auto rounded-lg border border-border-subtle bg-muted px-3 py-2 font-mono text-xs text-foreground">
                {token}
              </code>
              <button
                type="button"
                onClick={handleCopy}
                className="flex h-8 w-8 shrink-0 cursor-pointer items-center justify-center rounded-lg border border-input bg-card transition-colors hover:bg-accent focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring"
                aria-label="Copy token"
              >
                {copied ? (
                  <Check size={14} className="text-success" />
                ) : (
                  <Copy size={14} className="text-muted-foreground" />
                )}
              </button>
            </div>
          </div>
        </div>
        <DialogFooter>
          <Button onClick={onClose}>Done</Button>
        </DialogFooter>
      </DialogContent>
    </Dialog>
  )
}

function CreateTokenDialog({
  open,
  onOpenChange,
}: {
  open: boolean
  onOpenChange: (v: boolean) => void
}) {
  const [name, setName] = useState('')
  const [expiry, setExpiry] = useState('')
  const [error, setError] = useState('')
  const [createdToken, setCreatedToken] = useState<string | null>(null)
  const createPat = useCreatePat()

  function handleClose(v: boolean) {
    if (!v) {
      setName('')
      setExpiry('')
      setError('')
    }
    onOpenChange(v)
  }

  async function handleSubmit(e: React.FormEvent) {
    e.preventDefault()
    const trimmed = name.trim()
    if (!trimmed) {
      setError('Token name is required')
      return
    }
    setError('')
    try {
      const result = await createPat.mutateAsync({
        name: trimmed,
        expires_at: computeExpiryDate(expiry),
      }) as TokenResponse
      setName('')
      setExpiry('')
      onOpenChange(false)
      if (result.token) {
        setCreatedToken(result.token)
      }
    } catch (err) {
      setError(getApiErrorMessage(err, 'Failed to create token'))
    }
  }

  return (
    <>
      <Dialog open={open} onOpenChange={handleClose}>
        <DialogContent className="max-w-sm">
          <form onSubmit={(e) => { void handleSubmit(e) }}>
            <DialogHeader>
              <DialogTitle>Create access token</DialogTitle>
            </DialogHeader>
            <div className="my-4 space-y-4">
              <div className="space-y-1.5">
                <Label htmlFor="pat-name">Token name</Label>
                <Input
                  id="pat-name"
                  value={name}
                  onChange={(e) => setName(e.target.value)}
                  placeholder="e.g. CI pipeline, local dev"
                  autoFocus
                />
              </div>
              <div className="space-y-1.5">
                <Label htmlFor="pat-expiry">Expiry</Label>
                <select
                  id="pat-expiry"
                  value={expiry}
                  onChange={(e) => setExpiry(e.target.value)}
                  className="flex h-9 w-full cursor-pointer rounded-md border border-input bg-transparent px-3 py-1 text-sm shadow-sm transition-colors focus-visible:outline-none focus-visible:ring-1 focus-visible:ring-ring"
                >
                  {EXPIRY_OPTIONS.map((opt) => (
                    <option key={opt.value} value={opt.value}>
                      {opt.label}
                    </option>
                  ))}
                </select>
              </div>
              {error && <p className="text-sm text-destructive">{error}</p>}
            </div>
            <DialogFooter>
              <button
                type="button"
                className="cursor-pointer rounded-md border px-3 py-1.5 text-sm transition-colors hover:bg-accent"
                onClick={() => handleClose(false)}
              >
                Cancel
              </button>
              <Button type="submit" disabled={createPat.isPending}>
                {createPat.isPending ? 'Creating...' : 'Create token'}
              </Button>
            </DialogFooter>
          </form>
        </DialogContent>
      </Dialog>

      {createdToken && (
        <CopyOnceDialog
          token={createdToken}
          open={Boolean(createdToken)}
          onClose={() => setCreatedToken(null)}
        />
      )}
    </>
  )
}

export function AccessTokensTab() {
  const patsQuery = usePatsQuery()
  const deletePat = useDeletePat()
  const [createOpen, setCreateOpen] = useState(false)

  const tokens = patsQuery.data ?? []

  function handleDelete(tokenId: string) {
    deletePat.mutate(tokenId, {
      onError: (err) => toast.error(getApiErrorMessage(err, 'Failed to delete token')),
    })
  }

  return (
    <>
      <div className="mb-8 flex items-start justify-between gap-4">
        <div>
          <h2 className="text-page font-semibold tracking-tight">Personal Access Tokens</h2>
          <p className="mt-1 text-sm text-muted-foreground">
            Tokens prefixed with <code className="rounded bg-muted px-1 font-mono text-xs">fg_</code>{' '}
            can authenticate CLI tools, MCP clients, and automations.
          </p>
        </div>
        <Button onClick={() => setCreateOpen(true)}>
          <Plus size={14} className="mr-1.5" />
          New token
        </Button>
      </div>

      {patsQuery.isLoading ? (
        <div className="space-y-2">
          <Skeleton className="h-14 w-full" />
          <Skeleton className="h-14 w-full" />
        </div>
      ) : tokens.length === 0 ? (
        <div className="flex flex-col items-center justify-center rounded-xl border border-dashed border-border py-12 text-center">
          <Key size={28} className="mb-3 text-muted-foreground/50" />
          <p className="text-sm font-medium text-foreground">No access tokens</p>
          <p className="mt-1 text-sm text-muted-foreground">
            Create a token to authenticate CLI tools and automations.
          </p>
          <Button className="mt-4" onClick={() => setCreateOpen(true)}>
            <Plus size={14} className="mr-1.5" />
            New token
          </Button>
        </div>
      ) : (
        <div className="space-y-2">
          {tokens.map((token) => (
            <TokenRow
              key={token.id}
              token={token}
              onDelete={() => handleDelete(token.id)}
              deletePending={deletePat.isPending}
            />
          ))}
        </div>
      )}

      <CreateTokenDialog open={createOpen} onOpenChange={setCreateOpen} />
    </>
  )
}
