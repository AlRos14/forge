# Plan — Soporte multirepo por Task (0.9.2)

Fecha: 2026-11-18. Baseline inspeccionado: `feat/multirepo` en `22a02b98`
(0.9.1, esquema hasta V180). Este documento es un plan: no declara ninguna capacidad como
implementada ni aceptada. Ninguna evidencia de este plan se ha obtenido
compilando o ejecutando tests localmente; la validación será solo en CI.

**Estado:** M0 DONE (estático) — 2026-11-18.

## Objetivo

Que Forge ejecute la metodología de *work packets* de FirePOS sin adaptarla:
un packet declara un repo principal, varios repos afectados y contratos que viven
en otro repo, y un único implementador (con un revisor independiente) entrega
el cambio en todos esos repos a la vez.

```yaml
# governance/work-packets/FP-2026-004.yaml (firepos)
primary_repository: firepos-kiosk-frontend
affected_repositories: [firepos-kiosk-frontend, firepos-kiosk-android]
contracts: [firepos-api/0.29.0-design]      # vive en el repo firepos
roles:
  implementer_agent: ...
  reviewer_agent: unassigned-independent-agent
  human_approver: firepos-human-orchestrator
```

Correspondencia objetivo:

| Work packet | Forge |
|---|---|
| packet | Task |
| `primary_repository` | repo `primary` de la Task (hoy `task.repo_id`) |
| `affected_repositories` | repos `affected` de la Task (nuevo) |
| `contracts`, documentación | repos `context` de la Task, solo lectura (nuevo) |
| `implementer_agent` | WorkUnit de implementación con un workspace multirepo |
| `reviewer_agent` | revisión formal sobre el diff de todos los repos escritos |
| `acceptance.commands` | checks del catálogo por `(repo, check_key)` |
| `human_approver` | Gate + decisión Human; merge Human por PR |

## No objetivos

- Integración con Trello u otros tableros (diseño aparte, posterior).
- Importar work packets YAML como Tasks (posible seguimiento; no en 0.9.2).
- Merge automático. El merge sigue siendo decisión Human, una PR por repo.
- Transacciones atómicas entre repos remotos. GitHub no las ofrece; el plan
  hace visible y recuperable el estado parcial, no lo oculta.
- Submódulos git o monorepos sintéticos.
- Cambiar el contrato de Tasks de un solo repo: una Task sin repos adicionales
  se comporta exactamente como en 0.9.1.

## Hallazgo sobre la sesión de referencia

La sesión claude2 `514d06d8` (cwd `~/Proyectos/firepos`) solo escribió en el repo
`firepos` (contratos OpenAPI, validador, work packets) y en
`firepos-payment-arch`, que es un `git worktree` de `firepos`, no otro repo. El
multirepo real está en la metodología (`affected_repositories`), no en esa
sesión. Este plan apunta a la metodología.

## Baseline verificado en el código

1. **Repo de la Task.** `task.repo_id` es un único valor escalar nullable, no
   una restricción SQL `UNIQUE`; Tasks sin repo son válidas. `TaskService::create_task`
   lo toma del padre o de `project.primary_repo_id`
   (`crates/services/src/task_service/create.rs:109-122`). `CreateTaskRequest`
   no tiene campo de repo.
2. **Repo principal del proyecto.** Lo fija el primer repo añadido
   (`crates/db/src/sqlite/repo.rs:30-80`) y no se puede cambiar por API
   (`UpdateProjectRequest` no lo expone).
3. **Workspaces.** `workspace.repo_id` ya es por fila y `workspace_scope`
   distingue `integration` de `work_unit`. Estructuralmente una Task puede tener
   varios workspaces de repos distintos, pero
   `prepare_integration_workspace_for_work_unit`
   (`crates/services/src/task_service/workspace.rs:9`) exige
   `workspace.repo_id == task.repo_id`.
4. **Leases.** `workspace_lease.workspace_id` es nullable y no es `UNIQUE`;
   `repository_binding_id` es `TEXT` no vacío, no FK. La exclusividad se fija
   por WorkUnit (`idx_workspace_lease_active_work_unit`) y por Task solo para
   el lease legacy activo; no existe “un lease por workspace”. Los guards
   actuales comparan el binding con `task.repo_id` y deben pasar a validar el
   repo de la asociación Workspace/Task.
5. **Resultado de Execution.** `execution.workspace_id`, `before_sha` y
   `after_sha` son valores escalares por fila, no restricciones de unicidad.
   También son escalares `work_unit_review_subject.result_commit_sha`,
   `result_workspace_id`, `workspace_snapshot_digest` y sus equivalentes de
   `work_unit_result_requirement`; no hay columnas hijas por repo hoy.
6. **Integración.** `work_unit_integration` guarda WorkUnit, Execution,
   workspace origen/destino y SHA exactos. Su unicidad es
   `(task_id, operation_idempotency_key)`, no una fila única por par de
   workspaces ni por repo.
7. **PR.** `pr_metadata.task_id` es `UNIQUE` hoy. `remote_pr_admission` tiene
   PK `task_merge_operation_id`, `publish_operation_id UNIQUE` e índice
   `idx_remote_pr_admission_reconcile(state, updated_at, task_merge_operation_id)`;
   además conserva `remote_repo_identity`. Los guards vigentes de V109/V112/
   V113/V114/V173/V175 atan admisión y configuración al único `task.repo_id`.
   `pr_provider_config` y su política de feedback sí se guardan por repo.
8. **Validación.** `validation_check_definition.repo_id` y
   `validation_run.workspace_id/commit_sha` ya son por repo.
9. **Dependencias del plan de proyecto.** El handoff durable se serializa en
   `project_plan_handoff.context_json` con digest; el contexto incluye repo y
   commit integrado del predecesor. La validación actual compara un repo y
   ancestry dentro de ese repo; no tiene todavía una colección de handoffs.
10. **Replanes.** `replan_classifier` compara los escalares `repo_id` y
    `current_repo_id`; cualquier conjunto de repos requiere extender su entrada,
    digest y hechos de clasificación.

## Invariantes tocados

- **INV-016 (WorkUnit es alcance de ejecución):** el alcance incluye ahora un
  conjunto exacto de repos, fijado antes de la Execution.
- **INV-017 (escritores paralelos aislados):** un workspace multirepo es un
  conjunto de worktrees aislados; dos WorkUnits que escriban en el mismo repo
  siguen teniendo worktrees distintos.
- **INV-028/INV-030 (validación determinista):** cada `ValidationRun` sigue
  ligado a un workspace y commit exactos; el Gate exige el conjunto completo.
- **INV-032 (lo no soportado no es silencioso):** escribir en un repo `context`
  o fuera del conjunto falla cerrado, con motivo persistido.
- **INV-034 (sin fuentes de verdad ocultas):** el conjunto de repos vive en una
  tabla; `task.repo_id` queda como espejo del `primary`, comprobado por
  restricción, no como segunda fuente.
- Autoridad: añadir o quitar un repo `affected` es un cambio de alcance y
  requiere la decisión Human exacta (contrato del envelope v2).

## Modelo

### Repos de una Task

Tabla nueva `task_repository(task_id, repo_id, role, position, created_at)`, con
`role ∈ {primary, affected, context}`:

- exactamente un `primary` por Task con repo; `task.repo_id` debe coincidir;
- el conjunto vacío representa una Task repo-less válida (`task.repo_id=NULL`);
- `primary` es también escribible (se trata como `affected`);
- todos los repos pertenecen al Project de la Task;
- un repo no puede tener dos roles en la misma Task;
- `context` nunca recibe escrituras, integraciones ni PR.

Las Tasks existentes se rellenan con una fila `primary` desde `task.repo_id`.
Las subtareas heredan el conjunto del padre salvo que la petición indique otro.

### Alcance de repos del WorkUnit

Tabla `work_unit_repository(work_unit_id, repo_id)`: subconjunto de
los repos escribibles de la Task. Si está vacío, el WorkUnit cubre todos.
Permite, si un packet lo necesita, dos implementadores en paralelo (backend y
frontend) dentro de la misma Task sin compartir worktrees. Por defecto es un
implementador para todos los repos, como la plantilla de packets.

### Workspace multirepo

Para un WorkUnit con N repos escribibles y M repos de contexto:

```
<workspace_root>/<task>/<work_unit>/
  firepos-kiosk-frontend/   worktree, rama agent/<task>-<wu>
  firepos-kiosk-android/    worktree, misma rama
  firepos/                  worktree de contexto, commit fijado, solo lectura
  FORGE_WORKSPACE.md        índice generado: rol de cada carpeta y reglas
```

- Una fila `workspace` (y su `workspace_scope`) por repo, todas ligadas al
  WorkUnit; un grupo nuevo `workspace_group(id, task_id, work_unit_id,
  root_path)` les da una raíz común.
- Un lease por workspace escribible; el lease de contexto es de solo lectura.
- El harness arranca en la raíz del grupo. Así ve todos los repos como
  carpetas hermanas, igual que una sesión de Claude o Codex en `~/Proyectos`.
- Los repos de contexto se preparan en el commit fijado al crear la Task (o al
  aceptar un replan), con permisos de solo lectura en disco.
- `FORGE_WORKSPACE.md` (y el `AGENTS.md`/`CLAUDE.md` de raíz que lee cada
  harness) se generan fuera de los repos y no se versionan.

### Resultado de la Execution

Tabla nueva `execution_repository_result(execution_id, repo_id, workspace_id,
before_sha, after_sha, snapshot_digest)`. Las columnas actuales de
`execution` siguen reflejando el repo `primary`, así que las lecturas de un solo
repo no cambian.

Al terminar la Execution, Forge:

1. registra el commit de cada repo escribible (sin cambios es un resultado
   válido: `after_sha = before_sha`);
2. falla cerrado si un repo `context` tiene cambios o si hay repos git nuevos
   fuera del conjunto en la raíz.

`work_unit_review_subject` y `work_unit_result_requirement` ganan una tabla
hija por repo con el mismo patrón; las columnas actuales quedan para el
`primary`.

### Revisión

El revisor recibe un workspace de grupo con los commits de resultado de cada
repo y el diff por repo. El ReviewReport sigue siendo uno; los hallazgos pueden
llevar ruta prefijada por repo. Micro-fix, fallback y rework operan sobre el
mismo grupo.

### Validación y Gate

- Los checks se piden por `(repo, check_key)`; cada `ValidationRun` corre en el
  workspace y commit de ese repo.
- El Gate exige los checks requeridos de cada repo escrito, contra los commits
  exactos del resultado. Por DM2, un repo sin cambios no abre PR ni exige
  checks.

### Integración y PR

- Integración por repo, en orden `primary` primero y luego `affected` por
  `position`, cada una como `work_unit_integration` con su propio workspace de
  integración (un workspace `integration` por repo de la Task).
