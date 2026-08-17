-- A turn that fails before reaching a terminal checkpoint can leave orphan
-- LCM entries the canonical history has since disowned.  The runtime now
-- reconciles such divergence by truncating the provisional tail before
-- re-appending canonical history.  Deletes stay blocked for any entry a
-- summary node's source range reaches (DAG provenance is immutable); only
-- node-free tail entries may be removed.
DROP TRIGGER agent_lcm_entry_immutable_delete;

CREATE TRIGGER agent_lcm_entry_truncate_guard
BEFORE DELETE ON agent_lcm_entry
BEGIN
    SELECT RAISE(ABORT, 'LCM entries covered by summary nodes are immutable')
    WHERE EXISTS (
        SELECT 1 FROM agent_lcm_node
        WHERE agent_lcm_node.timeline_id = OLD.timeline_id
          AND agent_lcm_node.range_end >= OLD.sequence
    );
END;
