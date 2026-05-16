DELETE FROM runtime
WHERE rowid NOT IN (
    SELECT (
        SELECT latest.rowid
        FROM runtime AS latest
        WHERE latest.daemon_id = grouped.daemon_id
          AND latest.kind = grouped.kind
        ORDER BY latest.updated_at DESC, latest.created_at DESC, latest.id DESC
        LIMIT 1
    )
    FROM runtime AS grouped
);

CREATE UNIQUE INDEX IF NOT EXISTS idx_runtime_daemon_kind ON runtime(daemon_id, kind);