- Si un repo da conflicto, la Task queda bloqueada con el estado por repo. Los
  repos ya integrados no se revierten; la recuperación reintenta solo los
  pendientes.
- Una PR por repo escrito. `pr_metadata` pasa a `UNIQUE(task_id, repo_id)` y
  `remote_pr_admission` gana `repo_id`. Cada PR enlaza a las demás de la misma
  Task en su descripción.
- La Task pasa a `ready_to_merge` cuando todas sus PR están abiertas y
  admitidas, y a `done` cuando todas están mergeadas. Una PR cerrada sin merge
  bloquea la Task.
- El bucle de feedback de PR ya es por PR; el triage puede extender la Task y el
  rework vuelve al mismo grupo de workspaces.

### Dependencias entre Tasks (plan de proyecto)

Para cada repo que el predecesor integró y el sucesor escribe o lee como
contexto, se mantiene la comprobación de ascendencia sobre ese repo. Para los
repos que no comparten, basta con que el predecesor esté aceptado e integrado.
Los repos de contexto del sucesor se fijan en el commit integrado del
predecesor cuando este lo escribió (por ejemplo, el contrato en `firepos`).

### Repo principal del proyecto

`PATCH /projects/{id}` acepta `primary_repo_id` (owner/admin, repo del mismo
Project). Solo afecta a Tasks nuevas.

## Contratos públicos (a congelar en M0)

- `CreateTaskRequest`: `repositories?: [{repo_id, role}]`. Sin el campo, el
  comportamiento es el de 0.9.1.
- `PATCH /tasks/{id}/repositories` con `expected_version`: cambia el conjunto
  antes de la primera Execution; después, solo por replan con decisión Human.
- Respuestas de Task, WorkUnit, Execution, revisión, integración y PR: listas
  `repositories` / `repository_results` por repo.
- WorkUnit: `repository_ids?` al crear.
- `PATCH /projects/{id}`: `primary_repo_id`.
- MCP: los mismos campos en las herramientas de Task y WorkUnit existentes; sin
  herramientas nuevas salvo la de cambio de repos de la Task.
- SSE: los eventos de integración y PR incluyen `repo_id`.
- Web: selector de repos (primary / affected / context) al crear la Task; vista
  de estado por repo en el detalle.
- CHANGELOG `Unreleased` y `docs/api.md`, `docs/concepts/work-units.md`,
  `docs/concepts/review-and-validation.md`, `docs/concepts/pr-feedback-loop.md`.

## Migración

Tres migraciones nuevas, repartidas por fase; solo la reconstrucción de
`pr_metadata` rehace una tabla:

- **V181 / M1** `task_repository`, espejos/guards INV-034, backfill primary,
  `work_unit_repository`.
- **V182 / M2** `workspace_group`, resultados por repo y tablas hijas de Review
  Subject/Result Requirement; sin raíces comunes ficticias para workspaces
  históricos.
- **V183 / M3** batch Task-wide, repo en TaskIntegrationOperation y
  `remote_pr_admission`, rebuild de `pr_metadata` con `UNIQUE(task_id, repo_id)`.

No se edita ninguna migración histórica.

## Fases y puertas de CI

Cada fase en su rama, validada con `build-local-linux.yml run_checks=true`, PR
con CI completo y merge solo con todo en verde.

| Fase | Contenido | Prefijo de tests |
|---|---|---|
| **M0** | Congelar contratos de este documento (DTO, SQL, firmas) tras revisión | — |
| **M1** | Migración, capa `db`, conjunto de repos de la Task y del WorkUnit, `primary_repo_id` editable, API/MCP/web de creación | `multirepo_task_` |
| **M2** | Workspace de grupo, leases por repo, arranque del harness en la raíz, resultado por repo, detección de escrituras en contexto, revisión y validación por repo | `multirepo_workspace_`, `multirepo_result_` |
| **M3** | Integración por repo, PR por repo, Gate, lifecycle `ready_to_merge`/`done`, dependencias entre Tasks, replan de conjunto | `multirepo_integration_`, `multirepo_pr_` |
| **E2E** | Proyecto de prueba con `firepos` + dos `firepos-*` y un packet real, con agentes reales en la release instalada | manual, con evidencia |

Pruebas mínimas:

- una Task de un solo repo produce exactamente las mismas filas y respuestas que
  en 0.9.1 (regresión);
- crear la Task con un repo de otro Project falla;
- escribir en un repo de contexto hace fallar la Execution con motivo;
- conflicto en el segundo repo: el primero queda integrado, la Task bloqueada y
  el reintento solo toca el pendiente;
- Gate con un check fallido en un repo bloquea aunque los demás pasen;
- dos PR, una mergeada y otra abierta: la Task no llega a `done`;
- dependencia entre Tasks con repos compartidos y no compartidos.

## Decisiones para el dueño

- **DM1 — RESUELTA (owner-approved):** un implementador por packet por defecto;
  el alcance de repo del WorkUnit es opcional y permite paralelizar cuando se
  configure expresamente. Sin `repository_ids`, el WorkUnit cubre todos los
  repos escribibles de la Task.
- **DM2 — RESUELTA (owner-approved):** repos sin cambios no abren PR ni exigen
  checks.
- **DM3 — RESUELTA (owner-approved):** integrar `primary` primero y luego los
  repos `affected` por `position` ascendente; ante conflicto, conservar los
  éxitos y reintentar solo los pendientes.

## Riesgos

- **Tamaño.** M2 y M3 tocan workspace, Execution, revisión, integración, PR y
  Gate. Si M3 se alarga, 0.9.2 puede salir con M1–M2 (multirepo en ejecución y
  revisión) y M3 en 0.9.3, documentado como tal.
- **Harness y sandbox.** El sandbox de cada harness limita escrituras a su cwd;
  arrancar en la raíz del grupo cubre todos los repos escribibles. Los repos de
  contexto se protegen además con permisos de disco y con la comprobación
  posterior, porque el sandbox no distingue carpetas.
- **Instrucciones por repo.** Cada repo puede tener su `AGENTS.md`/`CLAUDE.md`;
  el índice de raíz debe indicar al agente que las lea, ya que el harness solo
  carga las de su cwd.

## Contratos congelados (M0)

Congelado sobre `feat/multirepo` (`22a02b98`), fuente 0.9.1 y V180. La prueba
SQL indicada abajo es local y estática; Cargo, builds y tests Rust quedan para
CI. Los archivos citados son el alcance previsto de implementación, no cambios
incluidos en este plan.

Correcciones del baseline ya aplicadas arriba: `task.repo_id` no es UNIQUE;
`workspace_lease.workspace_id` tampoco es UNIQUE y
`repository_binding_id` no tiene FK; la unicidad actual de
`work_unit_integration` es `(task_id, operation_idempotency_key)`; los SHA y
snapshots de `execution`/review son escalares sin unicidad SQL; y el handoff de
Project Plan vive en `context_json` con digest. No apareció otro hallazgo del
baseline que contradiga el código inspeccionado. También se corrigió la
procedencia del guard `workspace_scope_insert_guard`: su definición vigente es
V091, no V142/V170.

### Migraciones y esquema

V181 pertenece a M1, V182 a M2 y V183 a M3. No se editan V001–V180. Este es el
SQL congelado para tablas, índices, guards de espejo y backfills; las
adaptaciones de triggers ya existentes se enumeran tras los bloques.

**V181 — M1: asociación Task/WorkUnit.**

~~~sql
CREATE TABLE task_repository (
    task_id     TEXT NOT NULL REFERENCES task(id) ON DELETE CASCADE,
    repo_id     TEXT NOT NULL REFERENCES repo(id) ON DELETE CASCADE,
    role        TEXT NOT NULL CHECK (role IN ('primary', 'affected', 'context')),
    position    INTEGER NOT NULL CHECK (position >= 0),
    created_at  TEXT NOT NULL,
    PRIMARY KEY (task_id, repo_id),
    UNIQUE (task_id, position),
    CHECK ((role = 'primary' AND position = 0)
        OR (role IN ('affected', 'context') AND position > 0))
);
CREATE UNIQUE INDEX idx_task_repository_one_primary
    ON task_repository(task_id) WHERE role = 'primary';
CREATE INDEX idx_task_repository_repo
    ON task_repository(repo_id, task_id);

CREATE TRIGGER task_repository_insert_guard
BEFORE INSERT ON task_repository
WHEN NOT EXISTS (
    SELECT 1 FROM task t JOIN repo r ON r.id = NEW.repo_id
    WHERE t.id = NEW.task_id AND r.project_id = t.project_id
      AND (NEW.role != 'primary' OR t.repo_id = NEW.repo_id)
)
BEGIN
    SELECT RAISE(ABORT, 'Task repository must belong to its Project and primary must mirror task.repo_id');
END;

CREATE TRIGGER task_repository_update_guard
BEFORE UPDATE ON task_repository
WHEN NEW.task_id IS NOT OLD.task_id OR NEW.repo_id IS NOT OLD.repo_id
  OR NOT EXISTS (
      SELECT 1 FROM task t JOIN repo r ON r.id = NEW.repo_id
      WHERE t.id = NEW.task_id AND r.project_id = t.project_id
        AND (NEW.role != 'primary' OR t.repo_id = NEW.repo_id)
        AND (OLD.role != 'primary' OR NEW.role = 'primary'
             OR t.repo_id IS NOT OLD.repo_id)
  )
BEGIN
    SELECT RAISE(ABORT, 'Task repository identity is immutable and primary must mirror task.repo_id');
END;

CREATE TRIGGER task_repository_delete_guard
BEFORE DELETE ON task_repository
WHEN OLD.role = 'primary'
 AND EXISTS (SELECT 1 FROM task t WHERE t.id = OLD.task_id AND t.repo_id = OLD.repo_id)
 AND NOT EXISTS (
     SELECT 1 FROM task t
     JOIN project_deletion_guard guard ON guard.project_id = t.project_id
     WHERE t.id = OLD.task_id
 )
BEGIN
    SELECT RAISE(ABORT, 'change task.repo_id before removing its primary association');
END;

CREATE TRIGGER task_repository_mirror_after_task_insert
AFTER INSERT ON task
WHEN NEW.repo_id IS NOT NULL
BEGIN
    INSERT INTO task_repository(task_id, repo_id, role, position, created_at)
    VALUES (NEW.id, NEW.repo_id, 'primary', 0, NEW.created_at);
END;

CREATE TRIGGER task_repository_mirror_after_task_repo_update
AFTER UPDATE OF repo_id ON task
WHEN NEW.repo_id IS NOT OLD.repo_id
BEGIN
    UPDATE task_repository
       SET role = 'affected',
           position = (SELECT COALESCE(MAX(position), 0) + 1
                       FROM task_repository WHERE task_id = NEW.id)
     WHERE task_id = NEW.id AND role = 'primary'
       AND repo_id IS NOT NEW.repo_id;
    UPDATE task_repository
       SET role = 'primary', position = 0
     WHERE task_id = NEW.id AND repo_id = NEW.repo_id;
    INSERT INTO task_repository(task_id, repo_id, role, position, created_at)
    SELECT NEW.id, NEW.repo_id, 'primary', 0, NEW.updated_at
     WHERE NEW.repo_id IS NOT NULL
       AND NOT EXISTS (SELECT 1 FROM task_repository
                       WHERE task_id = NEW.id AND repo_id = NEW.repo_id);
