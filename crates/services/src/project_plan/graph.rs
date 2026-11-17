use std::collections::{BTreeMap, BTreeSet};

use sqlx::Row;

use super::{
    decode, ProjectPlanBatch, ProjectPlanDocument, ProjectPlanEdge, ProjectPlanForecast,
    ProjectPlanGraph, ProjectPlanNode, ProjectPlanNodeView, ProjectPlanProposal,
    ProjectPlanService,
};
use crate::{Result, ServiceError};

#[derive(Debug)]
struct TaskProjection {
    id: String,
    title: String,
    lifecycle: String,
    dispatch: String,
    running: i64,
    plan_apply_incomplete: bool,
}

impl ProjectPlanService {
    /// Return a durable projection of the optional project DAG. When no plan
    /// has been applied, ordinary Project Tasks still appear as independent
    /// nodes so a Project can run without a formal DAG.
    pub async fn graph(&self, project_id: &str) -> Result<ProjectPlanGraph> {
        let row = sqlx::query("SELECT version,document_json FROM project_plan WHERE project_id=?")
            .bind(project_id)
            .fetch_optional(self.db.pool())
            .await?
            .ok_or_else(|| ServiceError::not_found("project", project_id.to_owned()))?;
        let version: i64 = row.try_get("version")?;
        let document: ProjectPlanDocument = decode(row.try_get("document_json")?)?;

        let task_rows = sqlx::query(
            "SELECT t.id,t.title,COALESCE(l.state,'backlog') AS lifecycle_state,
                    COALESCE(d.state,'idle') AS dispatch_state,
                    (SELECT COUNT(*) FROM execution e WHERE e.task_id=t.id AND e.status='running') AS running_executions,
                    EXISTS(SELECT 1 FROM project_plan_import_task i
                     WHERE i.task_id=t.id AND NOT EXISTS(
                      SELECT 1 FROM project_plan_proposal_task pt JOIN project_plan_proposal p ON p.id=pt.proposal_id
                      WHERE pt.project_id=i.project_id AND pt.task_id=i.task_id AND pt.import_key=i.import_key
                       AND p.state='applied' AND p.applied_version IS NOT NULL
                       AND i.import_key='proposal:'||p.id||':node:'||pt.node_key
                     )) AS plan_apply_incomplete
             FROM task t
             LEFT JOIN task_lifecycle l ON l.task_id=t.id
             LEFT JOIN task_dispatch d ON d.task_id=t.id
             WHERE t.project_id=? AND t.deleted_at IS NULL
             ORDER BY t.created_at,t.id",
        )
        .bind(project_id)
        .fetch_all(self.db.pool())
        .await?;
        let tasks = task_rows
            .into_iter()
            .map(|row| {
                Ok(TaskProjection {
                    id: row.try_get("id")?,
                    title: row.try_get("title")?,
                    lifecycle: row.try_get("lifecycle_state")?,
                    dispatch: row.try_get("dispatch_state")?,
                    running: row.try_get("running_executions")?,
                    plan_apply_incomplete: row.try_get("plan_apply_incomplete")?,
                })
            })
            .collect::<std::result::Result<Vec<_>, sqlx::Error>>()?;
        let task_ids = tasks
            .iter()
            .map(|task| task.id.clone())
            .collect::<BTreeSet<_>>();

        let mut nodes_by_task = BTreeMap::<String, ProjectPlanNode>::new();
        for node in &document.nodes {
            if let Some(task_id) = node.task_id.as_ref().filter(|id| task_ids.contains(*id)) {
                nodes_by_task.insert(task_id.clone(), node.clone());
            }
        }
        let mut key_by_task = BTreeMap::<String, String>::new();
        let existing_keys = document
            .nodes
            .iter()
            .map(|node| node.key.clone())
            .collect::<BTreeSet<_>>();
        for task in &tasks {
            let key = if let Some(node) = nodes_by_task.get(&task.id) {
                node.key.clone()
            } else {
                let mut candidate = format!("@task:{}", task.id);
                while existing_keys.contains(&candidate)
                    || key_by_task.values().any(|key| key == &candidate)
                {
                    candidate.push('_');
                }
                candidate
            };
            key_by_task.insert(task.id.clone(), key);
        }

        let edge_rows = sqlx::query(
            "SELECT d.task_id,d.depends_on_id
             FROM task_dependency d
             JOIN task successor ON successor.id=d.task_id AND successor.project_id=? AND successor.deleted_at IS NULL
             JOIN task predecessor ON predecessor.id=d.depends_on_id AND predecessor.project_id=successor.project_id AND predecessor.deleted_at IS NULL
             ORDER BY d.depends_on_id,d.task_id",
        )
        .bind(project_id)
        .fetch_all(self.db.pool())
        .await?;
        let mut edge_metadata = BTreeMap::<(String, String), ProjectPlanEdge>::new();
        let task_by_key = document
            .nodes
            .iter()
            .filter_map(|node| {
                node.task_id
                    .as_ref()
                    .map(|task_id| (node.key.clone(), task_id.clone()))
            })
            .collect::<BTreeMap<_, _>>();
        for edge in &document.edges {
            if let (Some(predecessor), Some(successor)) = (
                task_by_key.get(&edge.predecessor_key),
                task_by_key.get(&edge.successor_key),
            ) {
                edge_metadata.insert((predecessor.clone(), successor.clone()), edge.clone());
            }
        }
        let mut edges = Vec::with_capacity(edge_rows.len());
        let mut predecessors = BTreeMap::<String, Vec<String>>::new();
        let mut successors = BTreeMap::<String, Vec<String>>::new();
        let mut indegree = tasks
            .iter()
            .map(|task| (task.id.clone(), 0usize))
            .collect::<BTreeMap<_, _>>();
        for row in edge_rows {
            let successor: String = row.try_get("task_id")?;
            let predecessor: String = row.try_get("depends_on_id")?;
            let predecessor_key = key_by_task.get(&predecessor).cloned().ok_or_else(|| {
                ServiceError::invalid_operation(
                    "Project dependency predecessor is outside the graph projection",
                )
            })?;
            let successor_key = key_by_task.get(&successor).cloned().ok_or_else(|| {
                ServiceError::invalid_operation(
                    "Project dependency successor is outside the graph projection",
                )
            })?;
            edges.push(
                edge_metadata
                    .get(&(predecessor.clone(), successor.clone()))
                    .cloned()
                    .unwrap_or(ProjectPlanEdge {
                        predecessor_key: predecessor_key.clone(),
                        successor_key: successor_key.clone(),
                        inferred: false,
                        source_ids: Vec::new(),
                    }),
            );
            predecessors
                .entry(successor.clone())
                .or_default()
                .push(predecessor.clone());
            successors
                .entry(predecessor)
                .or_default()
                .push(successor.clone());
            *indegree.entry(successor).or_default() += 1;
        }

        let readiness_rows = sqlx::query(
            "SELECT task_id,depends_on_id FROM project_task_dependency_readiness WHERE task_id IN (SELECT id FROM task WHERE project_id=? AND deleted_at IS NULL) AND satisfied=0 ORDER BY depends_on_id",
        )
        .bind(project_id)
        .fetch_all(self.db.pool())
        .await?;
        let mut unsatisfied = BTreeMap::<String, Vec<String>>::new();
        for row in readiness_rows {
            unsatisfied
                .entry(row.try_get("task_id")?)
                .or_default()
                .push(row.try_get("depends_on_id")?);
        }
        let batch_blockers = sqlx::query(
            "SELECT bt.task_id,bt.blocker FROM project_plan_batch_task bt JOIN project_plan_batch b ON b.id=bt.batch_id WHERE b.project_id=? AND b.state IN ('running','paused') AND bt.state='blocked' AND bt.blocker IS NOT NULL ORDER BY b.created_at DESC",
        )
        .bind(project_id)
        .fetch_all(self.db.pool())
        .await?;
        let mut blocked = BTreeMap::<String, Vec<String>>::new();
        for row in batch_blockers {
            let task_id: String = row.try_get("task_id")?;
            let blocker: String = row.try_get("blocker")?;
            let entries = blocked.entry(task_id).or_default();
            if !entries.contains(&blocker) {
                entries.push(blocker);
            }
        }

        let graph_indegree = indegree.clone();
        let mut level_by_task = BTreeMap::<String, usize>::new();
        let mut queue = indegree
            .iter()
            .filter_map(|(task_id, degree)| (*degree == 0).then_some(task_id.clone()))
            .collect::<BTreeSet<_>>();
        while let Some(task_id) = queue.pop_first() {
            let level = *level_by_task.get(&task_id).unwrap_or(&0);
            for successor in successors.get(&task_id).into_iter().flatten() {
                level_by_task
                    .entry(successor.clone())
                    .and_modify(|value| *value = (*value).max(level + 1))
                    .or_insert(level + 1);
                if let Some(degree) = indegree.get_mut(successor) {
                    *degree = degree.saturating_sub(1);
                    if *degree == 0 {
                        queue.insert(successor.clone());
                    }
                }
            }
        }

        let mut nodes = Vec::with_capacity(tasks.len());
        for task in &tasks {
            let plan_node = nodes_by_task.get(&task.id);
            let mut blockers = unsatisfied
                .get(&task.id)
                .into_iter()
                .flatten()
                .map(|id| format!("dependency:{id}"))
                .collect::<Vec<_>>();
            if !matches!(task.lifecycle.as_str(), "backlog" | "ready" | "active") {
                blockers.push(format!("lifecycle:{}", task.lifecycle));
            }
            if task.plan_apply_incomplete {
                blockers.push("plan:apply_incomplete".into());
            }
            if let Some(extra) = blocked.get(&task.id) {
                blockers.extend(extra.iter().cloned());
            }
            blockers.sort();
            blockers.dedup();
            nodes.push(ProjectPlanNodeView {
                key: key_by_task[&task.id].clone(),
                task_id: task.id.clone(),
                title: plan_node
                    .map(|node| node.title.clone())
                    .unwrap_or_else(|| task.title.clone()),
                lifecycle_state: task.lifecycle.clone(),
                dispatch_state: task.dispatch.clone(),
                readiness: if blockers.is_empty() {
                    "ready".into()
                } else {
                    "blocked".into()
                },
                blockers,
                unsatisfied_task_ids: unsatisfied.get(&task.id).cloned().unwrap_or_default(),
                parallel_level: *level_by_task.get(&task.id).unwrap_or(&0),
                running_executions: task.running,
                estimate_min_minutes: plan_node.and_then(|node| node.estimate_min_minutes),
                estimate_max_minutes: plan_node.and_then(|node| node.estimate_max_minutes),
                phase: plan_node.and_then(|node| node.phase.clone()),
                source_ids: plan_node
                    .map(|node| node.source_ids.clone())
                    .unwrap_or_default(),
            });
        }

        let proposal_rows = sqlx::query("SELECT * FROM project_plan_proposal WHERE project_id=? ORDER BY created_at DESC,id DESC LIMIT 100")
            .bind(project_id).fetch_all(self.db.pool()).await?;
        let proposals = proposal_rows
            .into_iter()
            .map(super::proposal_row)
            .collect::<Result<Vec<ProjectPlanProposal>>>()?;
        let batch_rows = sqlx::query("SELECT * FROM project_plan_batch WHERE project_id=? ORDER BY created_at DESC,id DESC LIMIT 100")
            .bind(project_id).fetch_all(self.db.pool()).await?;
        let mut batches = Vec::with_capacity(batch_rows.len());
        for row in batch_rows {
            let id: String = row.try_get("id")?;
            let task_ids = sqlx::query_scalar::<_, String>(
                "SELECT task_id FROM project_plan_batch_task WHERE batch_id=? ORDER BY task_id",
            )
            .bind(&id)
            .fetch_all(self.db.pool())
            .await?;
            batches.push(ProjectPlanBatch {
                id,
                project_id: row.try_get("project_id")?,
                graph_version: row.try_get("graph_version")?,
                version: row.try_get("version")?,
                state: row.try_get("state")?,
                task_ids,
                max_parallel_tasks: row.try_get("max_parallel_tasks")?,
                authorized_by: row.try_get("authorized_by")?,
                created_at: row.try_get("created_at")?,
                updated_at: row.try_get("updated_at")?,
            });
        }

        let forecast = forecast(&nodes, &predecessors, &successors, &graph_indegree);
        Ok(ProjectPlanGraph {
            project_id: project_id.to_owned(),
            version,
            nodes,
            edges,
            sources: document.sources,
            proposals,
            batches,
            forecast,
        })
    }
}

