import { useEffect, useState, type FormEvent } from 'react'
import { toast } from 'sonner'
import {
  useRepoPrFeedbackPolicyQuery,
  useUpdateRepoPrFeedbackPolicy,
} from '@/api/hooks'
import { Button } from '@/components/ui/button'
import { Input } from '@/components/ui/input'
import { Label } from '@/components/ui/label'
import { Select } from '@/components/ui/select'
import { getApiErrorCode, getApiErrorMessage, isApiStatus } from '@/lib/api-error'
import type {
  PrFeedbackPolicy,
  PrFeedbackPolicyResponse,
  PrFeedbackSummaryMode,
  UpdatePrFeedbackPolicyRequest,
} from '@/types/generated'

function policyFields(policy: PrFeedbackPolicy) {
  return {
    trusted_author_associations: policy.trusted_author_associations.join(', '),
    trusted_logins: policy.trusted_logins.join(', '),
    max_items_per_poll: String(policy.max_items_per_poll),
  }
}

export function RepoPrFeedbackPolicyEditor({ repoId }: { repoId: string }) {
  const query = useRepoPrFeedbackPolicyQuery(repoId)
  const updatePolicy = useUpdateRepoPrFeedbackPolicy()
  const [draft, setDraft] = useState<PrFeedbackPolicy | null>(null)
  const [associationsText, setAssociationsText] = useState('')
  const [trustedLoginsText, setTrustedLoginsText] = useState('')
  const [maxItemsText, setMaxItemsText] = useState('50')
  const [conflict, setConflict] = useState(false)
  const [errorMessage, setErrorMessage] = useState<string | null>(null)

  useEffect(() => {
    if (!query.data || draft) return
    setDraft(query.data.policy)
    const fields = policyFields(query.data.policy)
    setAssociationsText(fields.trusted_author_associations)
    setTrustedLoginsText(fields.trusted_logins)
    setMaxItemsText(fields.max_items_per_poll)
  }, [draft, query.data])

  const reloadLatest = async () => {
    const result = await query.refetch()
    if (!result.data) return
    setDraft(result.data.policy)
    const fields = policyFields(result.data.policy)
    setAssociationsText(fields.trusted_author_associations)
    setTrustedLoginsText(fields.trusted_logins)
    setMaxItemsText(fields.max_items_per_poll)
    setConflict(false)
    setErrorMessage(null)
  }

  const submit = async (event: FormEvent<HTMLFormElement>) => {
    event.preventDefault()
    if (!draft || conflict) return
    const maxItems = Number(maxItemsText)
    const policy: PrFeedbackPolicy = {
      ...draft,
      trusted_author_associations: associationsText
        .split(',')
        .map((value) => value.trim())
        .filter(Boolean),
      trusted_logins: trustedLoginsText
        .split(/[\n,]/)
        .map((value) => value.trim())
        .filter(Boolean),
      max_items_per_poll: maxItems,
    }
    const body: UpdatePrFeedbackPolicyRequest = {
      expected_updated_at: query.data?.updated_at ?? null,
      policy,
    }
    setErrorMessage(null)
    try {
      const saved: PrFeedbackPolicyResponse = await updatePolicy.mutateAsync({ repoId, body })
      setDraft(saved.policy)
      const fields = policyFields(saved.policy)
      setAssociationsText(fields.trusted_author_associations)
      setTrustedLoginsText(fields.trusted_logins)
      setMaxItemsText(fields.max_items_per_poll)
      toast.success('PR feedback policy saved')
    } catch (error) {
      if (
        isApiStatus(error, 409) &&
        getApiErrorCode(error) === 'pr_feedback.policy_version_conflict'
      ) {
        setConflict(true)
        setErrorMessage('This policy changed while you were editing it. Reload the latest policy before saving.')
        return
      }
      if (isApiStatus(error, 400) && getApiErrorCode(error) === 'pr_feedback.policy_invalid') {
        setErrorMessage('Policy values are invalid. Review the trusted associations, logins, and polling limit.')
        return
      }
      setErrorMessage(getApiErrorMessage(error, 'PR feedback policy could not be saved'))
    }
  }

  if (query.isLoading || !draft) {
    return (
      <section className="space-y-2 rounded-lg border p-4" aria-labelledby="pr-feedback-policy-title">
        <h3 id="pr-feedback-policy-title" className="font-semibold">PR feedback policy</h3>
        {query.isLoading ? <p className="text-sm text-muted-foreground">Loading policy…</p> : null}
        {query.isError ? (
          <div role="alert" className="text-sm text-destructive">
            PR feedback policy could not be loaded.{' '}
            <Button size="sm" variant="outline" onClick={() => void query.refetch()}>
              Retry
            </Button>
          </div>
        ) : null}
      </section>
    )
  }

  return (
    <section className="space-y-4 rounded-lg border p-4" aria-labelledby="pr-feedback-policy-title">
      <div>
        <h3 id="pr-feedback-policy-title" className="font-semibold">PR feedback policy</h3>
        <p className="mt-1 text-sm text-muted-foreground">
          Choose which feedback Forge ingests and how corrections or summaries are authorized.
          Merge remains a human operation.
        </p>
      </div>
      {errorMessage ? (
        <div role="alert" className="space-y-2 rounded-md border border-destructive/40 p-3 text-sm text-destructive">
          <p>{errorMessage}</p>
          {conflict ? (
            <Button size="sm" variant="outline" onClick={() => void reloadLatest()}>
              Reload latest policy
            </Button>
          ) : null}
        </div>
      ) : null}
      <form onSubmit={(event) => void submit(event)} className="space-y-3">
        <label className="flex items-center gap-2 text-sm">
          <input
            type="checkbox"
            checked={draft.ingest}
            onChange={(event) => setDraft({ ...draft, ingest: event.target.checked })}
          />
          Ingest pull request feedback
        </label>
        <div className="space-y-1.5">
          <Label htmlFor={`pr-feedback-associations-${repoId}`}>Trusted author associations</Label>
          <Input
            id={`pr-feedback-associations-${repoId}`}
            value={associationsText}
            onChange={(event) => setAssociationsText(event.target.value)}
            placeholder="OWNER, MEMBER, COLLABORATOR"
          />
        </div>
        <div className="space-y-1.5">
          <Label htmlFor={`pr-feedback-logins-${repoId}`}>Trusted logins</Label>
          <Input
            id={`pr-feedback-logins-${repoId}`}
            value={trustedLoginsText}
            onChange={(event) => setTrustedLoginsText(event.target.value)}
            placeholder="Comma separated GitHub logins"
          />
        </div>
        <div className="space-y-1.5">
          <Label htmlFor={`pr-feedback-limit-${repoId}`}>Maximum items per poll</Label>
          <Input
            id={`pr-feedback-limit-${repoId}`}
            type="number"
            min={1}
            max={100}
            value={maxItemsText}
            onChange={(event) => setMaxItemsText(event.target.value)}
          />
        </div>
        <div className="grid gap-3 sm:grid-cols-2">
          <div className="space-y-1.5">
            <Label htmlFor={`pr-feedback-push-${repoId}`}>Push corrections</Label>
            <Select
              id={`pr-feedback-push-${repoId}`}
              value={draft.push_corrections}
              options={[
                { value: 'human_approval', label: 'Human approval' },
                { value: 'automatic', label: 'Automatic' },
              ]}
              onChange={(value) =>
                setDraft({ ...draft, push_corrections: value as PrFeedbackPolicy['push_corrections'] })
              }
            />
          </div>
          <div className="space-y-1.5">
            <Label htmlFor={`pr-feedback-summary-${repoId}`}>Post summary</Label>
            <Select
              id={`pr-feedback-summary-${repoId}`}
              value={draft.post_summary}
              options={[
                { value: 'human_approval', label: 'Human approval' },
                { value: 'automatic', label: 'Automatic' },
                { value: 'off', label: 'Off' },
              ]}
              onChange={(value) =>
                setDraft({ ...draft, post_summary: value as PrFeedbackSummaryMode })
              }
            />
          </div>
        </div>
        <Button type="submit" disabled={updatePolicy.isPending || conflict}>
          {updatePolicy.isPending ? 'Saving…' : 'Save policy'}
        </Button>
      </form>
    </section>
  )
}
