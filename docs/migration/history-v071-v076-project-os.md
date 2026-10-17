# Historical note: migration V071–V076

> Historical material retained from the former Getting Started section during
> PR14. It records an earlier migration contract and is not a description of
> current Project or Task operations. See [the current architecture](../architecture.md)
> and the [Architecture V2 contract](architecture-v2.md) for current behavior.

## Historical migration record: V071–V076

This section records the earlier Main/Project Agent migration contract. It is
kept as migration history and does not describe current Project or Task setup.

The correction is forward-only. Migrations `V059`–`V070` remain unchanged; the
replacement begins at `V071` or later. Legacy conversation/collaboration
messages, IDs, ordering, ordinary bodies, provenance, runtime metadata,
sessions, LCM/memory references, and turn-job history are preserved. Multiple
source threads merge deterministically by timestamp, source ID, and source
sequence. If no single safe Main/Project binding can be inferred, Forge marks
the account or Project `agent_setup_required` instead of guessing or promoting
a Task Worker. Expired/ambiguous leases become finite retry or terminal states,
never silent success. V075 then quarantines the retired Room and membership
tables as `legacy_*`, converts Room-scoped semantic memory to Agent Chat scope,
and rejects any new Room authority record while retaining source provenance.

The Charter, Project artifact, milestone, release, and shared-media metadata
for this change are added by the forward-only
`V076__project_charter_milestones_media.sql` migration. V001–V075 remain
immutable; existing media IDs, URLs, storage keys, metadata, and file bytes are
preserved in place, with no file move/duplication or on-disk layout break.

Projects that predate the Charter model are explicitly
`legacy_unverified`/`charter_setup_required`; migration never fabricates an
approved Charter from old chat, Tasks, memory, or inferred names. The Project
Chat, Tasks, evidence capture, and Document maintenance remain usable. The
Project Agent may draft an adoption Charter from authorized current state, but
only explicit user approval of its exact revision establishes Project truth and
unblocks release. Existing task media IDs, URLs, storage keys, and file bytes
remain in place; migration does not move or duplicate files or claim an on-disk
layout break. If a migration or server restart fails, old media references and
bytes remain usable and physical cleanup is retried separately after checking
attachments and release pins.