END;

CREATE TRIGGER task_primary_repo_project_guard_update
BEFORE UPDATE OF repo_id ON task
WHEN NEW.repo_id IS NOT NULL
 AND NOT EXISTS (
     SELECT 1 FROM repo r
     WHERE r.id = NEW.repo_id AND r.project_id = NEW.project_id
 )
BEGIN
    SELECT RAISE(ABORT, 'Task primary repository must belong to its Project');
END;

INSERT INTO task_repository(task_id, repo_id, role, position, created_at)
SELECT id, repo_id, 'primary', 0, created_at
FROM task
WHERE repo_id IS NOT NULL;

CREATE TABLE work_unit_repository (
    work_unit_id  TEXT NOT NULL,
    task_id       TEXT NOT NULL,
    repo_id       TEXT NOT NULL,
    created_at    TEXT NOT NULL,
    PRIMARY KEY (work_unit_id, repo_id),
    FOREIGN KEY (work_unit_id, task_id)
        REFERENCES work_unit(id, task_id) ON DELETE CASCADE,
    FOREIGN KEY (task_id, repo_id)
        REFERENCES task_repository(task_id, repo_id) ON DELETE RESTRICT
);
CREATE INDEX idx_work_unit_repository_repo
    ON work_unit_repository(task_id, repo_id, work_unit_id);

CREATE TRIGGER work_unit_repository_insert_guard
BEFORE INSERT ON work_unit_repository
WHEN NOT EXISTS (
    SELECT 1
    FROM work_unit w
    JOIN task_repository tr
      ON tr.task_id = w.task_id AND tr.repo_id = NEW.repo_id
    WHERE w.id = NEW.work_unit_id AND w.task_id = NEW.task_id
      AND tr.role IN ('primary', 'affected')
)
 OR EXISTS (SELECT 1 FROM execution e WHERE e.work_unit_id = NEW.work_unit_id)
BEGIN
    SELECT RAISE(ABORT, 'WorkUnit repository scope must be writable and fixed before its first Execution');
END;

CREATE TRIGGER work_unit_repository_immutable_update
BEFORE UPDATE ON work_unit_repository
BEGIN
    SELECT RAISE(ABORT, 'WorkUnit repository scope is immutable');
END;

-- Tasks repo-less remain without a task_repository row.
-- An empty work_unit_repository means all writable Task repos;
-- context repositories cannot be selected.
~~~

Al crear la Task, el trigger AFTER INSERT materializa la fila `primary`; el
servicio inserta solo `affected/context` para no duplicarla. Al reemplazar el
primary, el servicio actualiza `task.repo_id` dentro de la transacción
versionada y los triggers degradan/promueven las filas espejo. El guard de
DELETE exceptúa el Project teardown ya autorizado por `project_deletion_guard`.

**V182 — M2: grupos de workspace y resultados por repo.** Se crean grupos solo
para WorkUnits nuevos; no se inventa una raíz común para workspaces históricos.

~~~sql
CREATE TABLE workspace_group (
    id           TEXT PRIMARY KEY,
    task_id      TEXT NOT NULL REFERENCES task(id) ON DELETE CASCADE,
    work_unit_id TEXT NOT NULL,
    root_path    TEXT NOT NULL CHECK (length(trim(root_path)) > 0),
    created_at   TEXT NOT NULL,
    UNIQUE (work_unit_id),
    UNIQUE (id, task_id),
    FOREIGN KEY (work_unit_id, task_id)
        REFERENCES work_unit(id, task_id) ON DELETE CASCADE
);
ALTER TABLE workspace_scope
    ADD COLUMN workspace_group_id TEXT
        REFERENCES workspace_group(id) ON DELETE RESTRICT;
CREATE INDEX idx_workspace_scope_group
    ON workspace_scope(workspace_group_id, workspace_id)
    WHERE workspace_group_id IS NOT NULL;

CREATE TABLE execution_repository_result (
    execution_id    TEXT NOT NULL REFERENCES execution(id) ON DELETE CASCADE,
    task_id         TEXT NOT NULL REFERENCES task(id) ON DELETE CASCADE,
    repo_id         TEXT NOT NULL REFERENCES repo(id) ON DELETE RESTRICT,
    workspace_id    TEXT NOT NULL REFERENCES workspace(id) ON DELETE RESTRICT,
    before_sha      TEXT,
    after_sha       TEXT,
    snapshot_digest TEXT CHECK (
        snapshot_digest IS NULL OR length(snapshot_digest) = 64
    ),
    created_at      TEXT NOT NULL,
    PRIMARY KEY (execution_id, repo_id),
    UNIQUE (execution_id, workspace_id)
);
CREATE INDEX idx_execution_repository_result_task
    ON execution_repository_result(task_id, execution_id, repo_id);
CREATE TRIGGER execution_repository_result_insert_guard
BEFORE INSERT ON execution_repository_result
WHEN NOT EXISTS (
    SELECT 1
    FROM execution e
    JOIN workspace w
      ON w.id = NEW.workspace_id AND w.task_id = NEW.task_id
    JOIN task_repository tr
      ON tr.task_id = NEW.task_id AND tr.repo_id = NEW.repo_id
     AND tr.role IN ('primary', 'affected')
    WHERE e.id = NEW.execution_id AND e.task_id = NEW.task_id
      AND w.repo_id = NEW.repo_id
      AND (e.work_unit_id IS NULL
           OR NOT EXISTS (SELECT 1 FROM work_unit_repository s
                          WHERE s.work_unit_id = e.work_unit_id)
           OR EXISTS (SELECT 1 FROM work_unit_repository s
                      WHERE s.work_unit_id = e.work_unit_id
                        AND s.repo_id = NEW.repo_id))
)
BEGIN
    SELECT RAISE(ABORT, 'repository result must bind the exact Execution, writable repo, and workspace');
END;

CREATE TABLE work_unit_review_subject_repository (
    review_work_unit_id        TEXT NOT NULL,
    task_id                    TEXT NOT NULL,
    repo_id                    TEXT NOT NULL,
    reviewed_work_unit_id      TEXT NOT NULL,
    reviewed_work_unit_version INTEGER CHECK (
        reviewed_work_unit_version IS NULL OR reviewed_work_unit_version >= 1
    ),
    result_execution_id        TEXT REFERENCES execution(id) ON DELETE RESTRICT,
    result_artifact_id         TEXT REFERENCES artifact(id) ON DELETE RESTRICT,
    result_commit_sha          TEXT,
    result_workspace_id        TEXT REFERENCES workspace(id) ON DELETE RESTRICT,
    workspace_snapshot_digest  TEXT CHECK (
        workspace_snapshot_digest IS NULL OR length(workspace_snapshot_digest) = 64
    ),
    created_at                 TEXT NOT NULL,
    PRIMARY KEY (review_work_unit_id, repo_id),
    FOREIGN KEY (review_work_unit_id, task_id)
        REFERENCES work_unit(id, task_id) ON DELETE CASCADE,
    FOREIGN KEY (reviewed_work_unit_id, task_id)
        REFERENCES work_unit(id, task_id) ON DELETE RESTRICT,
    FOREIGN KEY (task_id, repo_id)
        REFERENCES task_repository(task_id, repo_id) ON DELETE RESTRICT,
    CHECK (review_work_unit_id != reviewed_work_unit_id),
    CHECK (result_execution_id IS NOT NULL OR (
        result_artifact_id IS NULL AND result_commit_sha IS NULL
        AND result_workspace_id IS NULL AND workspace_snapshot_digest IS NULL
    ))
);
CREATE INDEX idx_work_unit_review_subject_repository_reviewed
    ON work_unit_review_subject_repository(
        task_id, reviewed_work_unit_id, reviewed_work_unit_version, repo_id
    );
CREATE TRIGGER work_unit_review_subject_repository_insert_guard
BEFORE INSERT ON work_unit_review_subject_repository
WHEN NOT EXISTS (
    SELECT 1 FROM work_unit review_wu
    JOIN work_unit reviewed_wu
      ON reviewed_wu.id = NEW.reviewed_work_unit_id
     AND reviewed_wu.task_id = NEW.task_id
    JOIN task_repository tr
      ON tr.task_id = NEW.task_id AND tr.repo_id = NEW.repo_id
     AND tr.role IN ('primary', 'affected')
    WHERE review_wu.id = NEW.review_work_unit_id
      AND review_wu.task_id = NEW.task_id
      AND (NEW.result_execution_id IS NULL OR EXISTS (
          SELECT 1 FROM execution e
          WHERE e.id = NEW.result_execution_id AND e.task_id = NEW.task_id
            AND e.work_unit_id = NEW.reviewed_work_unit_id
      ))
      AND (NEW.result_workspace_id IS NULL OR EXISTS (
          SELECT 1 FROM workspace w
          WHERE w.id = NEW.result_workspace_id AND w.task_id = NEW.task_id
            AND w.repo_id = NEW.repo_id
      ))
)
BEGIN
    SELECT RAISE(ABORT, 'review subject repo must bind exact same-Task WorkUnits and result');
END;

CREATE TABLE work_unit_result_requirement_repository (
    work_unit_id               TEXT NOT NULL,
    task_id                    TEXT NOT NULL,
    repo_id                    TEXT NOT NULL,
    source_work_unit_id        TEXT NOT NULL,
    source_work_unit_version   INTEGER NOT NULL CHECK (source_work_unit_version >= 1),
    result_execution_id        TEXT NOT NULL REFERENCES execution(id) ON DELETE RESTRICT,
    result_artifact_id         TEXT REFERENCES artifact(id) ON DELETE RESTRICT,
    result_commit_sha          TEXT,
    result_workspace_id        TEXT REFERENCES workspace(id) ON DELETE RESTRICT,
    workspace_snapshot_digest  TEXT CHECK (
        workspace_snapshot_digest IS NULL OR length(workspace_snapshot_digest) = 64
    ),
    created_at                 TEXT NOT NULL,
    PRIMARY KEY (work_unit_id, repo_id),
    FOREIGN KEY (work_unit_id, task_id)
        REFERENCES work_unit(id, task_id) ON DELETE CASCADE,
    FOREIGN KEY (source_work_unit_id, task_id)
        REFERENCES work_unit(id, task_id) ON DELETE RESTRICT,
    FOREIGN KEY (task_id, repo_id)
        REFERENCES task_repository(task_id, repo_id) ON DELETE RESTRICT,
    CHECK (work_unit_id != source_work_unit_id)
);
CREATE INDEX idx_work_unit_result_requirement_repository_source
    ON work_unit_result_requirement_repository(task_id, source_work_unit_id, repo_id);
