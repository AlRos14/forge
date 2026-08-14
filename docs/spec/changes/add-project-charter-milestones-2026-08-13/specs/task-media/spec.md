## MODIFIED Requirements

### Requirement: Task-Scoped Media Uploads

Forge SHALL preserve the task-scoped media upload/list/retrieve API for images, videos, and downloadable files. A Task upload SHALL retain its existing media asset ID, Task media ID, Task ID, filename, content type, byte size, storage key, file bytes, and stable Task API URL while adding Project ownership/attachment metadata. If a new internal Project `MediaAsset` ID is needed, it SHALL be an additive mapping to the same legacy row/storage key and SHALL not replace any existing ID. An authorized same-Project milestone MAY later attach the same underlying asset without moving or copying its bytes; this additional attachment SHALL NOT cause it to appear in another Task's media list or claim an on-disk layout break.

#### Scenario: Upload image evidence through a Task

- **WHEN** an authorized user uploads a supported image for a Task
- **THEN** Forge stores one asset under the owning Project metadata without moving or duplicating the existing bytes, creates one Task attachment, and returns the existing Task media response shape and URL
- **AND** listing that Task shows the media item

#### Scenario: Upload video evidence through a Task

- **WHEN** an authorized user uploads a supported video for a Task
- **THEN** Forge preserves the existing asset/storage key and Task attachment and returns metadata plus a stable Task URL suitable for existing comments

#### Scenario: Reuse Task upload for same-Project milestone

- **GIVEN** a milestone belongs to the same Project as a Task media item
- **WHEN** an authorized actor attaches that asset as milestone evidence
- **THEN** Forge adds an attachment without moving or duplicating stored bytes
- **AND** the original Task media identity, list entry, and URL remain unchanged while its Task attachment exists

#### Scenario: Attempt cross-Project reuse

- **WHEN** a milestone references a Task media asset owned by another Project
- **THEN** Forge denies the action before exposing the asset's filename, checksum, URL, or attachment count

#### Scenario: Stable Task URL survives restart

- **WHEN** a Task attachment remains active across a Forge server restart
- **THEN** its existing Task media URL retrieves the same asset
- **AND** the URL does not require regeneration or expose the underlying storage key

### Requirement: Task Media Lifecycle

Forge SHALL preserve Task media availability for archived, done, and cancelled Tasks and SHALL keep Workspace cleanup independent from Project media storage. Deleting a Task or Task media item SHALL remove/tombstone its Task attachment and make its Task URL/list entry unavailable. Forge SHALL physically remove the existing asset bytes only when no active attachment or immutable release pin references them. A release-pinned asset SHALL remain available through its stable authorized Project evidence URL even after the originating Task attachment is removed. Evidence availability SHALL be one of `available`, `quarantined`, `redacted`, or `purged`; quarantine blocks serving/counting pending review, redaction may expose only a policy-permitted derivative/metadata whose exact digest is accepted by the frozen policy, and an authorized redaction or mandatory purge SHALL append an availability projection/event making affected release evidence `evidence_unavailable` while retaining permitted tombstone/digest/audit metadata without rewriting the release manifest.

#### Scenario: Retrieve active Task media

- **WHEN** an authorized user opens a valid Task media URL whose Task attachment is active
- **THEN** Forge streams the referenced Project asset with the recorded content type
- **AND** authorization follows the owning Task's Project

#### Scenario: Workspace cleanup preserves Task attachment

- **WHEN** a Task reaches a terminal state and Forge cleans up its Task Workspace
- **THEN** the Task media attachment and URL remain available
- **AND** existing Task comments continue to render their evidence

#### Scenario: Delete unshared Task media

- **GIVEN** a Task media asset has no other attachment or release pin
- **WHEN** an authorized actor deletes the Task media item or owning Task under existing policy
- **THEN** Forge removes/tombstones the Task attachment, makes the Task URL unavailable, and physically deletes the unreferenced existing asset bytes

#### Scenario: Delete Task whose media is release-pinned

- **GIVEN** an immutable release pins an asset originally uploaded through a Task
- **WHEN** the Task or Task media item is deleted
- **THEN** Forge removes/tombstones the Task attachment and makes its Task URL unavailable
- **AND** it retains the asset bytes, release evidence metadata, checksum, and stable authorized Project URL

#### Scenario: Last release pin is mandatory-purged

- **GIVEN** a deleted Task has no attachment and a release is the last pin on its asset
- **WHEN** an authorized privacy/security/legal purge removes the bytes
- **THEN** Forge deletes the stored asset bytes, changes evidence availability to `purged`, marks release evidence `evidence_unavailable`, and preserves an immutable release tombstone/digest/audit record
- **AND** neither the former Task URL nor Project media URL serves the bytes

#### Scenario: Future hard delete cascades Task attachments