fn forecast(
    nodes: &[ProjectPlanNodeView],
    predecessors: &BTreeMap<String, Vec<String>>,
    successors: &BTreeMap<String, Vec<String>>,
    remaining_indegree: &BTreeMap<String, usize>,
) -> ProjectPlanForecast {
    if nodes.is_empty() {
        return ProjectPlanForecast {
            critical_path_task_ids: Vec::new(),
            min_minutes: None,
            max_minutes: None,
            incomplete_estimates: false,
            basis: "empty_graph".into(),
        };
    }
    if nodes
        .iter()
        .any(|node| node.estimate_min_minutes.is_none() || node.estimate_max_minutes.is_none())
    {
        return ProjectPlanForecast {
            critical_path_task_ids: Vec::new(),
            min_minutes: None,
            max_minutes: None,
            incomplete_estimates: true,
            basis: "estimates_incomplete".into(),
        };
    }
    let mut indegree = remaining_indegree.clone();
    let mut queue = indegree
        .iter()
        .filter_map(|(task_id, degree)| (*degree == 0).then_some(task_id.clone()))
        .collect::<BTreeSet<_>>();
    let by_id = nodes
        .iter()
        .map(|node| (node.task_id.clone(), node))
        .collect::<BTreeMap<_, _>>();
    let mut best_max = BTreeMap::<String, i64>::new();
    let mut best_min = BTreeMap::<String, i64>::new();
    let mut prior = BTreeMap::<String, String>::new();
    while let Some(task_id) = queue.pop_first() {
        let node = by_id[&task_id];
        let own_min = node.estimate_min_minutes.unwrap_or(0);
        let own_max = node.estimate_max_minutes.unwrap_or(0);
        let (parent_min, parent_max) = predecessors
            .get(&task_id)
            .into_iter()
            .flatten()
            .filter_map(|id| {
                best_max
                    .get(id)
                    .map(|max| (id, best_min.get(id).copied().unwrap_or(0), *max))
            })
            .max_by(|left, right| left.2.cmp(&right.2).then_with(|| right.0.cmp(left.0)))
            .map(|(id, min, max)| {
                prior.insert(task_id.clone(), id.clone());
                (min, max)
            })
            .unwrap_or((0, 0));
        let Some(path_min) = parent_min.checked_add(own_min) else {
            return incomplete_overflow();
        };
        let Some(path_max) = parent_max.checked_add(own_max) else {
            return incomplete_overflow();
        };
        best_min.insert(task_id.clone(), path_min);
        best_max.insert(task_id.clone(), path_max);
        for next in successors.get(&task_id).into_iter().flatten() {
            if let Some(degree) = indegree.get_mut(next) {
                *degree = degree.saturating_sub(1);
                if *degree == 0 {
                    queue.insert(next.clone());
                }
            }
        }
    }
    let Some((last, max)) = best_max
        .iter()
        .max_by(|left, right| left.1.cmp(right.1).then_with(|| right.0.cmp(left.0)))
    else {
        return incomplete_overflow();
    };
    let min = best_min.get(last).copied().unwrap_or(0);
    let mut path = vec![last.clone()];
    let mut cursor = last.clone();
    while let Some(parent) = prior.get(&cursor) {
        path.push(parent.clone());
        cursor = parent.clone();
    }
    path.reverse();
    ProjectPlanForecast {
        critical_path_task_ids: path,
        min_minutes: Some(min),
        max_minutes: Some(*max),
        incomplete_estimates: false,
        basis: "complete_task_estimates".into(),
    }
}

fn incomplete_overflow() -> ProjectPlanForecast {
    ProjectPlanForecast {
        critical_path_task_ids: Vec::new(),
        min_minutes: None,
        max_minutes: None,
        incomplete_estimates: true,
        basis: "estimate_overflow_or_invalid_graph".into(),
    }
}