CREATE TRIGGER work_unit_result_requirement_repository_insert_guard
BEFORE INSERT ON work_unit_result_requirement_repository
WHEN NOT EXISTS (
    SELECT 1 FROM work_unit wu
    JOIN work_unit source_wu
      ON source_wu.id = NEW.source_work_unit_id AND source_wu.task_id = NEW.task_id
    JOIN task_repository tr
      ON tr.task_id = NEW.task_id AND tr.repo_id = NEW.repo_id
     AND tr.role IN ('primary', 'affected')
    JOIN execution e
      ON e.id = NEW.result_execution_id AND e.task_id = NEW.task_id
     AND e.work_unit_id = NEW.source_work_unit_id
    WHERE wu.id = NEW.work_unit_id AND wu.task_id = NEW.task_id
      AND (NEW.result_workspace_id IS NULL OR EXISTS (
          SELECT 1 FROM workspace w
          WHERE w.id = NEW.result_workspace_id AND w.task_id = NEW.task_id
            AND w.repo_id = NEW.repo_id
      ))
)
BEGIN
    SELECT RAISE(ABORT, 'result requirement repo must bind exact same-Task WorkUnits and result');
END;

-- Backfill only completed exact Executions with a Workspace. Unknown SHA and
-- snapshot values remain NULL; no commit or digest is inferred.
INSERT INTO execution_repository_result(
    execution_id, task_id, repo_id, workspace_id,
    before_sha, after_sha, snapshot_digest, created_at
)
SELECT e.id, e.task_id, w.repo_id, w.id,
       e.before_sha, e.after_sha, NULL, e.updated_at
FROM execution e
JOIN workspace w ON w.id = e.workspace_id AND w.task_id = e.task_id
JOIN task_repository tr
  ON tr.task_id = e.task_id AND tr.repo_id = w.repo_id
 AND tr.role IN ('primary', 'affected')
WHERE e.status = 'completed';

INSERT INTO work_unit_review_subject_repository(
    review_work_unit_id, task_id, repo_id, reviewed_work_unit_id,
    reviewed_work_unit_version, result_execution_id, result_artifact_id,
    result_commit_sha, result_workspace_id, workspace_snapshot_digest, created_at
)
SELECT s.review_work_unit_id, s.task_id, w.repo_id, s.reviewed_work_unit_id,
       s.reviewed_work_unit_version, s.result_execution_id, s.result_artifact_id,
       s.result_commit_sha, s.result_workspace_id, s.workspace_snapshot_digest,
       s.created_at
FROM work_unit_review_subject s
JOIN workspace w ON w.id = s.result_workspace_id AND w.task_id = s.task_id
JOIN task_repository tr
  ON tr.task_id = s.task_id AND tr.repo_id = w.repo_id
 AND tr.role IN ('primary', 'affected');

INSERT INTO work_unit_result_requirement_repository(
    work_unit_id, task_id, repo_id, source_work_unit_id, source_work_unit_version,
    result_execution_id, result_artifact_id, result_commit_sha, result_workspace_id,
    workspace_snapshot_digest, created_at
)
SELECT r.work_unit_id, r.task_id, w.repo_id, r.source_work_unit_id,
       r.source_work_unit_version, r.result_execution_id, r.result_artifact_id,
       r.result_commit_sha, r.result_workspace_id, r.workspace_snapshot_digest,
       r.created_at
FROM work_unit_result_requirement r
JOIN workspace w ON w.id = r.result_workspace_id AND w.task_id = r.task_id
JOIN task_repository tr
  ON tr.task_id = r.task_id AND tr.repo_id = w.repo_id
 AND tr.role IN ('primary', 'affected');
~~~

V182 reemplaza `workspace_scope_insert_guard` (V091; no hay reemplazo suyo en
V142/V170): verifica repo asociado a la Task, solo permite context como
Workspace de solo lectura, y limita WorkUnit a role primary/affected y a su
scope vacío-o-explícito. Reemplaza también `workspace_lease_scope_guard_insert`
y `workspace_lease_active_renewal_guard` (cuerpos vigentes V120), además de
`workspace_lease_work_unit_binding_guard` y
`workspace_lease_work_unit_active_renewal_guard` (V091): cada uno conserva sus
checks actuales de Actor, RoleMembership, TaskRole, Execution, lease,
provider-start, workspace listo y capability digest, pero resuelve repo desde
`workspace.repo_id` + `task_repository` y el scope del WorkUnit, nunca desde
`task.repo_id`. Se conservan `workspace_lease_legacy_scope_guard`,
`workspace_lease_scope_pair_guard`, los guards de binding inmutable y los
guards de inmutabilidad/terminalidad, además de
`workspace_scope_immutable_update`. Se conservan las unicidades de lease
activo por WorkUnit y del lease legacy por Task. Los grupos históricos quedan
sin `workspace_group_id`.

**V183 — M3: operaciones y PR por repo.**

~~~sql
PRAGMA foreign_keys = OFF;
BEGIN IMMEDIATE;

CREATE TABLE task_repository_merge_batch (
    id                    TEXT PRIMARY KEY,
    task_id               TEXT NOT NULL REFERENCES task(id) ON DELETE RESTRICT,
    gate_evaluation_id    TEXT NOT NULL REFERENCES gate_evaluation(id) ON DELETE RESTRICT,
    repository_set_digest TEXT CHECK (
        repository_set_digest IS NULL OR length(repository_set_digest) = 64
    ),
    state                 TEXT NOT NULL DEFAULT 'integrating'
        CHECK (state IN ('integrating', 'publishing', 'open',
                         'completed', 'blocked', 'failed')),
    version               INTEGER NOT NULL DEFAULT 1 CHECK (version >= 1),
    created_at            TEXT NOT NULL,
    updated_at            TEXT NOT NULL,
    UNIQUE (id, task_id)
);
CREATE UNIQUE INDEX idx_task_repository_merge_batch_active
    ON task_repository_merge_batch(task_id)
    WHERE state IN ('integrating', 'publishing', 'open', 'blocked');

CREATE TABLE task_repository_merge_batch_repo (
    batch_id                 TEXT NOT NULL,
    task_id                  TEXT NOT NULL,
    repo_id                  TEXT NOT NULL,
    position                 INTEGER NOT NULL CHECK (position >= 0),
    state                    TEXT NOT NULL DEFAULT 'pending'
        CHECK (state IN ('pending', 'integrating', 'integrated', 'publishing',
                         'open', 'merged', 'conflict', 'closed', 'failed')),
    work_unit_integration_id TEXT REFERENCES work_unit_integration(id) ON DELETE RESTRICT,
    task_merge_operation_id  TEXT REFERENCES task_integration_operation(id) ON DELETE RESTRICT,
    pr_metadata_id           TEXT REFERENCES pr_metadata(id) ON DELETE RESTRICT,
    before_sha               TEXT,
    after_sha                TEXT,
    created_at               TEXT NOT NULL,
    updated_at               TEXT NOT NULL,
    PRIMARY KEY (batch_id, repo_id),
    UNIQUE (batch_id, position),
    FOREIGN KEY (batch_id, task_id)
        REFERENCES task_repository_merge_batch(id, task_id) ON DELETE CASCADE,
    FOREIGN KEY (task_id, repo_id)
        REFERENCES task_repository(task_id, repo_id) ON DELETE RESTRICT,
    CHECK (state NOT IN ('open', 'merged') OR pr_metadata_id IS NOT NULL)
);
CREATE INDEX idx_task_repository_merge_batch_repo_state
    ON task_repository_merge_batch_repo(batch_id, state, position);

ALTER TABLE task_integration_operation
    ADD COLUMN repo_id TEXT REFERENCES repo(id) ON DELETE RESTRICT;
ALTER TABLE task_integration_operation
    ADD COLUMN repository_merge_batch_id TEXT
        REFERENCES task_repository_merge_batch(id) ON DELETE RESTRICT;
UPDATE task_integration_operation
   SET repo_id = (SELECT t.repo_id FROM task t
                  WHERE t.id = task_integration_operation.task_id);
CREATE TEMP TABLE _v183_running_merge_scope_guard (
    unresolved INTEGER NOT NULL CHECK (unresolved = 0)
);
INSERT INTO _v183_running_merge_scope_guard
SELECT COUNT(*) FROM task_integration_operation op
WHERE op.kind = 'task_merge' AND op.status = 'running'
  AND (op.repo_id IS NULL OR op.gate_evaluation_id IS NULL);
DROP TABLE _v183_running_merge_scope_guard;
CREATE INDEX idx_task_integration_operation_repo
    ON task_integration_operation(task_id, repo_id, created_at, id);
CREATE INDEX idx_task_integration_operation_merge_batch_repo
    ON task_integration_operation(repository_merge_batch_id, repo_id, created_at, id)
    WHERE repository_merge_batch_id IS NOT NULL;

ALTER TABLE remote_pr_admission
    ADD COLUMN repo_id TEXT REFERENCES repo(id) ON DELETE RESTRICT;
UPDATE remote_pr_admission
   SET repo_id = (SELECT t.repo_id FROM task t
                  WHERE t.id = remote_pr_admission.task_id);
CREATE INDEX idx_remote_pr_admission_repo_reconcile
    ON remote_pr_admission(repo_id, state, updated_at, task_merge_operation_id);
CREATE TEMP TABLE _v183_remote_pr_repo_backfill_guard (
    unresolved INTEGER NOT NULL CHECK (unresolved = 0)
);
INSERT INTO _v183_remote_pr_repo_backfill_guard
SELECT COUNT(*) FROM remote_pr_admission WHERE repo_id IS NULL;
DROP TABLE _v183_remote_pr_repo_backfill_guard;

-- Keep a Task already in Merging attached to its exact durable reason_ref.
-- Legacy batch digest remains NULL; only new batch inserts require a digest.
UPDATE task_integration_operation
   SET repository_merge_batch_id = id
 WHERE kind = 'task_merge' AND status = 'running';
UPDATE task_integration_operation
   SET repository_merge_batch_id = (
       SELECT merge_op.repository_merge_batch_id
       FROM task_integration_operation merge_op
       WHERE merge_op.id = task_integration_operation.parent_operation_id
   )
 WHERE kind = 'publish_pr' AND status = 'running'
   AND parent_operation_id IS NOT NULL
   AND EXISTS (
       SELECT 1 FROM task_integration_operation merge_op
       WHERE merge_op.id = task_integration_operation.parent_operation_id
         AND merge_op.kind = 'task_merge' AND merge_op.status = 'running'
   );
