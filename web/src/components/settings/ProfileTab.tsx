import { useEffect, useState } from 'react'
import { toast } from 'sonner'
import { useUpdateMe } from '@/api/hooks'
import { Button } from '@/components/ui/button'
import { Input } from '@/components/ui/input'
import { Label } from '@/components/ui/label'
import { getApiErrorMessage } from '@/lib/api-error'
import { useAuthStore } from '@/stores/auth'

export function ProfileTab() {
  const user = useAuthStore((s) => s.user)
  const updateMe = useUpdateMe()

  const [displayName, setDisplayName] = useState(user?.display_name ?? '')
  const [email, setEmail] = useState(user?.email ?? '')
  const [error, setError] = useState('')

  useEffect(() => {
    setDisplayName(user?.display_name ?? '')
    setEmail(user?.email ?? '')
  }, [user])

  async function handleSubmit(e: React.FormEvent) {
    e.preventDefault()
    setError('')

    const trimmedEmail = email.trim()
    const trimmedName = displayName.trim()

    if (!trimmedEmail) {
      setError('Email is required')
      return
    }

    try {
      await updateMe.mutateAsync({
        email: trimmedEmail !== user?.email ? trimmedEmail : undefined,
        display_name: trimmedName !== (user?.display_name ?? '') ? trimmedName || null : undefined,
      })
      toast.success('Profile updated')
    } catch (err) {
      setError(getApiErrorMessage(err, 'Failed to update profile'))
    }
  }

  return (
    <>
      <div className="mb-8">
        <h2 className="text-page font-semibold tracking-tight">Profile</h2>
        <p className="mt-1 text-sm text-muted-foreground">
          Update your display name and email address.
        </p>
      </div>

      <form onSubmit={(e) => { void handleSubmit(e) }} className="max-w-sm space-y-5">
        <div className="space-y-1.5">
          <Label htmlFor="profile-name">Display name</Label>
          <Input
            id="profile-name"
            value={displayName}
            onChange={(e) => setDisplayName(e.target.value)}
            placeholder="Your name"
            maxLength={255}
          />
        </div>

        <div className="space-y-1.5">
          <Label htmlFor="profile-email">Email</Label>
          <Input
            id="profile-email"
            type="email"
            value={email}
            onChange={(e) => setEmail(e.target.value)}
            placeholder="you@example.com"
          />
        </div>

        {error && <p className="text-sm text-destructive">{error}</p>}

        <Button type="submit" disabled={updateMe.isPending}>
          {updateMe.isPending ? 'Saving...' : 'Save changes'}
        </Button>
      </form>
    </>
  )
}
