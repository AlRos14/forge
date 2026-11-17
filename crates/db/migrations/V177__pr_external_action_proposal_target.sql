-- P3 (plan «Congelado 7», D5): a human_approval PR external action is
-- authorized by a collaborative Proposal whose target is the action itself.
-- V173 already requires `target_kind = 'pr_external_action'` on the attached
-- Proposal, but the V091 insert guard still only admits task, execution,
-- workspace and work_unit targets, so such a Proposal could never be stored.
-- Forward-only: the guard is recreated with the same body plus one branch.
DROP TRIGGER proposal_actor_target_guard_insert;
CREATE TRIGGER proposal_actor_target_guard_insert
BEFORE INSERT ON proposal
WHEN NEW.target_kind NOT IN ('task', 'execution', 'workspace', 'work_unit', 'pr_external_action')
  OR (NEW.proposer_actor_kind = 'human' AND NOT EXISTS (
          SELECT 1 FROM user u WHERE u.id = NEW.proposer_actor_id
      ))
  OR (NEW.proposer_actor_kind = 'agent' AND NOT EXISTS (
          SELECT 1 FROM agent_identity ai WHERE ai.id = NEW.proposer_actor_id
      ))
  OR (NEW.target_kind = 'task' AND NEW.target_id != NEW.task_id)
  OR (NEW.target_kind = 'execution' AND NOT EXISTS (
          SELECT 1 FROM execution e WHERE e.id = NEW.target_id AND e.task_id = NEW.task_id
      ))
  OR (NEW.target_kind = 'workspace' AND NOT EXISTS (
          SELECT 1 FROM workspace w WHERE w.id = NEW.target_id AND w.task_id = NEW.task_id
      ))
  OR (NEW.target_kind = 'work_unit' AND NOT EXISTS (
          SELECT 1 FROM work_unit w WHERE w.id = NEW.target_id AND w.task_id = NEW.task_id
      ))
  OR (NEW.target_kind = 'pr_external_action' AND NOT EXISTS (
          SELECT 1 FROM pr_external_action a
          WHERE a.id = NEW.target_id AND a.task_id = NEW.task_id AND a.state = 'proposed'
      ))
  OR (NEW.supersedes_proposal_id IS NOT NULL AND NOT EXISTS (
          SELECT 1 FROM proposal p
          WHERE p.id = NEW.supersedes_proposal_id
            AND p.task_id = NEW.task_id AND p.status = 'superseded'
      ))
BEGIN
    SELECT RAISE(ABORT, 'Proposal ActorRef, target, or supersedes scope is invalid');
END;