INSERT INTO task_repository_merge_batch(
    id, task_id, gate_evaluation_id, repository_set_digest,
    state, version, created_at, updated_at
)
SELECT op.id, op.task_id, op.gate_evaluation_id, NULL,
       CASE WHEN admission.state = 'open' THEN 'open'
            WHEN op.remote_waiting = 1 THEN 'publishing'
            ELSE 'integrating' END,
       1, op.created_at, op.updated_at
FROM task_integration_operation op
LEFT JOIN remote_pr_admission admission
  ON admission.task_merge_operation_id = op.id
WHERE op.kind = 'task_merge' AND op.status = 'running';
INSERT INTO task_repository_merge_batch_repo(
    batch_id, task_id, repo_id, position, state,
    task_merge_operation_id, pr_metadata_id, created_at, updated_at
)
SELECT op.id, op.task_id, op.repo_id, 0,
       CASE WHEN admission.state = 'open' THEN 'open'
            WHEN admission.state = 'merged' THEN 'merged'
            WHEN admission.state = 'closed' THEN 'closed'
            WHEN op.remote_waiting = 1 THEN 'publishing'
            ELSE 'integrating' END,
       op.id,
       metadata.id, op.created_at, op.updated_at
FROM task_integration_operation op
JOIN task t ON t.id = op.task_id
LEFT JOIN remote_pr_admission admission
  ON admission.task_merge_operation_id = op.id
LEFT JOIN pr_metadata metadata
  ON metadata.task_merge_operation_id = op.id
WHERE op.kind = 'task_merge' AND op.status = 'running'
  AND op.repo_id IS NOT NULL;

-- Historical batches may have a NULL digest. New writes may not.
CREATE TRIGGER task_repository_merge_batch_insert_guard
BEFORE INSERT ON task_repository_merge_batch
WHEN NEW.repository_set_digest IS NULL
  OR length(NEW.repository_set_digest) != 64
  OR NOT EXISTS (
      SELECT 1 FROM gate_evaluation e
      WHERE e.id = NEW.gate_evaluation_id AND e.task_id = NEW.task_id
  )
BEGIN
    SELECT RAISE(ABORT, 'new repository merge batch requires an exact Gate evaluation and set digest');
END;

CREATE TRIGGER task_integration_operation_repository_immutable
BEFORE UPDATE OF repo_id, repository_merge_batch_id ON task_integration_operation
WHEN NEW.repo_id IS NOT OLD.repo_id
  OR NEW.repository_merge_batch_id IS NOT OLD.repository_merge_batch_id
BEGIN
    SELECT RAISE(ABORT, 'integration operation repository and batch are immutable');
END;

CREATE TRIGGER remote_pr_admission_repo_immutable
BEFORE UPDATE OF repo_id ON remote_pr_admission
WHEN NEW.repo_id IS NOT OLD.repo_id
BEGIN
    SELECT RAISE(ABORT, 'remote PR admission repository is immutable');
END;

CREATE TABLE pr_metadata_v183 (
    id                      TEXT PRIMARY KEY,
    task_id                 TEXT NOT NULL REFERENCES task(id),
    repo_id                 TEXT REFERENCES repo(id) ON DELETE RESTRICT,
    provider_type           TEXT NOT NULL,
    provider_pr_id          TEXT,
    pr_url                  TEXT,
    source_branch           TEXT NOT NULL,
    target_branch           TEXT NOT NULL,
    pr_state                TEXT NOT NULL DEFAULT 'draft',
    merge_status            TEXT NOT NULL DEFAULT 'pending',
    last_synced_at          TEXT,
    created_at              TEXT NOT NULL,
    updated_at              TEXT NOT NULL,
    task_merge_operation_id TEXT
        REFERENCES task_integration_operation(id) ON DELETE RESTRICT,
    publish_operation_id    TEXT
        REFERENCES task_integration_operation(id) ON DELETE RESTRICT,
    admission_status        TEXT NOT NULL DEFAULT 'legacy_unadmitted'
        CHECK (admission_status IN (
            'admitted', 'reconciliation_required', 'open', 'merged', 'closed',
            'failed', 'legacy_unadmitted'
        )),
    UNIQUE (task_id, repo_id)
);
INSERT INTO pr_metadata_v183(
    id, task_id, repo_id, provider_type, provider_pr_id, pr_url,
    source_branch, target_branch, pr_state, merge_status, last_synced_at,
    created_at, updated_at, task_merge_operation_id, publish_operation_id,
    admission_status
)
SELECT m.id, m.task_id, t.repo_id, m.provider_type, m.provider_pr_id, m.pr_url,
       m.source_branch, m.target_branch, m.pr_state, m.merge_status,
       m.last_synced_at, m.created_at, m.updated_at,
       m.task_merge_operation_id, m.publish_operation_id, m.admission_status
FROM pr_metadata m
LEFT JOIN task t ON t.id = m.task_id;
DROP TRIGGER legacy_pr_history_insert_guard;
DROP TRIGGER pr_metadata_admission_status_guard;
DROP TRIGGER pr_metadata_merge_admission_guard_insert;
DROP TRIGGER pr_metadata_merge_admission_guard_update;
DROP TRIGGER pr_rework_supersession_insert_guard;
DROP TRIGGER remote_pr_admission_insert_guard;
DROP TRIGGER remote_pr_history_insert_guard;
DROP TRIGGER task_integration_operation_provider_result_guard;
DROP TRIGGER task_merge_closed_provider_transition_guard;
DROP TRIGGER task_integration_operation_remote_admission_guard;
DROP TRIGGER task_publish_pr_gate_admission_guard;
DROP TRIGGER task_lifecycle_pr_feedback_rework_guard;
DROP TABLE pr_metadata;
ALTER TABLE pr_metadata_v183 RENAME TO pr_metadata;
CREATE INDEX idx_pr_metadata_task_repo
    ON pr_metadata(task_id, repo_id, created_at, id);
CREATE TRIGGER pr_metadata_repo_insert_guard
BEFORE INSERT ON pr_metadata
WHEN NEW.repo_id IS NULL OR NOT EXISTS (
    SELECT 1 FROM task t
    JOIN repo r ON r.id = NEW.repo_id AND r.project_id = t.project_id
    JOIN task_repository tr
      ON tr.task_id = t.id AND tr.repo_id = NEW.repo_id
     AND tr.role IN ('primary', 'affected')
    WHERE t.id = NEW.task_id
)
BEGIN
    SELECT RAISE(ABORT, 'new PR metadata requires a writable repository of its Task');
END;
CREATE TRIGGER pr_metadata_repo_immutable
BEFORE UPDATE OF repo_id ON pr_metadata
WHEN NEW.repo_id IS NOT OLD.repo_id
BEGIN
    SELECT RAISE(ABORT, 'PR metadata repository identity is immutable');
END;

CREATE TRIGGER task_repository_merge_batch_repo_insert_guard
BEFORE INSERT ON task_repository_merge_batch_repo
WHEN NOT EXISTS (
    SELECT 1 FROM task_repository tr
    WHERE tr.task_id = NEW.task_id AND tr.repo_id = NEW.repo_id
      AND tr.role IN ('primary', 'affected')
)
 OR (NEW.task_merge_operation_id IS NOT NULL AND NOT EXISTS (
     SELECT 1 FROM task_integration_operation op
     WHERE op.id = NEW.task_merge_operation_id
       AND op.task_id = NEW.task_id AND op.repo_id = NEW.repo_id
       AND op.repository_merge_batch_id = NEW.batch_id
 ))
 OR (NEW.pr_metadata_id IS NOT NULL AND NOT EXISTS (
     SELECT 1 FROM pr_metadata m
     WHERE m.id = NEW.pr_metadata_id
       AND m.task_id = NEW.task_id AND m.repo_id = NEW.repo_id
 ))
BEGIN
    SELECT RAISE(ABORT, 'merge batch repo must bind its exact writable repo and operation/PR');
END;

COMMIT;
PRAGMA foreign_keys = ON;
~~~

V183 crea `task_repository_merge_batch` como el único reason_ref Task-wide de
Merging y `task_repository_merge_batch_repo` como conjunto ordenado de repos
que sí cambiaron; repos sin cambios no generan fila. Un batch nuevo lleva
digest de 64 chars; el NULL de un batch histórico preserva el estado previo sin
inventar digest. Cada TaskMerge/PublishPr lleva
`repository_merge_batch_id`; los eventos por repo no reemplazan TaskLifecycle.
El batch llega a `completed` cuando cada fila está integrada localmente o su
PR está mergeada; `ready_to_merge` exige PRs abiertas y admitidas. Un
conflict/closed bloquea el Task y la recuperación opera solo sobre filas
pendientes del mismo batch.

V183 copia todas las filas y las 15 columnas actuales de `pr_metadata`:
`id, task_id, provider_type, provider_pr_id, pr_url, source_branch,
target_branch, pr_state, merge_status, last_synced_at, created_at, updated_at,
task_merge_operation_id, publish_operation_id, admission_status`; añade
`repo_id` desde `task.repo_id` y reemplaza solo `task_id UNIQUE` por
`UNIQUE(task_id, repo_id)`. Repo NULL histórico permanece NULL; triggers de
INSERT rechazan metadata nueva sin repo y exigen asociación primary/affected.
Los índices anteriores de `pr_metadata` eran PK y autoíndice de
`task_id UNIQUE`; se conserva PK, sustituye la unicidad por el compuesto y se
añade `idx_pr_metadata_task_repo`.

`remote_pr_admission` no se reconstruye: preserva en orden todas las columnas
`task_merge_operation_id, publish_operation_id, metadata_id, task_id,
provider_config_id, provider_type, provider_config_revision,
provider_config_digest, provider_base_url, token_secret_ref,
remote_repo_identity, source_branch, target_branch, admitted_source_sha, state,
reconciliation_reason, provider_event_id, observed_head_sha, merged_commit_sha,
result_event_id, created_at, updated_at, provider_status,
result_classification, credential_handle_id, credential_owner_user_id,
feedback_policy_json, feedback_policy_digest, rework_outcome, supersession_id,
readmits_supersession_id`; añade `repo_id`. Mantiene CHECKs `state IN
(admitted,reconciliation_required,open,merged,closed,head_mismatch,
publication_failed)`, `provider_status IN (open,merged,closed,publication_failed)`,
`result_classification IS NULL OR (head_mismatch AND provider_status=merged)`,
digest de feedback de 64 chars, `rework_outcome IS NULL OR
superseded_for_rework`, y los CHECKs de config digest, remote identity y
admitted SHA no vacíos. Mantiene FKs de TaskMerge, PublishPr, metadata, Task y
result event (provider config no tiene FK), PK `task_merge_operation_id`,
`publish_operation_id UNIQUE`, `idx_remote_pr_admission_reconcile` y
autoíndices. El guard de backfill aborta si alguna admisión histórica no puede
asociarse a un repo.

