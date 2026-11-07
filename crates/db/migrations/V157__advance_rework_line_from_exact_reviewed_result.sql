-- Independent reviewers can report on the same immutable result. A later
-- report must not prevent an earlier request_changes report from opening its
-- correction round. Match the exact current result, not the latest report ID.
DROP TRIGGER work_unit_review_line_advance_on_rework;

CREATE TRIGGER work_unit_review_line_advance_on_rework
AFTER INSERT ON task_rework_receipt
BEGIN
    UPDATE work_unit_review_line
       SET current_work_unit_id = NEW.correction_work_unit_id,
           current_work_unit_version = (SELECT version FROM work_unit WHERE id = NEW.correction_work_unit_id),
           current_result_execution_id = NULL,
           current_result_artifact_id = NULL,
           current_commit_sha = NULL,
           current_workspace_id = NULL,
           current_snapshot_digest = NULL,
           current_manifest_id = NULL,
           latest_report_artifact_id = NULL,
           latest_verdict = NULL,
           revision = revision + 1, updated_at = NEW.created_at
     WHERE task_id = NEW.task_id
       AND current_work_unit_id = NEW.reviewed_work_unit_id
       AND current_work_unit_version = NEW.reviewed_work_unit_version
       AND EXISTS (
           SELECT 1
           FROM artifact_execution_producer producer
           JOIN execution review_execution
             ON review_execution.id = producer.execution_id
            AND review_execution.task_id = producer.task_id
           JOIN work_unit_review_subject subject
             ON subject.review_work_unit_id = review_execution.work_unit_id
            AND subject.task_id = review_execution.task_id
           WHERE producer.artifact_id = NEW.review_report_artifact_id
             AND producer.task_id = NEW.task_id
             AND subject.reviewed_work_unit_id = work_unit_review_line.current_work_unit_id
             AND subject.reviewed_work_unit_version = work_unit_review_line.current_work_unit_version
             AND subject.result_execution_id = work_unit_review_line.current_result_execution_id
             AND subject.result_artifact_id IS work_unit_review_line.current_result_artifact_id
             AND subject.result_commit_sha IS work_unit_review_line.current_commit_sha
             AND subject.result_workspace_id IS work_unit_review_line.current_workspace_id
             AND subject.workspace_snapshot_digest IS work_unit_review_line.current_snapshot_digest
       );
END;
