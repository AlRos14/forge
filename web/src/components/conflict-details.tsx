import { getApiConflictDetails } from '@/lib/api-error'

type ConflictDetailsProps = {
  error?: unknown
  fallbackAuthority: string
}

function isRecord(value: unknown): value is Record<string, unknown> {
  return typeof value === 'object' && value !== null && !Array.isArray(value)
}

function firstValue(...values: unknown[]): unknown {
  return values.find((value) => value !== undefined && value !== null && value !== '')
}

function displayValue(value: unknown): string | null {
  if (typeof value === 'string' || typeof value === 'number') return String(value)
  if (typeof value === 'boolean') return value ? 'yes' : 'no'
  return null
}

function sideValue(
  details: Record<string, unknown> | undefined,
  side: 'expected' | 'current',
  field: 'revision' | 'digest',
): string | null {
  if (!details) return null
  const nested = isRecord(details[side]) ? details[side] : undefined
  const value =
    field === 'revision'
      ? firstValue(
          nested?.revision,
          nested?.revision_id,
          nested?.version,
          details[`${side}_revision`],
          details[`${side}_revision_id`],
          details[`${side}_version`],
        )
      : firstValue(
          nested?.digest,
          nested?.content_digest,
          nested?.render_digest,
          details[`${side}_digest`],
          details[`${side}_content_digest`],
          details[`${side}_render_digest`],
        )
  return displayValue(value)
}

function authorityValue(details: Record<string, unknown> | undefined): string | null {
  if (!details) return null
  const authority = isRecord(details.authority) ? details.authority : undefined
  return displayValue(
    firstValue(
      details.authority_domain,
      authority?.domain,
      authority?.name,
      details.authority,
      details.domain,
      details.affected_authority_domain,
    ),
  )
}

export function ConflictDetails({ error, fallbackAuthority }: ConflictDetailsProps) {
  const details = getApiConflictDetails(error)
  const authority = authorityValue(details) ?? fallbackAuthority
  const expectedRevision = sideValue(details, 'expected', 'revision')
  const currentRevision = sideValue(details, 'current', 'revision')
  const expectedDigest = sideValue(details, 'expected', 'digest')
  const currentDigest = sideValue(details, 'current', 'digest')
  const values = [expectedRevision, currentRevision, expectedDigest, currentDigest]

  return (
    <div className="mt-2 rounded-md border border-warning/40 bg-warning/10 px-2.5 py-2 text-xs text-warning">
      <p>
        <span className="font-semibold">Authority:</span> {authority}
      </p>
      {values.some(Boolean) ? (
        <dl className="mt-2 grid min-w-0 gap-x-3 gap-y-1 sm:grid-cols-2">
          {expectedRevision ? (
            <div className="min-w-0">
              <dt className="text-micro uppercase tracking-[0.08em]">Expected revision</dt>
              <dd className="break-all font-mono text-micro">{expectedRevision}</dd>
            </div>
          ) : null}
          {currentRevision ? (
            <div className="min-w-0">
              <dt className="text-micro uppercase tracking-[0.08em]">Current revision</dt>
              <dd className="break-all font-mono text-micro">{currentRevision}</dd>
            </div>
          ) : null}
          {expectedDigest ? (
            <div className="min-w-0">
              <dt className="text-micro uppercase tracking-[0.08em]">Expected digest</dt>
              <dd className="break-all font-mono text-micro">{expectedDigest}</dd>
            </div>
          ) : null}
          {currentDigest ? (
            <div className="min-w-0">
              <dt className="text-micro uppercase tracking-[0.08em]">Current digest</dt>
              <dd className="break-all font-mono text-micro">{currentDigest}</dd>
            </div>
          ) : null}
        </dl>
      ) : null}
    </div>
  )
}