- **WHEN** a future hard-delete operation removes a Task row
- **THEN** its Task attachment rows are removed by database cascade or equivalent transactional cleanup
- **AND** the shared asset remains only if another authorized attachment or release pin exists

#### Scenario: Task deletion races milestone attachment

- **GIVEN** a Task attachment is the only current reference to an asset
- **WHEN** one transaction deletes it while another same-Project transaction attaches the asset to a milestone
- **THEN** Forge serializes the reference changes so exactly one canonical outcome wins
- **AND** it either retains bytes for the committed milestone attachment or rejects that attachment before guarded garbage collection
- **AND** it never commits a live attachment whose bytes were deleted

#### Scenario: Forge restarts during physical cleanup

- **WHEN** Forge restarts after an asset becomes a garbage-collection candidate but before or during file deletion
- **THEN** reconciliation re-checks active attachments and release pins before completing idempotent cleanup
- **AND** it preserves referenced assets and removes only confirmed unreferenced bytes

### Requirement: Task Media Authorization

Forge SHALL require authenticated Task access for Task media listing, retrieval, upload, and deletion. Task URLs SHALL authorize through the active owning Task attachment and its Project. Project/milestone attachments and release pins SHALL NOT make a deleted Task URL valid or expose stable public URLs; the separate Project evidence URL SHALL authorize through the owning Project and current evidence availability.

#### Scenario: Unauthenticated media retrieval is rejected

- **WHEN** an unauthenticated client requests an active or deleted Task media URL
- **THEN** Forge rejects the request and streams no asset bytes

#### Scenario: Non-member media retrieval is rejected

- **WHEN** an authenticated user without access to the owning Project requests a Task media URL
- **THEN** Forge rejects the request without revealing whether another attachment or release pin exists

#### Scenario: Authorized member retrieves active Task media

- **WHEN** an authenticated user with Project access requests an active Task media URL
- **THEN** Forge streams the asset with the recorded content type

#### Scenario: Task URL after attachment removal

- **GIVEN** an asset remains pinned by a release
- **WHEN** a caller requests its deleted originating Task media URL
- **THEN** Forge returns the documented unavailable response
- **AND** access requires the separate authorized Project media URL

#### Scenario: Authorized deletion retains pinned bytes

- **WHEN** an authorized Project owner or admin deletes a Task media item that is release-pinned
- **THEN** Forge removes the Task attachment and records deletion metadata
- **AND** the existing Task delete response remains valid while Project/release views truthfully show that the shared asset is retained

### Requirement: Authorized Project Media Disposition

Forge SHALL expose the audited Project-scoped media disposition routes
`POST /api/v1/projects/{project_id}/media/{asset_id}/redact` and
`POST /api/v1/projects/{project_id}/media/{asset_id}/purge`. Only the Project
owner or an admin member may invoke them. Each request SHALL be a
`ProjectMediaTombstoneRequest` containing the current asset
`expected_version`, an idempotency key, explicit user authorization whose action
is respectively `project.media.redact` or `project.media.purge`, and a bounded
non-empty reason. The operation SHALL persist an immutable tombstone/audit
record and a replayable `project.media.redacted` or `project.media.purged`
domain event. Redaction SHALL set the shared asset/Project attachments to
`redacted` and block serving the original bytes through the Project media route;
the legacy Task media route retains its existing behavior while its Task
attachment remains active. Purge SHALL set them to `purged` and remove the
stored bytes, so neither former URL serves the bytes. Either disposition SHALL
project affected release pins as `evidence_unavailable` without rewriting an
immutable release manifest. A stale asset version SHALL fail with a typed
conflict, and a different request replay under the same idempotency key SHALL
fail with an idempotency conflict.

#### Scenario: Authorized Project owner redacts media

- **GIVEN** an owner or admin has access to an available Project media asset
- **WHEN** the owner submits the redaction route with matching authorization,
  current version, idempotency key, and reason
- **THEN** Forge records the redacted disposition and audit tombstone
- **AND** the asset and affected release evidence are no longer served as
  available bytes, while the original release manifest remains immutable

#### Scenario: Project member cannot purge media

- **WHEN** a non-admin Project member submits the purge route
- **THEN** Forge rejects the mutation without exposing storage metadata or
  changing the asset, attachments, pins, or bytes

#### Scenario: Authorized purge overlays a release pin

- **GIVEN** an immutable release pins the Project media asset
- **WHEN** an authorized owner/admin submits the purge route
- **THEN** Forge records the `purged` asset tombstone and `project.media.purged`
  event, removes the stored bytes, and projects the pin as
  `evidence_unavailable`
- **AND** neither the former Task URL nor Project media URL serves the bytes

#### Scenario: Media disposition replay and stale version

- **WHEN** the same disposition request is replayed with the same idempotency
  key and fingerprint
- **THEN** Forge returns the original committed media disposition
- **AND WHEN** the asset version is stale or the same key has different content
- **THEN** Forge returns the typed version or idempotency conflict and performs
  no second disposition
