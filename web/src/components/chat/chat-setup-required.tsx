import { Link } from '@tanstack/react-router'
import { ArrowUpRight, Gear, WarningCircle } from '@phosphor-icons/react'

export function ChatSetupRequired({ projectId }: { projectId?: string }) {
  return (
    <section
      aria-labelledby="chat-setup-required-heading"
      className="mx-auto flex w-full max-w-xl flex-col items-start rounded-xl border border-dashed border-ember-border bg-ember-surface p-5 sm:p-6"
      role="status"
    >
      <span className="flex h-9 w-9 items-center justify-center rounded-lg bg-background text-primary">
        <WarningCircle size={18} aria-hidden />
      </span>
      <h2 id="chat-setup-required-heading" className="mt-4 text-base font-semibold text-foreground">
        Chat setup required
      </h2>
      <p className="mt-2 max-w-lg text-sm leading-6 text-muted-foreground">
        {projectId
          ? 'This Project has one durable Agent Chat, but its Project Agent binding is not ready yet. Connect or select the Project Agent in settings before admitting a turn.'
          : 'The global Main Agent Chat is ready to keep its timeline, but no Main Agent binding is configured yet. Connect or select it in Agent settings before admitting a turn.'}
      </p>
      {projectId ? (
        <Link
          to="/agents"
          search={{ project: projectId }}
          className="mt-5 inline-flex items-center justify-center gap-1.5 whitespace-nowrap rounded-md bg-primary px-[14px] py-[7px] text-ui font-medium text-primary-foreground ring-offset-background transition-colors hover:bg-primary/90 focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring focus-visible:ring-offset-2"
        >
          <Gear size={14} aria-hidden />
          Open Project Agent settings
          <ArrowUpRight size={14} aria-hidden />
        </Link>
      ) : (
        <Link
          to="/agents"
          search={{ tab: 'bindings' }}
          className="mt-5 inline-flex items-center justify-center gap-1.5 whitespace-nowrap rounded-md bg-primary px-[14px] py-[7px] text-ui font-medium text-primary-foreground ring-offset-background transition-colors hover:bg-primary/90 focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring focus-visible:ring-offset-2"
        >
          <Gear size={14} aria-hidden />
          Open Main Agent settings
          <ArrowUpRight size={14} aria-hidden />
        </Link>
      )}
    </section>
  )
}
