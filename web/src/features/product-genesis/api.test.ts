import { afterEach, describe, expect, it, vi } from 'vitest'
import {
  approveProductGenesisCharterRevision,
  createProjectFromCharterApproval,
  getProductGenesisCharter,
  productGenesisApiPaths,
  saveProductGenesisCharterRevision,
} from './api'

const apiFetch = vi.hoisted(() => vi.fn())

vi.mock('@/api/client', () => ({ apiFetch }))

describe('Product Genesis Charter API', () => {
  afterEach(() => {
    apiFetch.mockReset()
  })

  it('reads the canonical Charter projection from the Main Agent scope', async () => {
    const response = {
      charter: null,
      revisions: [],
      current_draft_revision: null,
      current_approved_revision: null,
      approval: null,
      selected_project_agent: null,
    }
    apiFetch.mockResolvedValueOnce(response)

    await expect(getProductGenesisCharter('genesis-1')).resolves.toEqual(response)
    expect(apiFetch).toHaveBeenCalledWith(productGenesisApiPaths.charter('genesis-1'))
  })

  it('posts a typed immutable revision to the exact revisions route', async () => {
    const input = {
      mutation: {
        expected_version: 2,
        expected_digest: null,
        idempotency_key: 'revision:2',
        deduplication_key: null,
        authorization: {
          principal: { kind: 'user' as const, id: 'u1', display_name: null },
          authorization_basis: 'test',
          action: 'test',
          event_id: 'e1',
          occurred_at: '2026-08-13T00:00:00Z',
        },
      },
      charter_id: 'charter-1',
      base_revision_id: 'revision-1',
      project_mode: 'compact' as const,
      maturity: 'mvp' as const,
      content: {} as never,
      rendered_view: '# Project',
      render_version: 'v1',
      provenance: {} as never,
    }
    apiFetch.mockResolvedValueOnce({ id: 'revision-2' })

    await saveProductGenesisCharterRevision('genesis-1', input)

    expect(apiFetch).toHaveBeenCalledWith(productGenesisApiPaths.charterRevisions('genesis-1'), {
      method: 'POST',
      body: JSON.stringify(input),
    })
  })

  it('binds approval to the exact revision, digests, and responder revisions', async () => {
    const input = {
      mutation: {
        expected_version: 3,
        expected_digest: 'content-digest',
        idempotency_key: 'approval:revision-3',
        deduplication_key: 'approval:revision-3',
        authorization: {
          principal: { kind: 'user' as const, id: 'u1', display_name: null },
          authorization_basis: 'test',
          action: 'test',
          event_id: 'e2',
          occurred_at: '2026-08-13T00:00:00Z',
        },
      },
      charter_id: 'charter-1',
      revision_id: 'revision-3',
      content_digest: 'content-digest',
      render_digest: 'render-digest',
      expected_charter_version: 3,
      approved_project_name: 'Forge Test',
      approved_project_slug: 'forge-test',
      project_mode: 'standard' as const,
      selected_project_agent_identity_id: 'identity-1',
      selected_project_agent_profile_revision_id: 'profile-r4',
      selected_project_agent_operating_skill_revision: 'forge.project.orchestration/v1',
      selected_project_agent_policy_digest: 'policy-digest',
    }
    apiFetch.mockResolvedValueOnce({ id: 'approval-1', state: 'active' })

    await approveProductGenesisCharterRevision('genesis-1', 'revision-3', input)

    expect(apiFetch).toHaveBeenCalledWith(
      productGenesisApiPaths.approveCharterRevision('genesis-1', 'revision-3'),
      { method: 'POST', body: JSON.stringify(input) },
    )
  })

  it('creates and replays a Project only through the approval receipt action', async () => {
    const input = {
      approval_id: 'approval-1',
      idempotency_key: 'create:approval-1',
      authorization: {
        principal: { kind: 'user' as const, id: 'u1', display_name: null },
        authorization_basis: 'test',
        action: 'test',
        event_id: 'e3',
        occurred_at: '2026-08-13T00:00:00Z',
      },
    }
    apiFetch.mockResolvedValueOnce({ project_id: 'project-1' })

    await createProjectFromCharterApproval(input)

    expect(apiFetch).toHaveBeenCalledWith('/projects', {
      method: 'POST',
      body: JSON.stringify(input),
    })
  })
})
