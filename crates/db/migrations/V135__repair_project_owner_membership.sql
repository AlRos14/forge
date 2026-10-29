-- INV-040: Project.owner_id is the canonical owner and must be represented by
-- the exact matching project_member row with role 'owner'.
--
-- This is a forward-only, data-preserving repair. It does not invent an owner
-- for ownerless Projects, and it does not change which account is canonical.
-- An account that already had canonical owner authority keeps the same
-- authority; only its membership representation is made consistent. A dangling
-- owner_id whose user row no longer exists is left unchanged and fails closed
-- rather than aborting startup on the project_member foreign key.

INSERT INTO project_member (id, project_id, user_id, role, created_at, updated_at)
SELECT 'pm-owner-' || p.id, p.id, p.owner_id, 'owner', p.created_at, p.updated_at
FROM project AS p
WHERE p.owner_id IS NOT NULL
  AND EXISTS (
      SELECT 1
      FROM user AS u
      WHERE u.id = p.owner_id
  )
  AND NOT EXISTS (
      SELECT 1
      FROM project_member AS pm
      WHERE pm.project_id = p.id
        AND pm.user_id = p.owner_id
  );

-- If a dangling canonical owner still has another current owner membership,
-- transfer canonical identity to that existing owner. This preserves Project
-- administration without inventing a user or granting new authority.
UPDATE project
SET owner_id = (
    SELECT pm.user_id
    FROM project_member AS pm
    WHERE pm.project_id = project.id
      AND pm.role = 'owner'
    ORDER BY pm.created_at, pm.id
    LIMIT 1
)
WHERE owner_id IS NOT NULL
  AND NOT EXISTS (
      SELECT 1 FROM user AS u WHERE u.id = project.owner_id
  )
  AND EXISTS (
      SELECT 1 FROM project_member AS pm
      WHERE pm.project_id = project.id
        AND pm.role = 'owner'
  );

UPDATE project_member
SET role = 'owner',
    updated_at = (
        SELECT p.updated_at
        FROM project AS p
        WHERE p.id = project_member.project_id
    )
WHERE role <> 'owner'
  AND EXISTS (
      SELECT 1
      FROM project AS p
      WHERE p.id = project_member.project_id
        AND p.owner_id = project_member.user_id
  );
