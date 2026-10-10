import { useEffect, useState } from 'react'
import { useSearch } from '@tanstack/react-router'
import { Plus } from '@phosphor-icons/react'
import { Button } from '@/components/ui/button'
import {
  useFederatedAgentsQuery,
  useProvidersQuery,
} from '@/features/federation/hooks'
import type { FederatedAgent } from '@/features/federation/types'
import type { CliRuntimeEntryResponse, ProviderEntryResponse } from '@/types/generated'
import { PageHeader } from '@/features/federation/components'
import { AddProviderWizard, ProvidersTab } from '@/features/federation/ProvidersTab'
import { AgentsTab, NewAgentDialog } from '@/features/federation/AgentsTab'
import { ChangeModelDialog } from '@/features/federation/ChangeModelDialog'

const EMPTY_AGENTS: FederatedAgent[] = []
const EMPTY_ENTRIES: ProviderEntryResponse[] = []
const EMPTY_CLI_RUNTIMES: CliRuntimeEntryResponse[] = []

type SettingsTab = 'providers' | 'agents'

export function FederatedAgentsPage() {
  const routeSearch = useSearch({ strict: false }) as {
    provider?: string
    status?: string
    authorization?: string
    tab?: string
    identity?: string
  }
  const agentsQuery = useFederatedAgentsQuery()
  const providersQuery = useProvidersQuery()
  const [tab, setTab] = useState<SettingsTab>(() => {
    if (routeSearch.tab === 'providers' || routeSearch.status) return 'providers'
    return 'agents'
  })
  const [selectedId, setSelectedId] = useState<string | null>(routeSearch.identity ?? null)
  const [providerFilter, setProviderFilter] = useState('all')
  const [addProviderOpen, setAddProviderOpen] = useState(false)
  const [wizardOpen, setWizardOpen] = useState(false)
  const [wizardEntryId, setWizardEntryId] = useState<string | null>(null)
  const [changeModelTarget, setChangeModelTarget] = useState<FederatedAgent | null>(null)

  // A deep link (`?identity=...`) always wins the current selection.
  useEffect(() => {
    if (routeSearch.identity) setSelectedId(routeSearch.identity)
  }, [routeSearch.identity])

  const agents = agentsQuery.data?.items ?? EMPTY_AGENTS
  const entries = providersQuery.data?.items ?? EMPTY_ENTRIES
  const cliRuntimes = providersQuery.data?.cli_runtimes ?? EMPTY_CLI_RUNTIMES

  const tabs: { id: SettingsTab; label: string; count: number }[] = [
    { id: 'providers', label: 'Providers', count: entries.length },
    { id: 'agents', label: 'Agents', count: agents.length },
  ]

  function openNewAgentWizard(preselectedEntryId: string | null = null) {
    setWizardEntryId(preselectedEntryId)
    setWizardOpen(true)
  }

  function openChangeModel(agent: FederatedAgent) {
    setChangeModelTarget(agent)
  }

  return (
    <div className="min-h-full space-y-6 p-5 lg:p-8">
      <PageHeader
        eyebrow="Account-owned providers and agents"
        title="Agent Settings"
        description="Connect provider credentials and register external harness Agents."
        actions={
          tab === 'providers' ? (
            <Button onClick={() => setAddProviderOpen(true)}>
              <Plus size={16} aria-hidden />
              Add provider
            </Button>
          ) : tab === 'agents' ? (
            <Button onClick={() => openNewAgentWizard(null)}>
              <Plus size={16} aria-hidden />
              New agent
            </Button>
          ) : null
        }
      />

      <div
        role="tablist"
        aria-label="Agent Settings sections"
        className="flex flex-wrap gap-x-6 gap-y-1 border-b border-border-subtle"
      >
        {tabs.map((entry) => (
          <button
            key={entry.id}
            role="tab"
            id={`agent-settings-tab-${entry.id}`}
            aria-selected={tab === entry.id}
            aria-controls={`agent-settings-panel-${entry.id}`}
            className={`relative -mb-px inline-flex items-center gap-2 px-1 pb-3 pt-1 text-sm font-medium transition-colors focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring ${
              tab === entry.id ? 'text-foreground' : 'text-muted-foreground hover:text-foreground'
            }`}
            onClick={() => setTab(entry.id)}
          >
            {entry.label}
            <span
              className={`rounded-full border px-1.5 py-px font-mono text-micro ${
                tab === entry.id
                  ? 'border-ember-border bg-ember-surface text-primary'
                  : 'border-border-subtle bg-muted text-muted-foreground'
              }`}
            >
              {entry.count}
            </span>
            <span
              aria-hidden
              className={`absolute inset-x-0 bottom-0 h-0.5 rounded-full transition-colors ${
                tab === entry.id ? 'bg-primary' : 'bg-transparent'
              }`}
            />
          </button>
        ))}
      </div>

      {tab === 'providers' ? (
        <ProvidersTab
          entries={entries}
          cliRuntimes={cliRuntimes}
          isLoading={providersQuery.isLoading}
          isError={providersQuery.isError}
          onRetry={() => void providersQuery.refetch()}
          routeSearch={routeSearch}
          onShowAgents={(provider) => {
            setProviderFilter(provider)
            setTab('agents')
          }}
          onCreateAgentWithProvider={() => {
            setTab('agents')
            openNewAgentWizard(null)
          }}
        />
      ) : (
        <AgentsTab
          agents={agents}
          entries={entries}
          isLoading={agentsQuery.isLoading}
          isError={agentsQuery.isError}
          onRetry={() => void agentsQuery.refetch()}
          selectedId={selectedId}
          onSelect={setSelectedId}
          providerFilter={providerFilter}
          onProviderFilterChange={setProviderFilter}
          onChangeModel={(agent) => openChangeModel(agent)}
          onNewAgent={() => openNewAgentWizard(null)}
        />
      )}

      <AddProviderWizard
        open={addProviderOpen}
        onClose={() => setAddProviderOpen(false)}
        onCreateAgent={(entryId) => {
          setAddProviderOpen(false)
          setTab('agents')
          openNewAgentWizard(entryId)
        }}
      />
      <NewAgentDialog
        open={wizardOpen}
        onClose={() => setWizardOpen(false)}
        entries={entries}
        cliRuntimes={cliRuntimes}
        preselectedEntryId={wizardEntryId}
        onAddProvider={() => {
          setWizardOpen(false)
          setTab('providers')
          setAddProviderOpen(true)
        }}
      />
      <ChangeModelDialog
        agent={changeModelTarget}
        onClose={() => setChangeModelTarget(null)}
      />
    </div>
  )
}