Antes del DROP se eliminan temporalmente los nueve triggers cuyo SQL referencia
`pr_metadata` (tres attached y seis de otras tablas), y V183 los recrea con el
mismo nombre y el cuerpo vigente de su última migración. Los tres attached son
`pr_metadata_merge_admission_guard_insert` (V109),
`pr_metadata_admission_status_guard` (V113) y
`pr_metadata_merge_admission_guard_update` (V175; prevalece sobre V116/V109).
Los seis cross-table exactos son `legacy_pr_history_insert_guard` (V114),
`pr_rework_supersession_insert_guard` (V175),
`remote_pr_admission_insert_guard` (V175), `remote_pr_history_insert_guard`
(V116), `task_integration_operation_provider_result_guard` (cuerpo vigente
V175) y `task_merge_closed_provider_transition_guard` (V109). No se alteran
`external_feedback_item_insert_guard`, `pr_external_action_insert_guard`,
`pr_external_action_scope_guard`, `pr_external_action_start_guard`,
`task_coordination_request_external_feedback_source_guard` ni
`task_external_feedback_set_item_guard`: no referencian `pr_metadata`.

Además se eliminan y recrean temporalmente tres guards por el cambio de causa
de lifecycle: `task_integration_operation_remote_admission_guard` y
`task_publish_pr_gate_admission_guard` (cuerpo vigente V109), y
`task_lifecycle_pr_feedback_rework_guard` (V179). `pr_metadata` conserva las
dos autoíndices vigentes: PK `id` y `UNIQUE(task_id)`; V183 los cambia a PK
`id` y `UNIQUE(task_id, repo_id)`, añade `idx_pr_metadata_task_repo` y no hay
otros índices explícitos ni triggers attached que se omitan. Añade
`pr_metadata_repo_insert_guard` (repo no NULL, mismo Project, role
primary/affected) y `pr_metadata_repo_immutable`.

`remote_pr_admission` conserva sus seis triggers attached:
`remote_pr_admission_delete_guard`,
`remote_pr_admission_identity_immutable`,
`remote_pr_admission_insert_guard`, `remote_pr_admission_rework_guard`,
`remote_pr_admission_superseded_immutable`,
`remote_pr_admission_terminal_result_immutable`. Se reemplaza el insert guard
V175 manteniendo todos sus checks de TaskMerge/PublishPr, metadata, lifecycle,
remote identity, config, credential/policy snapshots y rework; sus joins ahora
exigen `NEW.repo_id` igual a `merge_op.repo_id`,
`publish_op.repo_id`, `metadata.repo_id`, `config.repo_id`, y la asociación
Task primary/affected. Nuevo guard vuelve repo_id inmutable, también para una
admisión superseded.

Los nueve guards que refieren a `pr_metadata` conservan su cuerpo vigente y
añaden igualdad de repo al enlace metadata/admission/operación/evento. En
particular, insert/update de metadata exigen
`NEW.repo_id = merge_op.repo_id = publish_op.repo_id`; status exige
`admission.repo_id = NEW.repo_id`; historial legado/remoto exige el repo exacto
de metadata/admisión; rework exige metadata y admisión del mismo repo y
`lifecycle.reason_ref = merge_op.repository_merge_batch_id`; resultado del
proveedor y cierre exigen `metadata.repo_id = operation.repo_id` y el batch
como `Merging.reason_ref`. El insert guard de admisión conserva sus checks V175
de TaskMerge/PublishPr, metadata, remote identity, config, credential/policy
snapshots y rework; exige además `NEW.repo_id = merge_op.repo_id =
publish_op.repo_id = metadata.repo_id = config.repo_id`, asociación
primary/affected y `lifecycle.reason_ref = merge_op.repository_merge_batch_id`.

Los tres guards adicionales se reemplazan así: la inserción TaskMerge exige
`repo_id` primary/affected y crea la operación dentro del batch; PublishPr
exige mismo `repo_id` y `repository_merge_batch_id` que su parent, Gate
satisfecho exacto y `Merging.reason_ref = batch.id`; feedback rework conserva
las transiciones D3' y enlaza la admisión al repo/batch exacto. `repo_id` y
`repository_merge_batch_id` de operaciones, `remote_pr_admission.repo_id` y
`pr_metadata.repo_id` son inmutables tras INSERT. Los estados por repo se
agregan en el batch antes de escribir TaskLifecycle; no se añade una segunda
autoridad de lifecycle. El orden de integración es `primary` primero y luego
`affected.position` ascendente, aplicado por servicio. Se conserva
`idx_task_integration_operation_active_task`,
`idx_task_integration_operation_publish_parent`,
`idx_task_integration_operation_task_created` y sus dos autoíndices actuales.
La integración local sigue serializada por Task; PR remotas durables pueden
coexistir por repo. Se conserva `idx_pr_rework_supersession_open`: rework de
feedback de PR queda serializado por Task, aunque metadata, admisión y eventos
conservan la identidad de repo exacta.

Invariante de espejo (INV-034): `task.repo_id` sigue nullable y es el valor de
la única fila primary; repo NULL implica cero filas primary. V181 lo mantiene
con los guards de INSERT/UPDATE/DELETE de `task_repository` y los triggers
espejo AFTER INSERT/UPDATE de Task del bloque. Todas las filas de repos deben
pertenecer al Project de la Task. Un cambio de repo-set con WorkUnits o
Execution requiere transacción versionada y replan con decisión Human exacta.

Validación estática ejecutada con `python3 -I`: se aplicaron en orden los 178
archivos históricos V001–V180 (V176 no existe), después los tres bloques SQL
V181–V183 y una fixture de Project/Repo/Task con `pr_metadata` legacy. Resultado
**PASS** en SQLite 3.46.1: backfill de una única fila primary coincidente,
Task repo-less sin fila, espejo INV-034 probado en INSERT/null/reasignación y
rechazo de borrar primary stale, PR legacy con sus 15 columnas intactas y
`repo_id` backfilled, cero filas en `PRAGMA foreign_key_check`. También se
cotejó el inventario de triggers (9 referencias a `pr_metadata`, 6 attached a
`remote_pr_admission`) contra las definiciones instaladas por V114/V116/V175/
V179/V109; las rutas de trigger modificadas quedan especificadas arriba, pero
no se ejecutaron sus casos de dominio. Cargo no se ejecutó.

### Tipos Rust, repositorios y servicios

Enums nuevos usan `enum_strings!` en `crates/db/src/models.rs` con estos valores
persistidos: `TaskRepositoryRole { Primary => "primary", Affected =>
"affected", Context => "context" }`; `TaskRepositoryMergeBatchState {
Integrating => "integrating", Publishing => "publishing", Open => "open",
Completed => "completed", Blocked => "blocked", Failed => "failed" }`;
`TaskRepositoryMergeBatchRepoState { Pending => "pending", Integrating =>
"integrating", Integrated => "integrated", Publishing => "publishing",
Open => "open", Merged => "merged", Conflict => "conflict", Closed =>
"closed", Failed => "failed" }`.

Modelos M2 adicionales: `CreateWorkspaceGroup { id: String, task_id: String,
work_unit_id: String, root_path: String, created_at: String }`;
`WorkUnitReviewSubjectRepository { review_work_unit_id: String, task_id: String,
repo_id: String, reviewed_work_unit_id: String,
reviewed_work_unit_version: Option<i64>, result_execution_id: Option<String>,
result_artifact_id: Option<String>, result_commit_sha: Option<String>,
result_workspace_id: Option<String>, workspace_snapshot_digest: Option<String>,
created_at: String }`; `WorkUnitResultRequirementRepository { work_unit_id: String,
task_id: String, repo_id: String, source_work_unit_id: String,
source_work_unit_version: i64,
result_execution_id: String, result_artifact_id: Option<String>,
result_commit_sha: Option<String>, result_workspace_id: Option<String>,
workspace_snapshot_digest: Option<String>, created_at: String }`.
`WorkUnitRepo::list_review_subject_repositories(&self, work_unit_id: &str) ->
db::Result<Vec<WorkUnitReviewSubjectRepository>>` y
`replace_result_requirement_repositories(&self, work_unit_id: &str, rows:
&[WorkUnitResultRequirementRepository]) -> db::Result<Vec<WorkUnitResultRequirementRepository>>` mantienen el Task y versionado exactos.

| Fase | Modelos, métodos y firmas congelados | Archivos exactos |
|---|---|---|
| M1 | `TaskRepository { task_id: String, repo_id: String, role: TaskRepositoryRole, position: i64, created_at: String }`; `TaskRepositoryWrite { repo_id: String, role: TaskRepositoryRole }`; `WorkUnitRepositoryScope { work_unit_id: String, task_id: String, repo_id: String, created_at: String }`. `TaskRepo::list_repositories(&self, task_id: &str) -> db::Result<Vec<TaskRepository>>`; `replace_repositories_in_tx(&self, tx: &mut Transaction<'_, Sqlite>, task_id: &str, expected_version: i64, rows: &[TaskRepositoryWrite], updated_at: &str) -> db::Result<Task>`. `WorkUnitRepo::list_repository_ids(&self, work_unit_id: &str) -> db::Result<Vec<String>>`; `CreateTask.repositories: Vec<TaskRepositoryWrite>` y `CreateWorkUnit.repository_ids: Vec<String>` se materializan en las transacciones existentes. `TaskService::create_task_with_repositories(&self, project_id: impl Into<String>, title: impl Into<String>, description: Option<String>, parent_task_id: Option<String>, priority: Option<i64>, repositories: Option<Vec<TaskRepositoryWrite>>, initial_lifecycle_state: Option<TaskLifecycleState>, merge_config: Option<Value>) -> services::Result<Task>`; `replace_task_repositories(&self, task_id: &str, expected_version: i64, repositories: Vec<TaskRepositoryWrite>) -> services::Result<Task>` (409 tras Execution, salvo replan autorizado). `UpdateProject.primary_repo_id: Option<Option<String>>`; `ProjectRepositoryService::update_primary_repo(&self, project_id: &str, expected_version: i64, primary_repo_id: Option<String>, caller_user_id: &str) -> services::Result<Project>`; mantiene `ProjectRepo::update_at_version`. | `crates/db/src/models.rs`, `crates/db/src/repository.rs`, `crates/db/src/sqlite/task.rs`, `crates/db/src/sqlite/project.rs`, `crates/db/src/sqlite/work_unit.rs`, `crates/services/src/task_service/create.rs`, nuevo `crates/services/src/task_service/repositories.rs`, `crates/services/src/task_service.rs`, nuevo `crates/services/src/project_repository_service.rs`, `crates/services/src/lib.rs`, `crates/services/src/work_unit_service.rs`. |
| M2 | `WorkspaceGroup { id: String, task_id: String, work_unit_id: String, root_path: String, created_at: String }`; `ExecutionRepositoryResult { execution_id: String, task_id: String, repo_id: String, workspace_id: String, before_sha: Option<String>, after_sha: Option<String>, snapshot_digest: Option<String>, created_at: String }`; `WorkUnitReviewSubjectRepository` y `WorkUnitResultRequirementRepository` conservan los campos exactos de sus tablas V182. `WorkspaceRepo::create_group_for_work_unit(&self, input: CreateWorkspaceGroup) -> db::Result<WorkspaceGroup>`; `ExecutionRepo::record_repository_results(&self, execution_id: &str, rows: &[ExecutionRepositoryResult]) -> db::Result<Vec<ExecutionRepositoryResult>>`; `WorkUnitService::prepare_workspace_group(&self, task_id: &str, work_unit_id: &str) -> services::Result<WorkspaceGroup>`; `TaskService::record_execution_repository_results(&self, execution_id: &str) -> services::Result<Vec<ExecutionRepositoryResult>>`; `ValidationService::run_for_repository(&self, task_id: &str, repo_id: &str, check_key: &str, workspace_id: &str, commit_sha: &str) -> services::Result<ValidationRun>`. `ValidationRunResponse` añade `repo_id: String`, resuelto del workspace exacto. | `crates/db/src/models.rs`, `crates/db/src/repository.rs`, `crates/db/src/sqlite/workspace.rs`, `crates/db/src/sqlite/workspace_lease.rs`, `crates/db/src/sqlite/execution.rs`, `crates/db/src/sqlite/work_unit.rs`, `crates/services/src/task_service/workspace.rs`, `crates/services/src/task_service/execution/runner.rs`, `crates/services/src/work_unit_service.rs`, `crates/services/src/validation_service.rs`, `crates/services/src/collaboration_service.rs`, `crates/api-types/src/execution.rs`, `crates/api-types/src/work_unit.rs`, `crates/api-types/src/task_dispatch.rs`, `crates/api-types/src/review_validation.rs`, `crates/api/src/routes/work_units.rs`, `crates/api/src/routes/work_unit_reviews.rs`, `crates/api/src/routes/reviews.rs`. |
| M3 | `TaskRepositoryMergeBatch { id: String, task_id: String, gate_evaluation_id: String, repository_set_digest: Option<String>, state: TaskRepositoryMergeBatchState, version: i64, created_at: String, updated_at: String }`; `TaskRepositoryMergeBatchRepoRow { batch_id: String, task_id: String, repo_id: String, position: i64, state: TaskRepositoryMergeBatchRepoState, work_unit_integration_id: Option<String>, task_merge_operation_id: Option<String>, pr_metadata_id: Option<String>, before_sha: Option<String>, after_sha: Option<String>, created_at: String, updated_at: String }`. `TaskIntegrationOperation` gains `repo_id: Option<String>` y `repository_merge_batch_id: Option<String>`; `PrMetadata.repo_id: Option<String>` (NULL solo para metadata histórica no admitida); `RemotePrAdmission.repo_id: String` (todo histórico se asocia en V183; los inserts nuevos no admiten NULL). `TaskRepositoryMergeBatchRepo::create(&self, batch: &TaskRepositoryMergeBatch, repos: &[TaskRepositoryMergeBatchRepoRow]) -> db::Result<TaskRepositoryMergeBatch>`; `transition_repo(&self, batch_id: &str, repo_id: &str, expected_version: i64, state: TaskRepositoryMergeBatchRepoState, work_unit_integration_id: Option<&str>, task_merge_operation_id: Option<&str>, pr_metadata_id: Option<&str>, before_sha: Option<&str>, after_sha: Option<&str>) -> db::Result<TaskRepositoryMergeBatchRepoRow>`. `PrMetadataRepo::list_by_task(&self, task_id: &str) -> db::Result<Vec<PrMetadata>>`; `get_by_task_repo(&self, task_id: &str, repo_id: &str) -> db::Result<Option<PrMetadata>>`; retires ambiguous `get_by_task_id`. `TaskIntegrationOperationManager::integrate_repositories(&self, task_id: &str, gate_evaluation_id: &str, ordered_repo_ids: &[String], idempotency_key: &str) -> services::Result<Vec<TaskRepositoryIntegrationResult>>`; `PrService::list_task_prs(&self, task_id: &str) -> services::Result<Vec<TaskRepositoryPrStatus>>`; `GateEngine::evaluate_required_checks_by_repo(&self, task_id: &str, repository_results: &[ExecutionRepositoryResult]) -> services::Result<GateEvaluation>`; `ProjectPlanService::verify_handoffs_by_repository(&self, predecessor_task_id: &str, successor_task_id: &str) -> services::Result<Vec<ProjectPlanRepositoryHandoff>>`. TaskLifecycle Merging fija `reason_kind = "task_repository_merge_batch"`, `reason_ref = batch.id`; no cambia su autoridad. | `crates/db/src/models.rs`, `crates/db/src/repository.rs`, `crates/db/src/sqlite.rs` (registro de módulo), nuevo `crates/db/src/sqlite/task_repository_merge_batch.rs`, `crates/db/src/sqlite/task_integration_operation.rs`, `crates/db/src/sqlite/pr_metadata.rs`, `crates/services/src/task_integration_operation.rs`, `crates/services/src/merge_service.rs`, `crates/services/src/pr_service.rs`, `crates/services/src/pr_feedback.rs`, `crates/services/src/gate_engine.rs`, `crates/services/src/validation_service.rs`, `crates/services/src/project_plan.rs`, `crates/services/src/task_coordination/replan.rs`, `crates/services/src/replan_classifier.rs`, `crates/services/src/task_lifecycle.rs`, `crates/api-types/src/task_repositories.rs`, `crates/api-types/src/integration.rs`, `crates/api-types/src/pr_feedback.rs`, `crates/api-types/src/public_events.rs`, `crates/api/src/routes/tasks/gates.rs`, `crates/api/src/routes/pr_feedback.rs`, `crates/api/src/routes/events.rs`. |

Tipos de retorno M3 definidos en sus módulos de servicio: `TaskRepositoryIntegrationResult { repo_id: String, state: TaskRepositoryMergeBatchRepoState, work_unit_integration_id: Option<String>, before_sha: Option<String>, after_sha: Option<String> }`; `TaskRepositoryPrStatus { repo_id: String, pr_metadata_id: String, provider_pr_id: Option<String>, pr_url: Option<String>, pr_state: String, merge_status: String, admission_status: String }`; `ProjectPlanRepositoryHandoff { repo_id: String, predecessor_commit_sha: Option<String>, successor_base_sha: Option<String>, ancestry_verified: bool }`.

La adición a `MergeAfterGateResponse` y sus tipos generados pertenece a
`crates/api-types/src/gates.rs` (M3).
La documentación de M2 actualiza `docs/api.md`,
`docs/concepts/executions.md`, `docs/concepts/work-units.md`,
`docs/concepts/review-and-validation.md`; M3 actualiza `docs/api.md`,
`docs/concepts/gates.md`, `docs/concepts/project-plan.md`,
`docs/concepts/pr-feedback-loop.md` y
`CHANGELOG.md` Unreleased.

Execution authority remains Actor + RoleMembership + WorkUnit + exact lease and
provider-start. Repo results are evidence; Gate remains deterministic and does
not run checks.

### Contratos API, MCP, SSE y web

Requests nuevos/modificados en `crates/api-types` mantienen
`Serialize, Deserialize, TS`, `#[ts(export)]` y
`#[serde(deny_unknown_fields)]`. Todo `i64` publicado lleva
`#[ts(type = "number")]`.

- `requests.rs`: `CreateTaskRequest.repositories:
  Option<Vec<TaskRepositoryInput>>`, `#[serde(default)]`,
  `#[ts(optional = nullable)]`; omisión mantiene herencia de padre o Project
  primary y `Some([])` crea explícitamente una Task repo-less. Nuevo
  `UpdateTaskRepositoriesRequest { expected_version: i64,
  repositories: Vec<TaskRepositoryInput> }` lleva
  `#[derive(Serialize, Deserialize, TS)]`, `#[serde(deny_unknown_fields)]`,
  `#[ts(export)]` y `#[ts(type = "number")]` para la versión.
  `UpdateProjectRequest.primary_repo_id: Option<Option<String>>` distingue
  omitido de null con `#[serde(default, deserialize_with =
  "deserialize_optional_update_field")]`, `#[ts(type = "string | null")]` y
  `#[ts(optional)]`; conserva el `version: i64` existente como number.
- Nuevo `task_repositories.rs`: `TaskRepositoryRole` deriva
  `Serialize, Deserialize, TS`, `#[serde(rename_all = "snake_case")]`,
  `#[ts(export)]`; `TaskRepositoryInput { repo_id: String, role:
  TaskRepositoryRole }` deriva los mismos traits con
  `#[serde(deny_unknown_fields)]` y `#[ts(export)]`. Responses:
  `TaskRepositoryResponse { repo_id: String, role: TaskRepositoryRole,
  position: i64, merge: Option<TaskRepositoryMergeResponse>, pull_request:
  Option<TaskRepositoryPullRequestResponse> }`; merge lleva
  `{ batch_id: String, state: TaskRepositoryMergeState,
  integration_id: Option<String>, task_merge_operation_id: Option<String>,
  pr_metadata_id: Option<String>, before_sha: Option<String>,
  after_sha: Option<String> }`;
  `TaskRepositoryPullRequestResponse { pr_metadata_id: String,
  provider_type: String, provider_pr_id: Option<String>, pr_url: Option<String>,
  pr_state: String, merge_status: String, admission_status: String }`.
  `position` lleva `#[ts(type = "number")]`; responses derivan
  `Serialize, Deserialize, TS` + `#[ts(export)]`. Ninguno incluye credential
  refs, snapshots de política ni tokens. M3 añade enum público
  `TaskRepositoryMergeState` con `#[serde(rename_all = "snake_case")]` y
  valores `pending|integrating|integrated|publishing|open|merged|conflict|
  closed|failed`; `TaskRepositoryMergeBatchState` usa
  `integrating|publishing|open|completed|blocked|failed`.
  `MergeAfterGateResponse` en `gates.rs` añade
  `repository_merge_batch: Option<TaskRepositoryMergeBatchResponse>` con
  `#[serde(default)]`, `#[ts(optional = nullable)]`; el batch es
  `{ id: String, task_id: String, gate_evaluation_id: String,
  repository_set_digest: Option<String>, state: TaskRepositoryMergeBatchState,
  version: i64, repositories: Vec<TaskRepositoryMergeRepoResponse> }`. Cada repo
  expone `{ repo_id: String, position: i64, state: TaskRepositoryMergeState,
  work_unit_integration_id: Option<String>, task_merge_operation_id:
  Option<String>, pr_metadata_id: Option<String>, before_sha: Option<String>,
  after_sha: Option<String> }`; positions/version son TS number y los arrays
  responses llevan `#[serde(default)]`.
- `TaskResponse.repositories: Vec<TaskRepositoryResponse>` en
  `execution.rs`, con `#[serde(default)]`; `repo_id: Option<String>` sigue
  siendo el espejo primary. `CreateWorkUnitRequest.repository_ids:
  Option<Vec<String>>` en `work_unit.rs`, `#[serde(default)]` y
  `#[ts(optional = nullable)]`; vacío/omitido significa todos los repos
  escribibles. `WorkUnitResponse.repository_ids: Vec<String>` lleva
  `#[serde(default)]`.
- `ExecutionResponse.repository_results: Vec<ExecutionRepositoryResultResponse>`
  en `execution.rs`; cada item tiene `repo_id: String`, `workspace_id: String`,
  `before_sha/after_sha/snapshot_digest: Option<String>` y sus arrays llevan
  `#[serde(default)]`. `ReviewSubjectResponse.repository_results` en
  `task_dispatch.rs` tiene items `{ repo_id: String,
  reviewed_work_unit_version: Option<i64>, result_execution_id:
  Option<String>, result_artifact_id: Option<String>, result_commit_sha:
  Option<String>, result_workspace_id: Option<String>,
  workspace_snapshot_digest: Option<String> }`; i64 usa TS number. `WorkUnitIntegrationResponse`
  en `work_unit.rs` añade `repo_id: String`.
- `ExternalFeedbackItemResponse`, `PrExternalActionResponse` y
  `PrReworkSupersessionResponse` en `pr_feedback.rs` añaden `repo_id: String`.
  Los DTO de eventos nuevos se exportan desde `public_events.rs` y añaden
  `repo_id` solo a eventos de integración/feedback/PR que tienen un único repo;
  requests rechazan campos desconocidos y responses mantienen los defaults
  aditivos de arrays.

**REST.**

| Método y ruta | Contrato |
|---|---|
| `POST /api/v1/projects/{project_id}/tasks` | Campo repositories; omisión compatible con 0.9.1. |
| `PATCH /api/v1/tasks/{task_id}/repositories` | Nuevo body expected_version + repositories; Project write authority y pre-Execution. Tras Execution, solo por replan autorizado. |
| `PATCH /api/v1/projects/{project_id}` | primary_repo_id nullable; owner/admin, repo del mismo Project, version esperada; solo Tasks futuras. |
| `POST /api/v1/tasks/{task_id}/work-units` | repository_ids subset writable de la Task. |
| `GET /api/v1/tasks/{task_id}/validation-runs` (existente) | `ValidationRunResponse.repo_id` resuelto del Workspace exacto. |
| `POST /api/v1/work-units/{id}/review-report` (existente) | M2 congela y devuelve resultados de review por repo. |
| `POST /api/v1/tasks/{id}/merge` (existente) | Gate exacto; M3 crea/continúa el batch y devuelve estado por repo. |
| `GET /api/v1/tasks/{task_id}/pr-feedback`, `/pr-actions`, `/pr-rework` (existentes) | Respuestas incluyen repo_id; listados permanecen Task-scoped y paginados. |
| `POST /api/v1/tasks/{task_id}/pr-actions/{action_id}/approve` y `/reject` (existentes) | Decisión Human exacta del action y repo asociado. |

Handlers exactos: M1 `crates/api/src/routes/tasks/crud.rs`,
`projects.rs`, `work_units.rs`; M2 `work_units.rs`, `events.rs`; M3
`crates/api/src/routes/tasks/gates.rs`, `pr_feedback.rs`, `events.rs`.
`crates/api-types/src/lib.rs` registra/expone los DTO en cada fase.

Errores tipados: añadir `ServiceError::{TaskRepositoriesInvalid,
TaskRepositoryNotInProject, TaskRepositoryReplanRequired,
WorkUnitRepositoriesInvalid, ProjectPrimaryRepoNotInProject}` y mapearlos en
`crates/api/src/errors.rs` a `ApiError` con códigos exactos:
`task.repositories.invalid` (400: primary ausente en un conjunto no vacío,
primary repetido, repo o rol duplicado); `task.repositories.repo_not_in_project` (422);
`task.repositories.replan_required` (409 tras Execution);
`work_unit.repositories.invalid` (422: cross-Task o context);
`project.primary_repo_not_in_project` (422). Versiones obsoletas conservan
`DbError::TaskVersionConflict`/`DbError::VersionConflict` y HTTP 409
`version_conflict`. Ningún error parcial escribe scope.

**MCP.** Ampliar schemas con `additionalProperties: false` en parámetros y
objetos anidados, y params/handlers de `forge_create_task`
(`repositories`: array de `{repo_id, role}`, roles primary/affected/context,
items `additionalProperties: false`), `forge_create_work_unit`
(`repository_ids: string[]`) y `forge_update_project`
(`primary_repo_id: string|null` y `version: integer` requerido para cada
update versionado). Añadir solo `forge_update_task_repositories` con
`task_id, expected_version, repositories`; items niegan propiedades extra.
Lectores `forge_get_task`, `forge_list_task_pr_feedback`,
`forge_list_task_pr_actions`, `forge_list_task_pr_rework` devuelven repo_id;
`forge_get_task` y `forge_create_task` devuelven Task repositories y
`forge_create_work_unit` devuelve `repository_ids`. `forge_transition_task_lifecycle`
conserva su schema; el servidor escribe el batch como `reason_ref`, sin aceptar
un batch id aportado por el cliente. No se añade herramienta de mutación PR ni
autoridad nueva.

**SSE.** Añadir a `PublicEventType`, allowlist y mapping:
`task.repositories_changed` ({task_id, task_version, repositories}),
`execution.repository_results_recorded` ({task_id, execution_id,
repository_results}), `work_unit.repository_integration_changed`
({task_id, work_unit_id, repo_id, integration_id, outcome, version}) y
`task.repository_pr_status_changed` ({task_id, repo_id, pr_metadata_id,
task_merge_operation_id, state, merge_status}). Añadir repo_id a eventos
existentes de integración, feedback/rework y external action. No publicar body
de feedback, secretos, rutas ni texto provider no confiable.

**Web.** M1 toca `web/src/components/task-create-dialog.tsx`,
`web/src/pages/ProjectSettingsPage.tsx`,
`web/src/components/settings/ReposTab.tsx`,
`web/src/pages/TaskDetailPage.tsx`,
`web/src/components/task-detail/task-work-management.tsx`; añadir
`web/src/components/task-detail/task-repository-panel.tsx`. Regenerar
`web/src/types/generated/` desde ts-rs, nunca editar bindings a mano.
M2 también toca `web/src/components/task-detail/task-validation-panel.tsx`,
`work-unit-review-panel.tsx` y `task-verification.ts`; M3 toca
`task-pr-actions-panel.tsx`, `task-pr-feedback-panel.tsx`,
`task-rework-panel.tsx` y `web/src/features/project-plan/ProjectPlanPage.tsx`.

### Propiedad de archivos y pruebas de M1

Cada archivo M1 pertenece a una sola lane:

| Lane | Archivos |
|---|---|
| db | `crates/db/migrations/V181__multirepo_task_repositories.sql`; `crates/db/src/models.rs`; `crates/db/src/repository.rs`; `crates/db/src/sqlite/task.rs`; `crates/db/src/sqlite/project.rs`; `crates/db/src/sqlite/work_unit.rs`; `crates/db/src/tests.rs`. |
| services + api + mcp | `crates/services/src/task_service/create.rs`; nuevo `crates/services/src/task_service/repositories.rs`; `crates/services/src/task_service.rs`; nuevo `crates/services/src/project_repository_service.rs`; `crates/services/src/work_unit_service.rs`; `crates/services/src/lib.rs`; `crates/api-types/src/requests.rs`; `crates/api-types/src/execution.rs`; `crates/api-types/src/work_unit.rs`; nuevo `crates/api-types/src/task_repositories.rs`; `crates/api-types/src/lib.rs`; `crates/api/src/lib.rs`; `crates/api/src/errors.rs`; `crates/api/src/routes/mod.rs`; `crates/api/src/routes/tasks/mod.rs`; `crates/api/src/routes/tasks/crud.rs`; `crates/api/src/routes/projects.rs`; `crates/api/src/routes/work_units.rs`; `crates/mcp-server/src/tools/descriptors.rs`; `crates/mcp-server/src/tools/handlers.rs`; `crates/mcp-server/src/tools/mod.rs`; `crates/mcp-server/src/params.rs`; `crates/mcp-server/src/tests.rs`; `crates/api/tests/happy_path.rs`; `crates/api/tests/repos_endpoints.rs`; `crates/api/tests/work_units.rs`; `crates/services/src/task_service/tests/service_tests/cases/subtasks.rs`; `.github/workflows/build-local-linux.yml`; `CHANGELOG.md`; `docs/api.md`; `docs/concepts/work-units.md`; `docs/concepts/task-operating-model.md`. |
| web | `web/src/components/task-create-dialog.tsx`; `web/src/components/task-detail/task-work-management.tsx`; `web/src/components/task-detail/task-work-management.test.tsx`; nuevo `web/src/components/task-detail/task-repository-panel.tsx`; `web/src/pages/TaskDetailPage.tsx`; `web/src/pages/ProjectSettingsPage.tsx`; `web/src/components/settings/ReposTab.tsx`; `web/src/types/generated/` (salida ts-rs). |

Tests M1 exactos y archivo existente:

- `multirepo_task_backfills_primary_and_preserves_repo_less_tasks`,
  `multirepo_task_primary_mirror_tracks_association_changes`,
  `multirepo_task_rejects_cross_project_and_duplicate_repository_roles` —
  `crates/db/src/tests.rs`.
- `multirepo_task_create_defaults_to_primary_and_inherits_subtask_scope`,
  `multirepo_task_repository_replace_conflicts_after_execution_or_stale_version`,
  `multirepo_task_project_primary_update_is_versioned_and_future_only` —
  `crates/services/src/task_service/tests/service_tests/cases/subtasks.rs`.
- `multirepo_task_create_request_omission_and_explicit_repositories` —
  `crates/api/tests/happy_path.rs`;
  `multirepo_task_project_patch_updates_primary_repo` —
  `crates/api/tests/repos_endpoints.rs`;
  `multirepo_task_work_unit_accepts_repository_ids` —
  `crates/api/tests/work_units.rs`.
- `multirepo_task_mcp_task_and_work_unit_schemas` —
  `crates/mcp-server/src/tests.rs`.
- `multirepo_task_create_and_work_unit_repository_selectors` —
  `web/src/components/task-detail/task-work-management.test.tsx`.

Añadir a `.github/workflows/build-local-linux.yml`:

~~~sh
cargo test --locked -p db --lib multirepo_task_
cargo test --locked -p services --lib multirepo_task_
cargo test --locked -p api --test happy_path multirepo_task_
cargo test --locked -p api --test repos_endpoints multirepo_task_
cargo test --locked -p api --test work_units multirepo_task_
cargo test --locked -p mcp-server --lib multirepo_task_
~~~

Estos filtros no se ejecutaron en M0.
