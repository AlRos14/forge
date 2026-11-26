# Plan — Soporte multirepo por Task (0.9.2)

Fecha: 2026-11-18. Baseline inspeccionado: `feat/multirepo` en `22a02b98`
(0.9.1, esquema hasta V180). Este documento congela los contratos y registra
el estado por anexo: M0 no declara capacidad implementada y el Anexo M1 marca
la integración como IMPLEMENTADO / NOT RUN, sin aceptación de CI. No se
compiló ni ejecutó tests localmente; la validación será solo en CI.

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
- Un lease writable por cada workspace escribible. Los workspaces context no
  reciben WorkspaceLease; su acceso es de solo lectura y se comprueba después
  de la Execution.
- El harness arranca en la raíz del grupo. Así ve todos los repos como
  carpetas hermanas, igual que una sesión de Claude o Codex en `~/Proyectos`.
- Los repos de contexto se fijan en el primer `workspace_group` que los necesite
  (o al aceptar un replan), antes de iniciar la Execution. V181 no conserva un
  SHA del repo al crear la Task; M2 lo registra sin inventar un valor histórico
  y todos los grupos posteriores de esa Task reutilizan ese pin. Se preparan con
  permisos de solo lectura en disco.
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
hija por repo. La identidad común del resultado conserva el Execution y la
versión; commit, Workspace y snapshot escalares reflejan el `primary` solo si
está en el scope, y quedan NULL si no lo está.

### Revisión

El revisor recibe un workspace de grupo con los commits de resultado de cada
repo y el diff por repo. El ReviewReport sigue siendo uno; los hallazgos pueden
llevar ruta prefijada por repo. Un fallback del mismo WorkUnit reutiliza
exactamente su grupo. Rework y micro-fix crean grupos nuevos y basan cada repo
en el resultado por repo fijado; no reciclan una raíz mutable de otra
Execution.
El grupo del revisor es de solo lectura, contiene solo los repos congelados en
su Review Subject y no recibe WorkspaceLease; su Execution mantiene
`workspace_id = NULL`.

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

Congelado inicialmente sobre `feat/multirepo` (`22a02b98`), fuente 0.9.1 y V180.
Revalidado contra el HEAD actual `ac87ab85d0c466ca1e73c22bbdedfa186a065aee`,
con M1 en el árbol. La prueba SQL indicada abajo es local y estática; Cargo,
builds y tests Rust quedan para CI. Los archivos citados son el alcance previsto
de implementación, no cambios incluidos en este plan.

Correcciones del baseline ya aplicadas arriba: `task.repo_id` no es UNIQUE;
`workspace_lease.workspace_id` tampoco es UNIQUE y
`repository_binding_id` no tiene FK; la unicidad actual de
`work_unit_integration` es `(task_id, operation_idempotency_key)`; los SHA y
snapshots de `execution`/review son escalares sin unicidad SQL; y el handoff de
Project Plan vive en `context_json` con digest. No apareció otro hallazgo del
baseline que contradiga el código inspeccionado. También se corrigió la
procedencia del guard `workspace_scope_insert_guard`: su definición vigente es
V091, no V142/V170.

Revalidación tras M1 (2026-11-20): las firmas M1 de `TaskRepo`,
`WorkUnitRepo::list_repository_ids`, `TaskService::create_task_with_repositories`,
`TaskService::replace_task_repositories`, `ProjectRepositoryService::update_primary_repo`
y `UpdateProject.primary_repo_id` coinciden con la implementación actual. Se
corrige V182 y el contrato M2 por estos hallazgos del código actual:

- `workspace_scope` aún tiene `idx_workspace_scope_work_unit UNIQUE(work_unit_id)`
  (V091), que impide guardar más de un repo en un WorkUnit. V182 debe retirarlo
  y crear un índice no único por `(work_unit_id, workspace_id)`.
- `work_unit_execution_admission_guard` (V091) también fija `workspace.repo_id`
  a `task.repo_id` y exige siempre `execution.workspace_id`. V182 debe
  reemplazarlo: admitir el grupo completo listo, aceptar primary como espejo
  escalar solo si está seleccionado y permitir NULL cuando el scope excluye el
  primary. No basta con cambiar los guards de `workspace_scope` y leases.
- `workspace_lease` tiene `idx_workspace_lease_active_work_unit UNIQUE(work_unit_id)`
  (V091), mientras cada lease enlaza un solo `workspace_id`. V182 debe acotar la
  unicidad activa a `(work_unit_id, workspace_id)`. La admisión y la autoridad
  pasan de un lease a una lista de leases por Execution.
- V181 guarda rol y orden de `task_repository`, pero no el SHA de repos `context`.
  V182 añade `task_repository_context_snapshot`; el primer grupo que use cada
  repo fija su SHA y no se hace backfill ficticio desde SQLite. El pin depende
  de Task y Repo, no de la fila mutable de asociación, y sobrevive al cambio
  versionado de alcance. Esto reemplaza la frase anterior “fijado al crear la
  Task”, imposible con el esquema M1.
- `WorkUnitService::prepare_workspace` recibe `CollaborationActorSource` y
  comprueba la autoridad antes de cargar/crear un workspace. La firma M2
  propuesta sin `source` omitía esa frontera y queda corregida abajo.
- Las tablas hijas de Review Subject y Result Requirement deben quedar ligadas
  al sujeto escalar común ya existente. El guard de Result Requirement también
  debe comparar `Execution.work_unit_version` con
  `source_work_unit_version`; ambos guards comparan snapshot con igualdad
  NULL-safe, sin permitir sustituir un NULL por un digest inventado.
- M1 permite que un WorkUnit seleccione un subconjunto writable sin incluir el
  primary. Por ello `Execution.workspace_id/before_sha/after_sha` son el espejo
  escalar del primary solo cuando está dentro del scope; en caso contrario son
  NULL y `repository_results` es la evidencia completa. En los sujetos, el
  Execution y la versión son identidad común; commit, Workspace y snapshot
  escalares reflejan el primary solo si está seleccionado. El resultado común
  de Artifact conserva la semántica existente.
- La prueba de regresión de un solo repo ya no puede exigir idénticas filas SQL:
  M2 añade por diseño un grupo y una fila de resultado. Debe exigir la misma
  respuesta escalar de 0.9.1 y exactamente un workspace y resultado del repo.

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
CREATE TRIGGER workspace_group_immutable_update
BEFORE UPDATE ON workspace_group
BEGIN
    SELECT RAISE(ABORT, 'WorkspaceGroup identity and root are immutable');
END;
CREATE TRIGGER workspace_group_immutable_delete
BEFORE DELETE ON workspace_group
WHEN NOT EXISTS (
    SELECT 1 FROM task t JOIN project_deletion_guard g ON g.project_id = t.project_id
    WHERE t.id = OLD.task_id
)
BEGIN
    SELECT RAISE(ABORT, 'WorkspaceGroup is immutable outside Project teardown');
END;
ALTER TABLE workspace_scope
    ADD COLUMN workspace_group_id TEXT
        REFERENCES workspace_group(id) ON DELETE RESTRICT;
DROP INDEX IF EXISTS idx_workspace_scope_work_unit;
CREATE INDEX idx_workspace_scope_work_unit
    ON workspace_scope(work_unit_id, workspace_id)
    WHERE scope_kind = 'work_unit';
CREATE INDEX idx_workspace_scope_group
    ON workspace_scope(workspace_group_id, workspace_id)
    WHERE workspace_group_id IS NOT NULL;

-- V181 has no Git SHA for a context repository. Pin on first group preparation;
-- all later WorkUnit groups for the Task reuse it, even across a repo-set
-- replan. The insert guard checks the current context association.
CREATE TABLE task_repository_context_snapshot (
    task_id      TEXT NOT NULL,
    repo_id      TEXT NOT NULL REFERENCES repo(id) ON DELETE RESTRICT,
    commit_sha   TEXT NOT NULL CHECK (length(trim(commit_sha)) > 0),
    captured_at  TEXT NOT NULL,
    PRIMARY KEY (task_id, repo_id),
    FOREIGN KEY (task_id) REFERENCES task(id) ON DELETE CASCADE
);
CREATE TRIGGER task_repository_context_snapshot_insert_guard
BEFORE INSERT ON task_repository_context_snapshot
WHEN NOT EXISTS (
    SELECT 1 FROM task_repository tr
    WHERE tr.task_id = NEW.task_id AND tr.repo_id = NEW.repo_id
      AND tr.role = 'context'
)
BEGIN
    SELECT RAISE(ABORT, 'context snapshot must bind a context repository of its Task');
END;
CREATE TRIGGER task_repository_context_snapshot_immutable_update
BEFORE UPDATE ON task_repository_context_snapshot
BEGIN
    SELECT RAISE(ABORT, 'context repository snapshot is immutable');
END;
CREATE TRIGGER task_repository_context_snapshot_immutable_delete
BEFORE DELETE ON task_repository_context_snapshot
WHEN NOT EXISTS (
    SELECT 1 FROM task t JOIN project_deletion_guard g ON g.project_id = t.project_id
    WHERE t.id = OLD.task_id
)
BEGIN
    SELECT RAISE(ABORT, 'context repository snapshot is immutable outside Project teardown');
END;

-- One active lease per WorkUnit/repository Workspace; retain the legacy
-- Task-wide lease uniqueness for non-WorkUnit records.
DROP INDEX IF EXISTS idx_workspace_lease_active_work_unit;
CREATE UNIQUE INDEX idx_workspace_lease_active_work_unit
    ON workspace_lease(work_unit_id, workspace_id)
    WHERE status = 'active' AND work_unit_id IS NOT NULL;

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
      AND (e.work_unit_id IS NULL AND EXISTS (
               SELECT 1 FROM workspace_scope ws
               WHERE ws.workspace_id = w.id AND ws.task_id = e.task_id
                 AND ws.scope_kind = 'integration'
           ) OR e.work_unit_id IS NOT NULL AND EXISTS (
               SELECT 1 FROM workspace_scope ws
               WHERE ws.workspace_id = w.id AND ws.task_id = e.task_id
                 AND ws.scope_kind = 'work_unit'
                 AND ws.work_unit_id = e.work_unit_id
                 AND (ws.workspace_group_id IS NULL OR EXISTS (
                     SELECT 1 FROM workspace_group g
                     WHERE g.id = ws.workspace_group_id
                       AND g.task_id = e.task_id AND g.work_unit_id = e.work_unit_id
                 ))
           ))
)
BEGIN
    SELECT RAISE(ABORT, 'repository result must bind the exact Execution, writable repo, and workspace');
END;
CREATE TRIGGER execution_repository_result_immutable_update
BEFORE UPDATE ON execution_repository_result
BEGIN
    SELECT RAISE(ABORT, 'Execution repository results are immutable');
END;
CREATE TRIGGER execution_repository_result_immutable_delete
BEFORE DELETE ON execution_repository_result
WHEN NOT EXISTS (
    SELECT 1 FROM task t JOIN project_deletion_guard g ON g.project_id = t.project_id
    WHERE t.id = OLD.task_id
)
BEGIN
    SELECT RAISE(ABORT, 'Execution repository results are immutable outside Project teardown');
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
    JOIN work_unit_review_subject scalar_subject
      ON scalar_subject.review_work_unit_id = NEW.review_work_unit_id
     AND scalar_subject.task_id = NEW.task_id
     AND scalar_subject.reviewed_work_unit_id = NEW.reviewed_work_unit_id
     AND scalar_subject.reviewed_work_unit_version IS NEW.reviewed_work_unit_version
     AND scalar_subject.result_execution_id IS NEW.result_execution_id
    JOIN task_repository tr
      ON tr.task_id = NEW.task_id AND tr.repo_id = NEW.repo_id
     AND tr.role IN ('primary', 'affected')
    WHERE review_wu.id = NEW.review_work_unit_id
      AND review_wu.task_id = NEW.task_id
      AND (NOT EXISTS (
          SELECT 1 FROM work_unit_repository scope
          WHERE scope.work_unit_id = NEW.reviewed_work_unit_id
      ) OR EXISTS (
          SELECT 1 FROM work_unit_repository scope
          WHERE scope.work_unit_id = NEW.reviewed_work_unit_id
            AND scope.repo_id = NEW.repo_id
      ))
      AND (NEW.result_execution_id IS NULL OR EXISTS (
          SELECT 1 FROM execution e
          WHERE e.id = NEW.result_execution_id AND e.task_id = NEW.task_id
            AND e.work_unit_id = NEW.reviewed_work_unit_id
            AND e.status = 'completed'
            AND e.work_unit_version = NEW.reviewed_work_unit_version
            AND EXISTS (
                SELECT 1 FROM execution_repository_result rr
                WHERE rr.execution_id = e.id AND rr.task_id = e.task_id
                  AND rr.repo_id = NEW.repo_id
                  AND rr.workspace_id = NEW.result_workspace_id
                  AND rr.after_sha IS NEW.result_commit_sha
                  AND rr.snapshot_digest IS NEW.workspace_snapshot_digest
            )
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
CREATE TRIGGER work_unit_review_subject_repository_freeze_update
BEFORE UPDATE ON work_unit_review_subject_repository
WHEN OLD.reviewed_work_unit_version IS NOT NULL OR OLD.result_execution_id IS NOT NULL
  OR NEW.review_work_unit_id IS NOT OLD.review_work_unit_id
  OR NEW.task_id IS NOT OLD.task_id OR NEW.repo_id IS NOT OLD.repo_id
  OR NEW.reviewed_work_unit_id IS NOT OLD.reviewed_work_unit_id
  OR NEW.created_at IS NOT OLD.created_at
  OR NEW.reviewed_work_unit_version IS NULL OR NEW.result_execution_id IS NULL
  OR NOT EXISTS (
      SELECT 1 FROM work_unit_review_subject scalar_subject
      WHERE scalar_subject.review_work_unit_id = NEW.review_work_unit_id
        AND scalar_subject.task_id = NEW.task_id
        AND scalar_subject.reviewed_work_unit_id = NEW.reviewed_work_unit_id
        AND scalar_subject.reviewed_work_unit_version IS NEW.reviewed_work_unit_version
        AND scalar_subject.result_execution_id IS NEW.result_execution_id
  )
  OR NOT EXISTS (
      SELECT 1 FROM execution e
      JOIN execution_repository_result rr
        ON rr.execution_id = e.id AND rr.task_id = e.task_id AND rr.repo_id = NEW.repo_id
      WHERE e.id = NEW.result_execution_id AND e.task_id = NEW.task_id
        AND e.work_unit_id = NEW.reviewed_work_unit_id AND e.status = 'completed'
        AND e.work_unit_version = NEW.reviewed_work_unit_version
        AND rr.workspace_id = NEW.result_workspace_id
        AND rr.after_sha IS NEW.result_commit_sha
        AND rr.snapshot_digest IS NEW.workspace_snapshot_digest
  )
BEGIN
    SELECT RAISE(ABORT, 'review subject repo may only freeze its exact completed repository result');
END;
CREATE TRIGGER work_unit_review_subject_repository_immutable_delete
BEFORE DELETE ON work_unit_review_subject_repository
WHEN NOT EXISTS (
    SELECT 1 FROM task t JOIN project_deletion_guard g ON g.project_id = t.project_id
    WHERE t.id = OLD.task_id
)
BEGIN
    SELECT RAISE(ABORT, 'Review subject repository is immutable outside Project teardown');
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
     AND e.work_unit_version = NEW.source_work_unit_version
     AND e.status = 'completed'
    JOIN execution_repository_result rr
      ON rr.execution_id = e.id AND rr.task_id = e.task_id
     AND rr.repo_id = NEW.repo_id
    WHERE wu.id = NEW.work_unit_id AND wu.task_id = NEW.task_id
      AND rr.workspace_id = NEW.result_workspace_id
      AND rr.after_sha IS NEW.result_commit_sha
      AND rr.snapshot_digest IS NEW.workspace_snapshot_digest
      AND EXISTS (
          SELECT 1 FROM work_unit_result_requirement scalar_requirement
          WHERE scalar_requirement.work_unit_id = NEW.work_unit_id
            AND scalar_requirement.task_id = NEW.task_id
            AND scalar_requirement.source_work_unit_id = NEW.source_work_unit_id
            AND scalar_requirement.source_work_unit_version = NEW.source_work_unit_version
            AND scalar_requirement.result_execution_id = NEW.result_execution_id
      )
      AND (NEW.result_workspace_id IS NULL OR EXISTS (
          SELECT 1 FROM workspace w
          WHERE w.id = NEW.result_workspace_id AND w.task_id = NEW.task_id
            AND w.repo_id = NEW.repo_id
      ))
)
BEGIN
    SELECT RAISE(ABORT, 'result requirement repo must bind exact same-Task WorkUnits and result');
END;
CREATE TRIGGER work_unit_result_requirement_repository_immutable_update
BEFORE UPDATE ON work_unit_result_requirement_repository
BEGIN
    SELECT RAISE(ABORT, 'WorkUnit result requirement repositories are immutable');
END;
CREATE TRIGGER work_unit_result_requirement_repository_immutable_delete
BEFORE DELETE ON work_unit_result_requirement_repository
WHEN NOT EXISTS (
    SELECT 1 FROM task t JOIN project_deletion_guard g ON g.project_id = t.project_id
    WHERE t.id = OLD.task_id
)
BEGIN
    SELECT RAISE(ABORT, 'WorkUnit result requirement repositories are immutable outside Project teardown');
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
       rr.after_sha, rr.workspace_id, rr.snapshot_digest,
       s.created_at
FROM work_unit_review_subject s
JOIN workspace w ON w.id = s.result_workspace_id AND w.task_id = s.task_id
JOIN execution_repository_result rr
  ON rr.execution_id = s.result_execution_id AND rr.task_id = s.task_id
 AND rr.repo_id = w.repo_id AND rr.workspace_id = s.result_workspace_id
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
       rr.after_sha, rr.workspace_id, rr.snapshot_digest,
       r.created_at
FROM work_unit_result_requirement r
JOIN workspace w ON w.id = r.result_workspace_id AND w.task_id = r.task_id
JOIN execution_repository_result rr
  ON rr.execution_id = r.result_execution_id AND rr.task_id = r.task_id
 AND rr.repo_id = w.repo_id AND rr.workspace_id = r.result_workspace_id
JOIN task_repository tr
  ON tr.task_id = r.task_id AND tr.repo_id = w.repo_id
 AND tr.role IN ('primary', 'affected');
~~~

V182 reemplaza `workspace_scope_insert_guard` (V091; no hay reemplazo suyo en
V142/V170): verifica repo asociado a la Task, solo permite context como
Workspace de solo lectura, y limita un WorkUnit escritor a repos primary/
affected y a su scope vacío-o-explícito. Un WorkUnit reviewer solo puede tener
scopes read-only para los repos de su Review Subject congelado. También verifica
que `workspace_group_id` pertenezca al mismo Task y WorkUnit. Reemplaza
`work_unit_execution_admission_guard` (V091), que hoy exige siempre el repo
primary y un solo Workspace: la admisión de un escritor requiere el grupo
completo listo, todas sus filas scope exactas y un lease por repo writable;
`execution.workspace_id` es el Workspace primary solo cuando está seleccionado.
La admisión de un reviewer exige sus repos sujetos exactos, `workspace_id = NULL`
y cero leases. Se reemplaza la unicidad
`idx_workspace_scope_work_unit` por un índice no único: un WorkUnit contiene
una fila scope por repo. Reemplaza también `workspace_lease_scope_guard_insert`
y `workspace_lease_active_renewal_guard` (cuerpos vigentes V120), además de
`workspace_lease_work_unit_binding_guard` y
`workspace_lease_work_unit_active_renewal_guard` (V091): cada uno conserva sus
checks actuales de Actor, RoleMembership, TaskRole, Execution, lease,
provider-start, workspace listo y capability digest, pero resuelve repo desde
`workspace.repo_id` + `task_repository` y el scope del WorkUnit, nunca desde
`task.repo_id`. Cada workspace writable del grupo recibe un lease ligado al
mismo Execution; el guard exige que `execution.workspace_id` sea el workspace
primary del grupo si el scope lo incluye y permite NULL si no lo incluye. Los
context no reciben WorkspaceLease. `workspace_lease_active_renewal_guard` y
`workspace_lease_work_unit_binding_guard` validan que cada workspace del lease
pertenezca al grupo exacto. Se conserva la unicidad activa legacy por Task y
`idx_workspace_lease_active_work_unit` pasa a ser único por
`(work_unit_id, workspace_id)`. Se conservan `workspace_lease_legacy_scope_guard`,
`workspace_lease_scope_pair_guard`, los guards de binding inmutable y los
guards de inmutabilidad/terminalidad, además de
`workspace_scope_immutable_update`. Los grupos históricos quedan sin
`workspace_group_id`; V182 no construye grupos ni contexto retrospectivo.

**V183 — M3: operaciones y PR por repo.**

> **Errata congelada M0:** este bloque registra la propuesta inicial, no es el
> SQL aprobable de V183. La lista `DROP TRIGGER` de abajo pierde triggers
> cross-table y la recreación no recompone los guards attached. La sección
> `## M3 — planning/freeze` al final de este documento sustituye la definición
> de V183 y sus afirmaciones de trigger/index. No implementar desde este bloque.

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
-- Keep cross-table trigger SQL and inbound FKs naming pr_metadata. With
-- foreign_keys=OFF and legacy_alter_table=ON, renaming the old table does not
-- rewrite their references. The old attached triggers follow the old table
-- and are removed when it is dropped; the cross-table triggers remain.
ALTER TABLE pr_metadata RENAME TO pr_metadata_legacy;
ALTER TABLE pr_metadata_v183 RENAME TO pr_metadata;
DROP TABLE pr_metadata_legacy;
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

> **Errata congelada M0:** los párrafos descriptivos que siguen a este SQL
> pertenecen al mismo borrador histórico y también quedan supersedidos. En
> particular, no es cierto que el borrador preserve/recree todos los guards,
> que baste `task.repo_id` para backfillear admission, que el índice
> `idx_pr_metadata_task_repo` sea el final, ni que todos los triggers deban
> eliminarse. La sección `## M3 — planning/freeze` al final fija el catálogo,
> el rebuild rename/swap y las invariantes vigentes.

V183 crea `task_repository_merge_batch` para agregar el estado de merge de Tasks
multirepo y `task_repository_merge_batch_repo` como conjunto ordenado de repos
que sí cambiaron; repos sin cambios no generan fila. Un batch nuevo lleva
digest de 64 chars; el NULL de un batch histórico preserva el estado previo sin
inventar digest. TaskMerge y PublishPr de un Task multirepo llevan el
`repository_merge_batch_id` exacto; una operación histórica single-repo puede
seguir sin batch y conserva `TaskMerge.id` como `Merging.reason_ref`. Los
eventos por repo no reemplazan TaskLifecycle.
El batch llega a `completed` cuando cada fila está integrada localmente o su
PR está mergeada; `ready_to_merge` exige PRs abiertas y admitidas. Un
conflict/closed bloquea el Task y la recuperación opera solo sobre filas
pendientes del mismo batch.

V183 copia todas las filas y las 15 columnas actuales de `pr_metadata`:
`id, task_id, provider_type, provider_pr_id, pr_url, source_branch,
target_branch, pr_state, merge_status, last_synced_at, created_at, updated_at,
task_merge_operation_id, publish_operation_id, admission_status`; añade
`repo_id` desde `task.repo_id` y reemplaza solo `task_id UNIQUE` por
`UNIQUE(task_id, repo_id)`. Repo NULL histórico permanece NULL; el flujo
multirepo exige repo exacto y asociación primary/affected. La ruta legacy
single-repo sin batch puede omitir `repo_id`, que se resuelve desde `task.repo_id`
para sus checks de autoridad.
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
asociarse a un repo. La escritura legacy single-repo sin batch también puede
omitir `repo_id`; el guard la limita a `task.repo_id` y rechaza Tasks multirepo.

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
`pr_metadata_repo_insert_guard` (repo exacto del mismo Project y role
primary/affected para multirepo; la escritura legacy single-repo sin batch
puede dejar repo NULL y resolverlo desde `task.repo_id`) y
`pr_metadata_repo_immutable`.

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
de metadata/admisión; rework exige metadata y admisión del mismo repo y la
causa exacta de Merging (`batch.id` con batch y `merge_op.id` para la ruta
single-repo legacy sin batch); resultado del proveedor y cierre exigen
`metadata.repo_id = operation.repo_id` y esa misma causa exacta. El insert
guard de admisión conserva sus checks V175
de TaskMerge/PublishPr, metadata, remote identity, config, credential/policy
snapshots y rework; exige además `NEW.repo_id = merge_op.repo_id =
publish_op.repo_id = metadata.repo_id = config.repo_id`, asociación
primary/affected y la causa de Merging que corresponde a la operación.

Los tres guards adicionales se reemplazan así: una TaskMerge multirepo exige
`repo_id` primary/affected y su fila de batch; la ruta single-repo legacy puede
omitir batch y resolver el repo primario de Task. PublishPr exige el mismo
`repo_id` y `repository_merge_batch_id` que su parent (ambos nulos en la ruta
single-repo legacy), Gate satisfecho exacto y la causa Merging correspondiente;
feedback rework conserva las transiciones D3' y enlaza la admisión al repo/batch
exacto. `repo_id` y
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
`TaskRepositoryContextSnapshot { task_id: String, repo_id: String,
commit_sha: String, captured_at: String }` y
`CreateTaskRepositoryContextSnapshot` con los mismos campos;
`ExecutionRepositoryResult {
execution_id: String, task_id: String, repo_id: String, workspace_id: String,
before_sha: Option<String>, after_sha: Option<String>,
snapshot_digest: Option<String>, created_at: String }`;
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
`WorkspaceScope.workspace_group_id: Option<String>` une cada Workspace nuevo a
su grupo; los scopes históricos quedan NULL. `WorkUnitRepo` crea de forma
atómica el sujeto escalar y sus filas repo; las filas de resultado de Review
se congelan una vez desde el Execution exacto.

| Fase | Modelos, métodos y firmas congelados | Archivos exactos |
|---|---|---|
| M1 | `TaskRepository { task_id: String, repo_id: String, role: TaskRepositoryRole, position: i64, created_at: String }`; `TaskRepositoryWrite { repo_id: String, role: TaskRepositoryRole }`; `WorkUnitRepositoryScope { work_unit_id: String, task_id: String, repo_id: String, created_at: String }`. `TaskRepo::list_repositories(&self, task_id: &str) -> db::Result<Vec<TaskRepository>>`; `replace_repositories_in_tx(&self, tx: &mut Transaction<'_, Sqlite>, task_id: &str, expected_version: i64, rows: &[TaskRepositoryWrite], updated_at: &str) -> db::Result<Task>`. `WorkUnitRepo::list_repository_ids(&self, work_unit_id: &str) -> db::Result<Vec<String>>`; `CreateTask.repositories: Vec<TaskRepositoryWrite>` y `CreateWorkUnit.repository_ids: Vec<String>` se materializan en las transacciones existentes. `TaskService::create_task_with_repositories(&self, project_id: impl Into<String>, title: impl Into<String>, description: Option<String>, parent_task_id: Option<String>, priority: Option<i64>, repositories: Option<Vec<TaskRepositoryWrite>>, initial_lifecycle_state: Option<TaskLifecycleState>, merge_config: Option<Value>) -> services::Result<Task>`; `replace_task_repositories(&self, task_id: &str, expected_version: i64, repositories: Vec<TaskRepositoryWrite>) -> services::Result<Task>` (409 tras Execution, salvo replan autorizado). `UpdateProject.primary_repo_id: Option<Option<String>>`; `ProjectRepositoryService::update_primary_repo(&self, project_id: &str, expected_version: i64, primary_repo_id: Option<String>, caller_user_id: &str) -> services::Result<Project>`; mantiene `ProjectRepo::update_at_version`. | `crates/db/src/models.rs`, `crates/db/src/repository.rs`, `crates/db/src/sqlite/task.rs`, `crates/db/src/sqlite/project.rs`, `crates/db/src/sqlite/work_unit.rs`, `crates/services/src/task_service/create.rs`, nuevo `crates/services/src/task_service/repositories.rs`, `crates/services/src/task_service.rs`, nuevo `crates/services/src/project_repository_service.rs`, `crates/services/src/lib.rs`, `crates/services/src/work_unit_service.rs`. |
| M2 | `WorkspaceGroup { id, task_id, work_unit_id, root_path, created_at }`; `WorkspaceScope.workspace_group_id: Option<String>`; `TaskRepositoryContextSnapshot { task_id, repo_id, commit_sha, captured_at }`; `ExecutionRepositoryResult { execution_id, task_id, repo_id, workspace_id, before_sha: Option<String>, after_sha: Option<String>, snapshot_digest: Option<String>, created_at }`; `CreateWorkUnitWorkspace.workspace_group_id: String`; `CreateWorkUnitExecution.workspace_leases: Vec<CreateWorkspaceLease>` (sustituye el actual `Option` singular). `WorkspaceRepo::create_group_for_work_unit(&self, input: CreateWorkspaceGroup) -> db::Result<WorkspaceGroup>` y `get_group_by_work_unit_id(&self, work_unit_id: &str) -> db::Result<Option<WorkspaceGroup>>`; `WorkUnitWorkspaceRepo::list_by_work_unit_id(&self, work_unit_id: &str) -> db::Result<Vec<Workspace>>`; `WorkspaceLeaseRepo::list_active_for_work_unit(&self, work_unit_id: &str) -> db::Result<Vec<WorkspaceLease>>` y `get_active_for_execution_workspace(&self, execution_id: &str, workspace_id: &str) -> db::Result<Option<WorkspaceLease>>`; `TaskRepo::pin_context_repository_snapshot(&self, input: CreateTaskRepositoryContextSnapshot) -> db::Result<TaskRepositoryContextSnapshot>` (insert-or-return del primer pin); `ExecutionRepo::record_repository_results(&self, execution_id: &str, rows: &[ExecutionRepositoryResult]) -> db::Result<Vec<ExecutionRepositoryResult>>` y `list_repository_results(&self, execution_id: &str) -> db::Result<Vec<ExecutionRepositoryResult>>`; `WorkUnitRepo::create_with_review_subject_repositories(&self, input: CreateWorkUnit, subject: CreateWorkUnitReviewSubject, repositories: &[WorkUnitReviewSubjectRepository], event: CreateDomainEvent) -> db::Result<CollaborationWrite<WorkUnit>>`, `list_review_subject_repositories(&self, work_unit_id: &str) -> db::Result<Vec<WorkUnitReviewSubjectRepository>>`, `freeze_review_subject_repositories(&self, review_work_unit_id: &str, rows: &[WorkUnitReviewSubjectRepository]) -> db::Result<Vec<WorkUnitReviewSubjectRepository>>`, `list_result_requirement_repositories(&self, work_unit_id: &str) -> db::Result<Vec<WorkUnitResultRequirementRepository>>` y `insert_result_requirement_repositories_in_tx(&self, tx: &mut Transaction<'_, Sqlite>, rows: &[WorkUnitResultRequirementRepository]) -> db::Result<()>`. `WorkUnitService::prepare_workspace_group(&self, source: CollaborationActorSource, work_unit_id: &str) -> services::Result<WorkspaceGroup>`; `TaskService::record_execution_repository_results(&self, execution_id: &str) -> services::Result<Vec<ExecutionRepositoryResult>>`; `ValidationService::run_for_repository(&self, task_id: &str, repo_id: &str, check_key: &str, workspace_id: &str, commit_sha: &str) -> services::Result<ValidationRun>`. `EnqueueCheckRequest.repo_id: String` y `ValidationSubject.repo_id: String` seleccionan el miembro exacto de `(repo_id, check_key)`; `ValidationRunResponse` añade `repo_id: String` resuelto del Workspace exacto. | `crates/db/migrations/V182__multirepo_workspaces_results.sql`, `crates/db/src/models.rs`, `crates/db/src/repository.rs`, `crates/db/src/sqlite/task.rs`, `crates/db/src/sqlite/workspace.rs`, `crates/db/src/sqlite/work_unit.rs`, `crates/db/src/sqlite/workspace_lease.rs`, `crates/db/src/sqlite/execution.rs`, `crates/db/src/tests.rs`, `crates/services/src/task_service/workspace.rs`, `crates/services/src/task_service/execution/runner.rs`, `crates/services/src/work_unit_service.rs`, `crates/services/src/validation_service.rs`, `crates/services/src/validation_check_runner.rs`, `crates/services/src/task_coordination/turn_outputs.rs`, `crates/services/src/validation_post_integration.rs`, `crates/services/src/coordinator_outputs/validation_request.rs`, `crates/services/src/coordinator_outputs/tests.rs`, `crates/services/src/collaboration_service.rs`, `crates/api-types/src/execution.rs`, `crates/api-types/src/work_unit.rs`, `crates/api-types/src/task_dispatch.rs`, `crates/api-types/src/review_validation.rs`, `crates/api/src/routes/work_units.rs`, `crates/api/src/routes/work_unit_reviews.rs`, `crates/api/src/routes/reviews.rs`. |
| M3 | `TaskRepositoryMergeBatch { id: String, task_id: String, gate_evaluation_id: String, repository_set_revision: i64, repository_set_digest: Option<String>, state: TaskRepositoryMergeBatchState, version: i64, created_at: String, updated_at: String }`; `TaskRepositoryMergeBatchRepoRow { batch_id: String, task_id: String, repository_set_revision: i64, repo_id: String, position: i64, state: TaskRepositoryMergeBatchRepoState, work_unit_integration_id: Option<String>, task_merge_operation_id: Option<String>, pr_metadata_id: Option<String>, before_sha: Option<String>, after_sha: Option<String>, created_at: String, updated_at: String }`. `TaskIntegrationOperation` gains `repo_id: Option<String>` y `repository_merge_batch_id: Option<String>`; `PrMetadata.repo_id: Option<String>` y `RemotePrAdmission.repo_id: Option<String>` (NULL para datos históricos y escrituras legacy single-repo sin batch, que resuelven el repo primario desde la Task; el flujo multirepo requiere repo y batch exactos). `TaskRepositoryMergeBatchRepo::create(&self, batch: &TaskRepositoryMergeBatch, repos: &[TaskRepositoryMergeBatchRepoRow]) -> db::Result<TaskRepositoryMergeBatch>`; `transition_repo(&self, batch_id: &str, repo_id: &str, expected_version: i64, state: TaskRepositoryMergeBatchRepoState, work_unit_integration_id: Option<&str>, task_merge_operation_id: Option<&str>, pr_metadata_id: Option<&str>, before_sha: Option<&str>, after_sha: Option<&str>) -> db::Result<TaskRepositoryMergeBatchRepoRow>`. `PrMetadataRepo::list_by_task(&self, task_id: &str) -> db::Result<Vec<PrMetadata>>`; `get_by_task_repo(&self, task_id: &str, repo_id: &str) -> db::Result<Option<PrMetadata>>`; retires ambiguous `get_by_task_id`. `TaskIntegrationOperationManager::integrate_repositories(&self, task_id: &str, gate_evaluation_id: &str, ordered_repo_ids: &[String], idempotency_key: &str) -> services::Result<Vec<TaskRepositoryIntegrationResult>>`; `PrService::list_task_prs(&self, task_id: &str) -> services::Result<Vec<TaskRepositoryPrStatus>>`; `GateEngine::evaluate_required_checks_by_repo(&self, task_id: &str, repository_results: &[ExecutionRepositoryResult]) -> services::Result<GateEvaluation>`; `ProjectPlanService::verify_handoffs_by_repository(&self, predecessor_task_id: &str, successor_task_id: &str) -> services::Result<Vec<ProjectPlanRepositoryHandoff>>`. En multirepo, TaskLifecycle Merging fija `reason_kind = "task_repository_merge_batch"`, `reason_ref = batch.id`; la ruta legacy single-repo sin batch conserva `TaskMerge.id` como `reason_ref`. | `crates/db/src/models.rs`, `crates/db/src/repository.rs`, `crates/db/src/sqlite.rs` (registro de módulo), nuevo `crates/db/src/sqlite/task_repository_merge_batch.rs`, `crates/db/src/sqlite/task_integration_operation.rs`, `crates/db/src/sqlite/pr_metadata.rs`, `crates/services/src/task_integration_operation.rs`, `crates/services/src/merge_service.rs`, `crates/services/src/pr_service.rs`, `crates/services/src/pr_feedback.rs`, `crates/services/src/gate_engine.rs`, `crates/services/src/validation_service.rs`, `crates/services/src/project_plan.rs`, `crates/services/src/task_coordination/replan.rs`, `crates/services/src/replan_classifier.rs`, `crates/services/src/task_lifecycle.rs`, `crates/api-types/src/task_repositories.rs`, `crates/api-types/src/integration.rs`, `crates/api-types/src/pr_feedback.rs`, `crates/api-types/src/public_events.rs`, `crates/api/src/routes/tasks/gates.rs`, `crates/api/src/routes/pr_feedback.rs`, `crates/api/src/routes/events.rs`. |

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
  position: i64, change_state: TaskRepositoryChangeState,
  integration: Option<TaskRepositoryIntegrationResponse>,
  merge: Option<TaskRepositoryMergeResponse>,
  pull_request: Option<TaskRepositoryPullRequestResponse> }`; integration lleva
  `{ repo_id: String, integration_id: Option<String>, outcome: String,
  before_sha: Option<String>, after_sha: Option<String> }`; merge lleva
  `{ batch_id: String, repo_id: String, state: TaskRepositoryMergeState,
  integration_id: Option<String>, task_merge_operation_id: Option<String>,
  pr_metadata_id: Option<String>, before_sha: Option<String>,
  after_sha: Option<String> }`;
  `TaskRepositoryPullRequestResponse { repo_id: String, admission_id: Option<String>,
  pr_metadata_id: String, provider_type: String, provider_pr_id: Option<String>, pr_url: Option<String>,
  pr_state: String, merge_status: String, admission_status: String }`.
  `position` lleva `#[ts(type = "number")]`; responses derivan
  `Serialize, Deserialize, TS` + `#[ts(export)]`. Ninguno incluye credential
  refs, snapshots de política ni tokens. M3 añade enum público
  `TaskRepositoryMergeState` con `#[serde(rename_all = "snake_case")]` y
  valores `pending|integrating|integrated|publishing|open|merged|conflict|
  closed|failed`; `TaskRepositoryMergeBatchState` usa
  `integrating|publishing|open|completed|blocked|failed`.
  `MergeAfterGateResponse` en `gates.rs` conserva sus campos escalares y añade
  `repository_merge_batch_id: Option<String>` y
  `repositories: Vec<TaskRepositoryMergeResponse>`; ambos tienen
  `#[serde(default)]`. El batch completo se proyecta en `TaskResponse` con
  `TaskRepositoryMergeBatchResponse { id, task_id, gate_evaluation_id,
  repository_set_revision, state, version, repositories, created_at, updated_at }`;
  cada fila incluye `batch_id`, `repo_id`, estado, integración, operación, metadata
  PR y SHA anterior/posterior. Los campos escalares de la respuesta de merge
  siguen representando el repo primary.
- `TaskResponse.repositories: Vec<TaskRepositoryResponse>` en
  `execution.rs`, con `#[serde(default)]`; `repository_set_revision: i64`,
  `repository_set_digest: Option<String>` y el historial
  `repository_merge_batches` se exponen; `repo_id: Option<String>` sigue siendo
  el espejo primary. `CreateWorkUnitRequest.repository_ids:
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
`task.repository_merge_batch_changed` ({task_id, repo_id, repository_revision,
repository_merge_batch_id, integration_id, admission_id, state, batch_state,
version}) y `task.repository_pr_status_changed` ({task_id, repo_id,
repository_revision, repository_merge_batch_id, admission_id, pr_metadata_id,
state, admission_status, pr_state, version}). Añadir repo_id a eventos
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

## Anexo M1 — 2026-11-19

**Estado: M1 IMPLEMENTADO / NOT RUN.** La implementación y la revisión estática
están completas; la validación de ejecución queda reservada a CI.

Trabajo integrado:

- Se añadió V181, sus modelos y repositorios, los alcances de repositorios para
  Tasks y WorkUnits, la herencia de alcance al crear subtareas y la actualización
  versionada del repo principal de Project para Tasks futuras.
- Se integraron REST, MCP y web con los DTO congelados, los errores tipados y
  los campos requeridos en los literales Rust y fixtures TypeScript de todo el
  workspace; README y arquitectura reflejan ahora el esquema V181.
- Se añadió `task.repositories_changed` como runtime hint posterior al commit,
  con payload `{task_id, task_version, repositories}`, proyección SSE acotada,
  tipo público generado y documentación.
- Se revisaron los registros de rutas y herramientas, las exportaciones Rust y
  TypeScript, los mapeos de errores y las firmas de helpers. Los 11 nombres de
  test M1 del plan aparecen exactamente una vez.
- `rustfmt --edition 2021` y `git diff --check` pasaron; el barrido estático no
  encontró campos requeridos ausentes ni espacios finales.

Desviaciones y límites: una Task con Execution no admite todavía una
replanificación autorizada de repositorios; el reemplazo responde `409
task.repositories.replan_required`. No se ejecutaron builds, tests, clippy ni
comandos pnpm; su resultado es NOT RUN y no constituye aceptación de CI.

### Anexo M1 — cierre CI

`build-local-linux.yml run_checks=true` **PASS** en `feat/multirepo` (run
44662476947), tras dos vueltas de corrección: clippy (`explicit_counter_loop`,
`explicit_auto_deref`), `use sqlx::Row` en `task_service/create.rs`, mock del
editor Markdown en el test de selectores web y comparación del alcance heredado
por repo/rol/posición. M1 queda **DONE**.

### Propiedad de archivos y pruebas de M2

**Revalidación M0/M1 contra el árbol actual.** HEAD revisado:
`ac87ab85d0c466ca1e73c22bbdedfa186a065aee`; M1 está integrado y CI PASS según
el Anexo M1. Las firmas M1 de `TaskRepo`, `WorkUnitRepo::list_repository_ids`,
`TaskService::create_task_with_repositories`, `replace_task_repositories`,
`ProjectRepositoryService::update_primary_repo` y `UpdateProject.primary_repo_id`
coinciden con el código actual. M0 queda corregido en el sitio, no solo en este
anexo: V182 elimina la unicidad por WorkUnit de `workspace_scope` y de leases,
reemplaza también `work_unit_execution_admission_guard`, fija el SHA context
en la primera preparación de grupo y conserva la barrera de autorización
`CollaborationActorSource`. La respuesta escalar queda NULL para el primary si
el WorkUnit no lo seleccionó. El contexto es de solo lectura sin
`WorkspaceLease`; fallback conserva el grupo del mismo WorkUnit, mientras
rework y micro-fix crean grupos nuevos desde cada resultado repo congelado.

Los nombres y firmas M2 congelados en “Tipos Rust, repositorios y servicios”
sustituyen la firma previa de `prepare_workspace_group(task_id, work_unit_id)`:
recibe `source` antes de cargar o crear datos. `CreateWorkUnitExecution` pasa de
`workspace_lease: Option<_>` a `workspace_leases: Vec<_>`, y la lectura singular
`get_by_work_unit_id` pasa a `list_by_work_unit_id`. No se backfillea un SHA
context inventado ni se crean grupos para Workspaces históricos. V182 se
revalidó con `python3 -I` en SQLite 3.46.1: se aplicaron los 179 archivos de
migración disponibles hasta V181 y el bloque V182; `PRAGMA foreign_key_check`
devolvió cero filas, el índice de scope quedó no único y el índice activo de
leases quedó único por `(work_unit_id, workspace_id)`; se instalaron los guards
de grupo, pin context, resultados, freeze y borrado inmutable. PASS. Esto valida
la aplicación DDL, los índices y las FK, no el comportamiento de cada trigger;
ese comportamiento queda para los tests M2 indicados abajo. Cargo, pnpm, builds
y tests no se ejecutaron.

**Call paths actuales que cambia M2** (líneas del HEAD citado):

- **Preparar grupo, Workspaces y leases.**
  `WorkUnitService::prepare_coordination_workspaces`
  (`crates/services/src/work_unit_service.rs:1562`) omite actualmente la
  preparación si `get_by_work_unit_id` devuelve un Workspace (1594) y llama a
  `prepare_workspace` (1608). `WorkUnitService::prepare_workspace`
  (`work_unit_service.rs:1615`) autoriza primero mediante `source` (1620),
  prepara el integration Workspace primary (1651), lee un único
  `work_unit_result_requirement.result_commit_sha` (1663), carga un Workspace
  singular (1681) o crea su fila scope (1775). M2 lo convierte en preparación
  idempotente de todas las filas scope del grupo; cada repo writable toma su
  propio base fijado y cada repo context toma el snapshot Task inmutable.
  `WorkUnitService::bind_execution` (`work_unit_service.rs:1457`) hoy comprueba
  un Workspace (1511–1527), llama a
  `TaskService::prepare_work_unit_workspace_lease`
  (`crates/services/src/task_service/governance.rs:19`) y pasa un lease singular
  al `CreateWorkUnitExecution` en `work_unit_service.rs:1545–1555`.
  `TaskService::verify_active_workspace_lease` (governance.rs:148),
  `verify_active_work_unit_workspace_lease` (179), `workspace_lease_inputs`
  (304), `verify_execution_workspace_authority` (387) y
  `revoke_active_workspace_lease_for_execution` (349) también asumen un lease
  único y repo primary: M2 verifica, renueva y revoca el conjunto exacto por
  `(Execution, Workspace)`.
  `prepare_integration_workspace_for_work_unit`
  (`crates/services/src/task_service/workspace.rs:9`) y `create_fresh_workspace`
  (253) aún resuelven `task.repo_id`; `resolve_repo_source` (339) ya resuelve un
  Repo exacto. M2 usa este último por repo para el grupo y deja la integración
  Task para el alcance que ya tiene.
- **Raíz y ciclo de vida físico.**
  `WorkspaceManager::create_work_unit_worktree`
  (`crates/workspace/src/lib.rs:136`), `recover_work_unit_worktree` (183),
  `cleanup_work_unit_worktree` (231) y `work_unit_worktree_path` (294) crean o
  resuelven hoy una carpeta por Workspace. `WorkspaceCleanupService::cleanup_work_unit_workspace`
  (`crates/services/src/workspace_cleanup.rs:293`) valida scope (316), ruta
  exacta (373) y borra un hijo (462). M2 persiste una raíz de grupo y conserva
  la recuperación/limpieza por cada hijo; limpiar uno no puede borrar hermanos.
  El generador de `FORGE_WORKSPACE.md` y las instrucciones raíz para harnesses
  son nuevos; hoy no hay generador con ese nombre.
- **CWD del harness.**
  `TaskDispatcher::dispatch_task_once` (`crates/services/src/task_dispatcher/dispatch.rs:39`)
  prepara coordinación (85), llama `launch_work_unit_execution` (116) y
  `start_execution` (133). `TaskService::launch_work_unit_execution`
  (`crates/services/src/task_service/execution/launch.rs:362`) carga un
  Workspace singular (408–440). `TaskService::resolve_execution_working_context`
  (`crates/services/src/task_service.rs:519`) valida el lease y entrega el
  `workspace.worktree_path` (543–559); `TaskService::execution_start_params`
  (`crates/services/src/task_service/execution/runner.rs:1556`) lo envía como
  `workspace_path`. `TaskService::run_execution`
  (`crates/services/src/task_service/execution/runner.rs:438`) resuelve ese
  contexto (477) y entrega `ExecutionContext.worktree_path` a
  `TaskExecutor::execute` (957–978); hoy `has_repository_workspace` se deriva
  de `execution.workspace_id.is_some()` (968), que es falso cuando el primary
  no forma parte del scope y para un review group. M2 lo deriva del grupo y sus
  scopes exactos. El contrato atraviesa
  `HarnessAdapter::{start,execute}` (`crates/executors/src/adapter.rs:113,
  166–170`), `ExecutionContext` (crates/executors/src/lib.rs:41–53),
  `handle_request_with_terminal` (crates/forge-daemon/src/commands.rs:19,
  110–114) y `DaemonRuntime::start`
  (crates/forge-client/src/daemon_runtime.rs:369–415). Los CWD efectivos son
  `ClaudeCodeAdapter::execute` (`crates/cli-adapters/src/claude.rs:910`) y
  `run` (claude.rs:372–416), `CodexAdapter::execute`
  (`crates/cli-adapters/src/codex.rs:421–424`), `CursorAdapter::execute`
  (`crates/cli-adapters/src/cursor.rs:527–579`), `GeminiAdapter::execute`
  (`crates/cli-adapters/src/gemini.rs:399–422`), `OpencodeAdapter::execute`
  (`crates/cli-adapters/src/opencode.rs:377–464`), `SmithAdapter::execute`
  (`crates/cli-adapters/src/smith.rs:229–238`), y `ShellAdapter::execute`
  (`crates/cli-adapters/src/shell.rs:98`) → `ShellExecutor::execute`
  (`crates/executors/src/shell.rs:111–136`). Todos ya usan el campo
  `worktree_path`; M2 debe pasarles la raíz de grupo, no el hijo primary.
  `prepare_non_repository_work_unit_context`
  (`crates/services/src/task_service.rs:439`), llamado desde la rama no
  mutante de `resolve_execution_working_context` (task_service.rs:561–589),
  arranca hoy en el directorio aislado
  `.forge-workunit-context`; `review_read_paths_for_execution`
  (runner.rs:1505–1554) devuelve un solo Workspace sujeto. M2 arranca al
  reviewer en su grupo read-only y entrega solo los hijos exactos del Review
  Subject.
- **Resultados antes del cierre.**
  `TaskService::launch_work_unit_execution`
  (`crates/services/src/task_service/execution/launch.rs:362`) inicializa hoy
  `Execution.before_sha` y `workspace_id` desde ese único Workspace (524–530).
  `TaskService::run_execution` (runner.rs:438) hoy captura el SHA de un único
  worktree y detecta cambios sin commit en 1050–1079; actualiza Execution y
  emite su evento vía `ExecutionRepo::update_with_event` en 1302–1324 y después
  revoca el lease en 1329. M2 inspecciona todos los hijos antes de ese update,
  falla cerrado ante cualquier cambio context o repo Git nuevo bajo la raíz,
  y persiste un `ExecutionRepositoryResult` por repo writable antes del evento
  terminal. `execution.workspace_id/before_sha/after_sha` reflejan primary solo
  cuando forma parte del scope; los resultados por repo son la evidencia
  completa.
- **Review y requirements.**
  `WorkUnitService::create_with_request_digest`
  (`crates/services/src/work_unit_service.rs:201`) crea el review subject escalar
  en `WorkUnitRepo::create_with_review_subject` (628–647).
  `TaskReworkService::request_rework` (`crates/services/src/task_rework.rs:871`)
  inserta un result requirement y el review subject pendiente en 1457–1487.
  `review_subject_ready` (2009), `get_review_subject` (2039),
  `result_requirement_available` (2145), `populate_review_subject_result`
  (2162) y `review_subject_response` (2224) son lectores/escritores escalares.
  `TaskService::freeze_review_subject_and_inputs`
  (`crates/services/src/task_service/execution/runner.rs:36`) y
  `review_read_paths_for_execution` (1505) congelan/leen un Workspace;
  `CollaborationService::ensure_review_subject_current`
  (`crates/services/src/collaboration_service.rs:113`) y
  `create_review_report_from_execution` (671), más
  `TaskService::submit_human_review_report`
  (`crates/services/src/task_service/review.rs:351`), consumen ese sujeto.
  `TaskReworkService::process_micro_fix_result`
  (`crates/services/src/task_rework/micro_fix.rs:479`) y
  `ensure_micro_fix_review_work_unit` (922) también enlazan un único SHA,
  Workspace y subject (998–1037). M2 crea/congela/expone hijos por repo; el
  ReviewReport sigue siendo uno.
- **Selección de Workspace de validación.**
  `resolve_subject` (`crates/services/src/validation_check_runner.rs:75`)
  elige hoy `get_by_work_unit_id` (100) para `WorkUnitResult` o el integration
  Workspace único (112), y toma HEAD/digest en 122–124.
  `RequestValidationOutput` y `ValidationSubject`
  (`crates/services/src/coordinator_outputs/validation_request.rs:47–63`) hoy no
  llevan repo; `apply_request_validation`
  (`crates/services/src/task_coordination/turn_outputs.rs:184–209`),
  `enqueue_recheck` (217, 262–277) y `enqueue_after_integration`
  (`crates/services/src/validation_post_integration.rs:25, 57–71`) construyen
  `EnqueueCheckRequest` sin `repo_id`. M2 añade el repo requerido.
  `enqueue_check_request` (`crates/services/src/validation_check_runner.rs:138`)
  fija el sujeto; `process_request` (401) carga `request.workspace_id` (426) y
  ejecuta `ValidationService::run_check_core`
  (`crates/services/src/validation_service.rs:115`) con el commit/digest exactos
  (validation_check_runner.rs:500–519). M2 resuelve `(repo_id, check_key)` al
  Workspace/commit de ese resultado y conserva el pin hasta el fin del run; el
  repo queda ligado al Workspace exacto guardado en la request.
- **Respuestas REST/MCP/web.**
  `execution_response` (`crates/api/src/routes/mod.rs:431`) y
  `execution_response_with_usage` (467) proyectan hoy solo escalares;
  `response_for` de WorkUnit (`crates/api/src/routes/work_units.rs:102–124`)
  carga el sujeto escalar mediante `get_review_subject` (115–119), y
  `validation_run_response` (`crates/api/src/routes/reviews.rs:130–152`) arma
  el DTO sin repo. `forge_get_task` (`crates/mcp-server/src/tools/handlers.rs:286`)
  y `forge_get_work_unit` (656) proyectan esos mismos DTOs. M2 hidrata
  `repository_results` desde repos DB exactos; C añade
  `execution_response_with_repository_results(db: &db::SqliteDb,
  execution: Execution) -> ApiResult<ExecutionResponse>` y hace que todos los
  response paths lo usen. Web consume esa lista en
  `ExecutionDetailSidebar` (execution-detail/ExecutionDetailSidebar.tsx:155),
  el panel de review y el de validation; nunca infiere repo por `task.repo_id`.
- **Fallback, rework y micro-fix.**
  `process_work_unit_fallbacks`
  (`crates/services/src/task_dispatcher/fallback.rs:61`),
  `process_unavailable_work_unit_execution` (135) y `activate_fallback` (612)
  reasignan Actor conservando el mismo WorkUnit: vuelven a usar su grupo ya
  preparado. `request_rework` (task_rework.rs:871) y
  `process_micro_fix_result` / `ensure_micro_fix_review_work_unit`
  (micro_fix.rs:479, 922) crean WorkUnits nuevos; su grupo nuevo deriva la base
  de cada repo de `work_unit_result_requirement_repository`, nunca del último
  Execution ni del repo primary por defecto.

**Lanes M2: propiedad exclusiva por archivo.** No hay ownership compartido.
B posee `work_unit_service.rs` y `task_service/execution/runner.rs` completos;
C consume sus contratos sin editar esos archivos. A implementa primero la
superficie DB indicada en la tabla M2 de arriba.

| Lane | Alcance y archivos exclusivos |
|---|---|
| **A — DB/V182** | Migración `crates/db/migrations/V182__multirepo_workspaces_results.sql`; `crates/db/src/models.rs`, `repository.rs`, `sqlite/task.rs`, `sqlite/workspace.rs`, `sqlite/work_unit.rs`, `sqlite/workspace_lease.rs`, `sqlite/execution.rs`, `tests.rs`. Implementa los índices y guards V182, snapshot context, grupo/scope, leases por repo, resultados y tablas hijas de Review/Requirement, con row mappers y métodos exactos de M0. No toca servicios ni contratos públicos. |
| **B — grupo, autoridad y Execution** | `crates/workspace/src/lib.rs`; `crates/services/src/work_unit_service.rs`; `crates/services/src/task_service.rs`, `task_service/workspace.rs`, `task_service/governance.rs`, `task_service/execution/launch.rs`, `task_service/execution/runner.rs`; `crates/services/src/task_dispatcher/dispatch.rs`, `task_dispatcher/fallback.rs`; `crates/services/src/workspace_cleanup.rs`, `recovery.rs`; `crates/executors/src/adapter.rs`, `lib.rs`, `shell.rs`; `crates/cli-adapters/src/claude.rs`, `codex.rs`, `cursor.rs`, `gemini.rs`, `opencode.rs`, `shell.rs`, `smith.rs`; `crates/forge-daemon/src/commands.rs`; `crates/forge-client/src/daemon_runtime.rs`; prueba de ejecución en `crates/services/src/task_service/tests/service_tests/cases/executions.rs` y prueba inline de fallback en `task_dispatcher/fallback.rs`. B implementa `WorkUnitService::prepare_workspace_group(&self, source: CollaborationActorSource, work_unit_id: &str) -> services::Result<WorkspaceGroup>` y `TaskService::record_execution_repository_results(&self, execution_id: &str) -> services::Result<Vec<ExecutionRepositoryResult>>`; registra resultados antes del evento terminal. B también adapta la creación directa de review subject en `work_unit_service.rs` para llamar el método DB atómico de A. |
| **C — resultados derivados y superficies** | `crates/services/src/task_rework.rs`, `task_rework/micro_fix.rs`, `task_rework/validation_triage.rs`, `validation_check_runner.rs`, `validation_service.rs`, `validation_post_integration.rs`, `task_coordination/turn_outputs.rs`, `coordinator_outputs/validation_request.rs`, `coordinator_outputs/tests.rs`, `collaboration_service.rs`, `task_service/review.rs`; `crates/api-types/src/execution.rs`, `work_unit.rs`, `task_dispatch.rs`, `review_validation.rs`, `public_events.rs`, `lib.rs`; `crates/api/src/routes/mod.rs`, `executions.rs`, `work_units.rs`, `work_unit_reviews.rs`, `reviews.rs`, `tasks/reviews.rs`, `events.rs`; `crates/api/tests/happy_path.rs`, `work_units.rs`, `work_unit_reviews.rs`; `crates/mcp-server/src/tools/descriptors.rs`, `tools/handlers.rs`, `tests.rs`; `web/src/api/hooks.ts`, `pages/TaskDetailPage.tsx`, `components/execution-detail/ExecutionDetailSidebar.tsx`, `components/task-detail/task-validation-panel.tsx`, `task-verification.ts`, `task-verification.test.ts`, `work-unit-review-panel.tsx`, `work-unit-review-panel.test.tsx`, `web/src/types/generated/`; `.github/workflows/build-local-linux.yml`, `CHANGELOG.md`, `docs/api.md`, `docs/concepts/executions.md`, `docs/concepts/work-units.md`, `docs/concepts/review-and-validation.md`. C publica `ExecutionResponse.repository_results`, resultados de subject por repo, `ValidationRunResponse.repo_id` y el evento `execution.repository_results_recorded`; valida cada check contra un Workspace y commit exactos. |

**Frontera entre B y C.** B es dueño de toda la preparación/admisión en
`work_unit_service.rs` y del congelado de inputs en `runner.rs`; C llama
`WorkUnitService::prepare_workspace_group(source, work_unit_id)`, consume
`ExecutionRepo::list_repository_results(execution_id)` y
`WorkUnitRepo::{list_review_subject_repositories,
list_result_requirement_repositories}` de A, y modifica solo sus archivos.
Para crear subjects por repo, B y C llaman
`WorkUnitRepo::create_with_review_subject_repositories(input, subject,
repositories, event)` de A para altas atómicas y usa
`insert_result_requirement_repositories_in_tx(tx, rows)` en la transacción de
rework; C no añade SQL de escritura al archivo de B. La congelación de hijos de
Review Subject usa `freeze_review_subject_repositories(review_work_unit_id,
rows)` de A. El fallback de B mantiene el grupo del WorkUnit exacto; los
WorkUnits nuevos de C reciben grupos propios basados en sus filas de result
requirement.

**Tests M2 exactos y ubicación.** Cada nombre empieza por uno de los dos
prefijos congelados; los casos requeridos son obligatorios:

- DB: `multirepo_workspace_v182_allows_multiple_scopes_and_active_repo_leases`
  y `multirepo_workspace_context_snapshot_pins_once_and_is_immutable` —
  `crates/db/src/tests.rs`; `multirepo_result_rows_require_exact_task_repo_and_workspace`
  y `multirepo_result_review_and_requirement_rows_bind_exact_execution` —
  mismo archivo; rechaza versión de WorkUnit distinta, huérfanos del padre
  escalar y digest distinto incluso cuando el resultado guarda NULL.
- Grupo con **dos repos writable y uno context**:
  `multirepo_workspace_group_prepares_two_writable_and_one_context_repo` —
  `crates/services/src/task_service/tests/service_tests/cases/executions.rs`.
  Comprueba tres worktrees hermanos, dos leases, pin context compartido y raíz
  `FORGE_WORKSPACE.md` fuera de los repos.
- Review group: `multirepo_workspace_review_group_is_read_only_and_exact_subject_scoped`
  — mismo `executions.rs`; solo contiene los repos/SHA del subject y no admite
  WorkspaceLease.
- Escritura accidental en context:
  `multirepo_workspace_context_write_fails_execution_with_reason` — mismo
  `executions.rs`; modifica HEAD, tracked y/o untracked del repo context y exige
  Execution failed con repo y motivo persistidos, sin resultado exitoso.
- Regresión de **un solo repo**:
  `multirepo_workspace_single_repo_preserves_legacy_scalar_response` —
  `crates/api/tests/happy_path.rs`; conserva JSON escalar existente y devuelve
  exactamente un Workspace y un repository result.
- B: `multirepo_result_records_all_repo_shas_before_terminal_event` —
  `executions.rs`; `multirepo_workspace_fallback_reuses_same_work_unit_group`
  — `crates/services/src/task_dispatcher/fallback.rs`.
- C: `multirepo_result_review_subject_freezes_each_repo` y
  `multirepo_result_rework_requirement_pins_each_repo_base` —
  `crates/services/src/task_rework.rs`; `multirepo_result_micro_fix_uses_exact_repo_bases`
  — `crates/services/src/task_rework/micro_fix.rs`;
  `multirepo_result_validation_output_requires_repo_id` —
  `crates/services/src/coordinator_outputs/tests.rs`;
  `multirepo_result_validation_selects_requested_repo_workspace` —
  `crates/services/src/validation_check_runner.rs`.
- API: `multirepo_result_execution_response_lists_each_repo` y
  `multirepo_result_validation_response_names_exact_repo` —
  `crates/api/tests/work_units.rs`; `multirepo_result_review_response_lists_each_repo`
  — `crates/api/tests/work_unit_reviews.rs`.
- MCP: `multirepo_result_reader_tools_expose_repo_results` —
  `crates/mcp-server/src/tests.rs`.
- Web: `multirepo_result_execution_sidebar_renders_repo_shas` — nuevo
  `web/src/components/execution-detail/ExecutionDetailSidebar.test.tsx`;
  `multirepo_result_review_panel_lists_subject_repos` —
  `web/src/components/task-detail/work-unit-review-panel.test.tsx`;
  `multirepo_result_validation_labels_repo` —
  `web/src/components/task-detail/task-verification.test.ts`.

Añadir estos filtros a `.github/workflows/build-local-linux.yml` junto a los de
M1 (no ejecutados al congelar este plan):

~~~sh
cargo test --locked -p db --lib multirepo_workspace_
cargo test --locked -p db --lib multirepo_result_
cargo test --locked -p services --lib multirepo_workspace_
cargo test --locked -p services --lib multirepo_result_
cargo test --locked -p api --test happy_path multirepo_workspace_
cargo test --locked -p api --test work_units multirepo_result_
cargo test --locked -p api --test work_unit_reviews multirepo_result_
cargo test --locked -p mcp-server --lib multirepo_result_
~~~

Las pruebas web conservan la suite pnpm existente; la validación M2 de Rust se
filtra por los nombres anteriores. Ninguno de estos filtros se ejecuta al
congelar M2.

### Anexo M2 — integración del workspace

**Estado: INTEGRADO / NOT RUN.** Se reconciliaron los cambios A/B/C en el
árbol compartido. Los llamadores de `get_by_work_unit_id` ahora consumen la
lista de Workspaces; las lecturas de leases consumen la lista activa o la
consulta exacta por `(Execution, Workspace)`. Todas las construcciones de
`CreateWorkUnitWorkspace` llevan `workspace_group_id` y las admisiones usan
`workspace_leases`, incluso en fixtures históricos. Se conserva la lectura
escalar para ejecuciones de un repo y Workspaces históricos sin grupo.

Se crean filas pendientes de Review Subject para cada repo writable del
WorkUnit fuente abierto; el congelado posterior las completa desde sus
resultados exactos. El productor existente registra y publica
`execution.repository_results_recorded` después de guardar los resultados y
antes del evento terminal; se verificó en `runner.rs` y no se duplicó. SSE se
suscribe al nombre explícito. La comprobación de un Execution con `workspace_id`
NULL valida el grupo, sus scopes y los resultados por repo, incluido el caso
en que el repo primary no pertenece al alcance seleccionado. Los lectores de
orquestación y ciclo de vida ahora consumen el conjunto completo del grupo.

**Desviaciones:** no se cambiaron los nombres ni firmas M2 congelados. Los
ajustes adicionales se limitan a adaptar fixtures y lectores singulares al
modelo de grupo; el payload de coordinación conserva su miembro primary
singular cuando existe y añade la lista completa por repo. `ci.yml` ejecuta
`scripts/ci-rust.sh`, cuya suite incluye `cargo test --workspace --all-targets`;
los filtros M2 explícitos están en `build-local-linux.yml`.

**Comprobaciones estáticas:** los 22 nombres de prueba M2 del contrato aparecen
exactamente una vez en sus archivos; los ocho filtros requeridos están en el
workflow local. `cargo fmt --all` y `git diff --check` pasan. Los builds,
checks, tests, clippy, pnpm y CI quedan **NOT RUN** por la restricción de esta
integración; la compilación y el comportamiento de runtime requieren el CI.
Riesgo residual: la lógica Rust y los fixtures de grupos nuevos solo recibieron
revisión estática hasta que CI ejecute el workspace completo.

## Anexo M2 — cierre CI

`build-local-linux.yml` con `run_checks=true` en verde: run 44923735519
(build y rust-checks). Correcciones tras la integración, en orden:
compilación y clippy; ruta legacy de revisión sin repos y `repo_id` por
defecto al deserializar; teardown de Project borra explícitamente las filas
V182 bajo el guard; claim de arranque valida el conjunto completo de leases por
repo; captura de resultados por repo también en el cierre por API; Gate compara
`repository_results`; un grupo de un solo repo conserva el CWD 0.9.1 (el
worktree); la revisión Human congela las filas por repo y prepara el grupo de
solo lectura antes de admitir, y su replay no lo vuelve a preparar;
`FORGE_WU_ADMISSION_REVIEW_GROUP_UNAVAILABLE` → 409
`work_unit_review_group_unavailable`.

## M3 — planning/freeze

**Estado de este documento: contrato M3 congelado para implementación; M3 no
está implementado.** M1/M2 son la base existente de esta rama. Esta sección es
la autoridad para las decisiones M3 y corrige el borrador marcado en el bloque
V183 de M0. No convierte un resultado M1/M2 en evidencia M3.

### Revisión M0 contra M1/M2 y el código actual

| Contrato | Resultado de la revisión | Decisión M3 |
|---|---|---|
| DM1: alcance writable de WorkUnit | M1 persiste `primary`, `affected`, `context`; M2 guarda `workspace_group` y un resultado por repo. REST/MCP ya admiten `repository_ids` al crear WorkUnits. | Mantener. `context` es lectura; solo `primary`/`affected` integran, reciben Gate/checks y pueden tener PR. Un WorkUnit sin `repository_ids` conserva el alcance writable completo de su revisión de repos. El plan del coordinador también puede seleccionar un subconjunto mediante `repository_ids`, validado y fijado con la revisión efectiva del Task. |
| DM2: PR/check por repo modificado | M2 guarda resultados de Execution por repo, pero no demuestra que el repo tenga diff neto frente al target. | Un repo entra al batch si su integración final tiene diff neto no vacío frente al target/base fijado. En modo `pull_request`, crear exactamente una PR por repo modificado. Repo sin cambios no crea PR ni requiere checks. `direct_merge` conserva su flujo actual por repo y la confirmación de merge sigue siendo humana. |
| DM3: integración primaria primero | `WorkUnitService::integrate` sigue siendo escalar: resuelve el Workspace de integración con `task.repo_id` y guarda un `WorkUnitIntegration`. El supuesto del borrador M0 de añadir `integrate_repositories` a `TaskIntegrationOperationManager` no coincide con el ownership actual. | Extender `WorkUnitService::integrate` para los resultados de ese WorkUnit, en el orden `primary`, luego `affected.position`; cada repo usa su propio Workspace de integración y fila hija durable. `work_unit_integration` sigue siendo la operación/idempotency parent; no se reconstruye ni se rompe su clave single-repo. Al primer conflicto/error, no deshacer éxitos previos; reintentar solo filas pendientes. `TaskIntegrationOperationManager` conserva la exclusión durable del Task. |
| Un PR por repo | `pr_metadata` tiene `UNIQUE(task_id)` y `PrMetadataRepo::get_by_task_id`; el servicio actual asume un metadata actual por Task. El repo y `work_mode` del PR se obtienen desde `task.repo_id`. | V183 debe hacer `UNIQUE(task_id, repo_id)`; lectura/escritura de proyección exige `(task_id,repo_id)` o lista. Admission, provider config, feedback, rework y acción externa se resuelven por el `admission_id` inmutable y su `repo_id`, nunca por “la PR de la Task”. No se crea PR para repos sin diff. |
| Gate y validación | Gate v2 `RequiredCheck` no tiene `repo_id`; `evaluate_required_checks` lee un único Workspace de integración por Task. El runner ya fija repo/workspace/commit en `run_for_repository`, pero la resolución de integración aún usa la API singular. | Gate v1/v2 permanecen byte/semánticamente iguales para no romper single-repo. Añadir policy v3 con `repo_id` obligatorio por required check; multi-repo usa v3. La identidad requerida es `(repo_id, check_key, definition_digest, workspace_id, commit_sha)` y solo los repos modificados generan sujetos. Un check de otro repo no satisface este check. |
| TaskLifecycle y batch | TaskLifecycle ya es la única autoridad; Gate aplica `ready_to_merge` desde una GateEvaluation exacta. M0 no puede dar por `done` un Task al observar la primera PR mergeada. | Un batch y sus filas por repo son evidencia/progreso, no autoridad lifecycle. `ready_to_merge` requiere Gate satisfecho y que todas las filas PR estén abiertas/admitidas (y las filas direct-merge preparadas). `done` requiere confirmación humana y éxito de todos los repos exigidos: cada PR mergeada y cada repo `direct_merge` integrado. Nunca auto-mergear un PR. Los estados parciales persisten y se muestran mientras `TaskLifecycle` permanece coherente. |
| Dependencias | `accepted_task_context_in_tx` exige predecessor `done`; el plan/handoff aún compara un solo integration repo. | El readiness sigue dependiendo de `TaskLifecycle::Done`, que solo alcanza el TaskMergeBatch completo. Además, la validación del handoff compara todos los repos relevantes compartidos: los repos changed del predecessor intersectados con repos `affected` o `context` declarados por el successor. Una PR abierta, cerrada o desconocida bloquea el successor; un repo sin cambios no es requisito. Para `context`, fijar el commit del predecessor ya mergeado como snapshot de lectura. |
| Replan posterior a Execution | `replace_task_repositories` retorna `TaskRepositoryReplanRequired` después de una Execution; el 409 es correcto para PATCH directo. El classifier y el contexto de replan comparan `repo_id` escalar. Las filas de WorkUnit/review/result tienen FKs restrictivos al `task_repository` actual. | Conservar el 409 directo. Habilitar cambio solo por coordinator replan + decisión Human exacta + materialización atómica. Añadir revisión inmutable del repo-set y pin por WorkUnit; no borrar/reinterpretar la revisión que usaron Executions, reviews, resultados o batches previos. Resolver explícitamente las FKs restrictivas mediante las tablas de revisión; no mutar la historia para evadirlas. |
| Respuestas y CLI | Hay DTO scaffolding M1 (`TaskRepositoryResponse`, merge y PR opcionales), pero `task_response_inner` deja ambos `None`. `MergeAfterGateResponse` y `WorkUnitIntegrationResponse` son escalares. MCP existente devuelve `TaskResponse` y lista PR feedback/actions por Task. `forge-ctl` no tiene subcomando de PR, Gate, integración ni edición de repos. | Completar esos DTOs con estado por repo conservando campos escalares actuales para el caso single-repo 0.9.1. MCP conserva tools existentes y añade `repo_id` a cada elemento/evento/listado. No añadir flags ni comando forge-ctl para M3; `task get/list` refleja el DTO JSON vigente. |

### V183 final: reconstrucción `pr_metadata` y catálogo SQLite

El SQL del bloque M0 solo copia `task.repo_id`, elimina triggers cross-table y
recrea dos guards nuevos. Se sustituye por el bloque exacto siguiente dentro
de V183, después de añadir/backfillear `task_integration_operation.repo_id`,
`task_integration_operation.repository_merge_batch_id` y
`remote_pr_admission.repo_id`. El migration runner actual ejecuta directamente
una migración que contiene `PRAGMA foreign_keys` (`crates/db/src/migration.rs:225–277`);
por ello el encabezado completo de V183 debe ser `PRAGMA foreign_keys=OFF;`
`PRAGMA legacy_alter_table=ON; BEGIN IMMEDIATE;` y su cierre `COMMIT; PRAGMA
foreign_keys=ON; PRAGMA legacy_alter_table=OFF;`. La copia toma primero la
identidad repo del TaskMerge/admission histórico y cae a `task.repo_id` para el
PR legacy sin admission. `repo_id` sigue nullable para los datos históricos y para las escrituras 0.9.1 single-repo sin batch; sus guards resuelven la identidad desde `task.repo_id`. En multirepo se exige el repo exacto. Se
reemplaza únicamente la unicidad por Task por unicidad `(task_id, repo_id)`.

```sql
PRAGMA foreign_keys = OFF;
PRAGMA legacy_alter_table = ON;
BEGIN IMMEDIATE;

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
SELECT m.id, m.task_id,
       COALESCE(merge_op.repo_id, admission.repo_id, t.repo_id),
       m.provider_type, m.provider_pr_id, m.pr_url,
       m.source_branch, m.target_branch, m.pr_state, m.merge_status,
       m.last_synced_at, m.created_at, m.updated_at,
       m.task_merge_operation_id, m.publish_operation_id, m.admission_status
FROM pr_metadata AS m
LEFT JOIN task AS t ON t.id = m.task_id
LEFT JOIN task_integration_operation AS merge_op
  ON merge_op.id = m.task_merge_operation_id
LEFT JOIN remote_pr_admission AS admission
  ON admission.metadata_id = m.id;

-- Preserve cross-table trigger SQL and inbound FK target names. With
-- foreign_keys=OFF and legacy_alter_table=ON, this rename does not rewrite
-- references in other tables' triggers or foreign keys. Attached triggers and
-- autoindexes move with the legacy table and disappear when it is dropped.
ALTER TABLE pr_metadata RENAME TO pr_metadata_legacy;
ALTER TABLE pr_metadata_v183 RENAME TO pr_metadata;
DROP TABLE pr_metadata_legacy;

-- The composite UNIQUE index is the task_id/repo_id lookup index. This index
-- supports repo-oriented sync/reconciliation without duplicating that prefix.
CREATE INDEX idx_pr_metadata_repo_task
    ON pr_metadata(repo_id, task_id) WHERE repo_id IS NOT NULL;

CREATE TRIGGER pr_metadata_merge_admission_guard_insert
BEFORE INSERT ON pr_metadata
WHEN NEW.task_merge_operation_id IS NULL
  OR NEW.publish_operation_id IS NULL
  OR NEW.repo_id IS NULL
  OR NOT EXISTS (
      SELECT 1
      FROM task_integration_operation AS merge_op
      JOIN task_integration_operation AS publish_op
        ON publish_op.id = NEW.publish_operation_id
       AND publish_op.parent_operation_id = merge_op.id
       AND publish_op.task_id = merge_op.task_id
      WHERE merge_op.id = NEW.task_merge_operation_id
        AND merge_op.task_id = NEW.task_id
        AND merge_op.repo_id = NEW.repo_id
        AND merge_op.kind = 'task_merge'
        AND merge_op.status = 'running'
        AND merge_op.remote_waiting = 1
        AND publish_op.kind = 'publish_pr'
        AND publish_op.repo_id = NEW.repo_id
        AND publish_op.repository_merge_batch_id = merge_op.repository_merge_batch_id
        AND publish_op.gate_evaluation_id = merge_op.gate_evaluation_id
  )
BEGIN
    SELECT RAISE(ABORT, 'PR metadata requires its exact repo-scoped TaskMerge and PublishPr');
END;

CREATE TRIGGER pr_metadata_merge_admission_guard_update
BEFORE UPDATE OF task_merge_operation_id, publish_operation_id ON pr_metadata
WHEN (NEW.task_merge_operation_id IS NOT OLD.task_merge_operation_id
      OR NEW.publish_operation_id IS NOT OLD.publish_operation_id)
 AND (
    NEW.task_merge_operation_id IS NULL
    OR NEW.publish_operation_id IS NULL
    OR NEW.repo_id IS NULL
    OR (OLD.task_merge_operation_id IS NOT NULL AND NOT EXISTS (
        SELECT 1
        FROM remote_pr_admission AS paused_admission
        JOIN pr_rework_supersession AS paused
          ON paused.id = paused_admission.supersession_id
        WHERE paused_admission.task_merge_operation_id = OLD.task_merge_operation_id
          AND paused_admission.publish_operation_id = OLD.publish_operation_id
          AND paused_admission.metadata_id = OLD.id
          AND paused_admission.task_id = OLD.task_id
          AND paused_admission.repo_id = OLD.repo_id
          AND paused_admission.rework_outcome = 'superseded_for_rework'
          AND paused.task_id = OLD.task_id
          AND paused.state IN ('open', 'remote_closed')
    ) AND NOT EXISTS (
        SELECT 1
        FROM remote_pr_admission AS old_admission
        JOIN remote_pr_history AS history
          ON history.task_merge_operation_id = old_admission.task_merge_operation_id
         AND history.task_id = old_admission.task_id
         AND history.original_metadata_id = old_admission.metadata_id
        WHERE old_admission.task_merge_operation_id = OLD.task_merge_operation_id
          AND old_admission.publish_operation_id = OLD.publish_operation_id
          AND old_admission.metadata_id = OLD.id
          AND old_admission.task_id = OLD.task_id
          AND old_admission.repo_id = OLD.repo_id
          AND old_admission.state IN ('merged', 'closed', 'head_mismatch', 'publication_failed')
          AND history.provider_type = OLD.provider_type
          AND history.provider_pr_id IS OLD.provider_pr_id
          AND history.pr_url IS OLD.pr_url
          AND history.source_branch = OLD.source_branch
          AND history.target_branch = OLD.target_branch
          AND history.pr_state = OLD.pr_state
          AND history.merge_status = OLD.merge_status
          AND history.admission_status = OLD.admission_status
    ))
    OR NOT EXISTS (
        SELECT 1
        FROM task_integration_operation AS merge_op
        JOIN task_integration_operation AS publish_op
          ON publish_op.parent_operation_id = merge_op.id
         AND publish_op.task_id = merge_op.task_id
        WHERE merge_op.id = NEW.task_merge_operation_id
          AND merge_op.task_id = NEW.task_id
          AND merge_op.repo_id = NEW.repo_id
          AND merge_op.kind = 'task_merge'
          AND merge_op.status = 'running'
          AND merge_op.remote_waiting = 1
          AND publish_op.id = NEW.publish_operation_id
          AND publish_op.kind = 'publish_pr'
          AND publish_op.status = 'running'
          AND publish_op.repo_id = NEW.repo_id
          AND publish_op.repository_merge_batch_id = merge_op.repository_merge_batch_id
          AND publish_op.gate_evaluation_id = merge_op.gate_evaluation_id
    )
 )
BEGIN
    SELECT RAISE(ABORT, 'PR metadata rebind requires exact repo-scoped history and admission');
END;

CREATE TRIGGER pr_metadata_admission_status_guard
BEFORE UPDATE OF admission_status ON pr_metadata
WHEN NEW.admission_status IS NOT OLD.admission_status
 AND NEW.admission_status != 'legacy_unadmitted'
 AND NOT EXISTS (
     SELECT 1 FROM remote_pr_admission AS admission
     WHERE admission.metadata_id = NEW.id
       AND admission.task_id = NEW.task_id
       AND admission.repo_id = NEW.repo_id
       AND admission.task_merge_operation_id = NEW.task_merge_operation_id
       AND admission.publish_operation_id = NEW.publish_operation_id
       AND (
           (NEW.admission_status = 'admitted' AND admission.state = 'admitted')
           OR (NEW.admission_status = 'reconciliation_required'
               AND admission.state = 'reconciliation_required')
           OR (NEW.admission_status = 'open' AND admission.state = 'open')
           OR (NEW.admission_status = 'merged' AND admission.state = 'merged')
           OR (NEW.admission_status = 'closed' AND admission.state = 'closed')
           OR (NEW.admission_status = 'failed'
               AND admission.state IN ('head_mismatch', 'publication_failed'))
       )
 )
BEGIN
    SELECT RAISE(ABORT, 'PR admission provenance must match its exact repo-scoped remote admission');
END;

CREATE TRIGGER pr_metadata_repo_insert_guard
BEFORE INSERT ON pr_metadata
WHEN NEW.repo_id IS NULL OR NOT EXISTS (
    SELECT 1
    FROM task AS t
    JOIN repo AS r ON r.id = NEW.repo_id AND r.project_id = t.project_id
    JOIN task_integration_operation AS merge_op
      ON merge_op.id = NEW.task_merge_operation_id
     AND merge_op.task_id = t.id AND merge_op.repo_id = NEW.repo_id
    JOIN task_repository_merge_batch_repo AS batch_repo
      ON batch_repo.batch_id = merge_op.repository_merge_batch_id
     AND batch_repo.task_id = t.id AND batch_repo.repo_id = NEW.repo_id
    WHERE t.id = NEW.task_id
)
BEGIN
    SELECT RAISE(ABORT, 'new PR metadata requires its exact writable repo in the Task batch');
END;

CREATE TRIGGER pr_metadata_repo_immutable
BEFORE UPDATE OF repo_id ON pr_metadata
WHEN NEW.repo_id IS NOT OLD.repo_id
BEGIN
    SELECT RAISE(ABORT, 'PR metadata repository identity is immutable');
END;

COMMIT;
PRAGMA foreign_keys = ON;
PRAGMA legacy_alter_table = OFF;
```

El bloque contiene la transacción y los PRAGMA de la conexión directa requeridos
por `crates/db/src/migration.rs:225–277`. La secuencia rename/swap
mantiene resolubles los SQL de triggers cross-table y deja el FK entrante
apuntando a `pr_metadata`. Al borrar `pr_metadata_legacy` desaparecen sus tres
triggers attached; el SQL los recrea antes de terminar V183. No se necesita
`DROP TRIGGER` explícito para ellos. El `UNIQUE(task_id,repo_id)` es también el
índice de acceso por Task; `idx_pr_metadata_task_repo` de M0 era redundante y
se reemplaza por el índice repo-first anterior.

#### Inventario de dependencias SQLite comprobado hasta V182

Inventario obtenido desde `sqlite_master`, `PRAGMA foreign_key_list` e
inspección de SQL de todas las migraciones V001–V182:

| Tipo | Nombre/objeto | Migración vigente | Tratamiento V183 |
|---|---|---|---|
| Trigger attached | `pr_metadata_merge_admission_guard_insert` | V109 | Se elimina implícitamente con `DROP TABLE`; se recrea arriba con repo y batch exactos. |
| Trigger attached | `pr_metadata_admission_status_guard` | V113 | Se elimina implícitamente; se recrea arriba y añade igualdad de `repo_id`. |
| Trigger attached | `pr_metadata_merge_admission_guard_update` | V175 (reemplazó V116/V109) | Se elimina implícitamente; se recrea arriba preservando terminal history/rework y fija el repo en admission y operation. |
| Cross-table | `legacy_pr_history_insert_guard` en `legacy_pr_history` | V114 | No se elimina: su propietario sobrevive al rebuild; su lookup usa `legacy_metadata_id` exacto. Se verifica que el nombre/SQL sigan instalados. |
| Cross-table | `pr_rework_supersession_insert_guard` en `pr_rework_supersession` | V175 | Sobrevive al rebuild sin cambios por el rename/swap. V183 la reemplaza después, conservando los checks V175 y exigiendo repo igual en admission/metadata/operation; valida `reason_ref = batch.id` en multirepo y `reason_ref = merge_op.id` en single-repo legacy sin batch. |
| Cross-table | `remote_pr_admission_insert_guard` en `remote_pr_admission` | V175 | Sobrevive al rebuild sin cambios por el rename/swap. V183 la reemplaza después, conservando los checks V175, resolviendo `pr_provider_config` desde el repo de la operación (o `task.repo_id` en legacy), igualando los repos y validando el recibo de lifecycle correspondiente a batch o a TaskMerge legacy. |
| Cross-table | `remote_pr_history_insert_guard` en `remote_pr_history` | V116 | Sobrevive al rebuild. V183 la reemplaza después, conservando sus checks y exigiendo igualdad de repo entre admission y metadata; cuando la identidad legacy es NULL, la compara con `task.repo_id`. |
| Cross-table | `task_integration_operation_provider_result_guard` en `task_integration_operation` | V175 | Sobrevive al rebuild sin cambios. V183 la reemplaza después, conservando sus checks y exigiendo repo igual entre operación, admission y metadata; permite que los tres sigan NULL en la ruta legacy single-repo. |
| Cross-table | `task_merge_closed_provider_transition_guard` en `task_lifecycle_transition` | V109 | No se elimina: enlaza operation, evento y metadata por IDs exactos; preservar. |
| View | Cualquier view que mencione `pr_metadata` | — | **Ninguna** en V001–V182. |
| FK entrante | `remote_pr_admission.metadata_id → pr_metadata.id ON DELETE CASCADE` | V112 | La tabla no se reconstruye; FK conserva el nombre destino. `foreign_key_check` requerido tras restaurar `foreign_keys=ON`. |
| Índices de tabla | PK `id`, autoíndice `UNIQUE(task_id)` | Base/V109 | PK se mantiene; el autoíndice de Task se reemplaza por `UNIQUE(task_id,repo_id)`. No había índice explícito de `pr_metadata` en V001–V182. |

El rename/swap conserva sin recompilación los seis triggers cross-table que
referencian `pr_metadata`, los demás triggers adjuntos a sus tablas y el FK
entrante. Los tres triggers attached a `pr_metadata` se recrean explícitamente
en el bloque. V183 sustituye además cuatro triggers cross-table indicados en
la tabla por cuerpos con repo/batch exactos; no los elimina como efecto del
rebuild. El inventario anterior incluye los nueve triggers cuyo SQL menciona
`pr_metadata`; no hay views. Los otros dos guards de operation enumerados
abajo no mencionan `pr_metadata`, pero cambian por su propio contrato de batch.

Tablas relacionadas que **no** se reconstruyen: `remote_pr_admission` conserva
sus seis triggers attached (`remote_pr_admission_delete_guard`,
`remote_pr_admission_identity_immutable`, `remote_pr_admission_insert_guard`,
`remote_pr_admission_rework_guard`,
`remote_pr_admission_superseded_immutable`,
`remote_pr_admission_terminal_result_immutable`), PK/UNIQUE de operaciones e
índice `idx_remote_pr_admission_reconcile`. El nuevo `repo_id` requiere un
guard inmutable adicional; su insert guard V175 se reemplaza para admitir PRs
de cualquier repo del batch y comparar config, operación y metadata con la
misma repo. `remote_pr_history` y `pr_rework_supersession` conservan sus FK y
recibos; las FK salientes de la nueva `pr_metadata` siguen siendo Task, Repo,
TaskMerge y PublishPr. `task_integration_operation` tampoco se reconstruye:
conserva sus índices/guards actuales y añade repo/batch inmutables. Los guards
de insert de TaskMerge/PublishPr cambian lookup a `NEW.repo_id` y batch exacto.

El inventario de FK relacionado leído en el checkpoint V182 queda así: `pr_metadata`
sale a `task.id` y a `task_integration_operation.id` (TaskMerge y PublishPr,
ambos RESTRICT); `remote_pr_admission` sale a TaskMerge, PublishPr, Task,
`domain_event` y `pr_metadata.id` (CASCADE); `remote_pr_history` enlaza Task,
PublishPr, `domain_event` y admission por su TaskMerge;
`pr_rework_supersession` enlaza Task, `task_coordination_request`, `domain_event`
y admission por TaskMerge; `legacy_pr_history` enlaza Task; y
`task_integration_operation` enlaza Task, GateEvaluation, parent operation y
`domain_event`; `task_lifecycle_transition` enlaza Task, GateEvaluation y
`domain_event`. Ninguna otra FK entrante apunta a `pr_metadata`; se añade solo
el nuevo `pr_metadata.repo_id → repo.id ON DELETE RESTRICT` en V183.

Las tablas no reconstruidas conservan sus otros triggers attached: los tres
de `legacy_pr_history`, los tres de `remote_pr_history`, los tres de
`pr_rework_supersession`, los guards de integridad/immutability de
`task_integration_operation`, los guards de lifecycle de
`task_lifecycle_transition` y los cuatro guards de credentials/policy en
`pr_provider_config`. Solo se sustituyen los nombres enumerados arriba; V183
no vuelve a crear/renombrar otras tablas, vistas o índices.

El bloque M0 también pretendía borrar los seis triggers cross-table y tres
guards relacionados (`task_integration_operation_remote_admission_guard`,
`task_publish_pr_gate_admission_guard`, `task_lifecycle_pr_feedback_rework_guard`)
sin recrear sus cuerpos. Eso queda retirado. Los dos guards de admisión de
operation sí requieren reemplazo en V183: conservan los checks existentes,
resuelven `work_mode` con el repo exacto, atan parent/child al mismo repo y batch en multirepo, permiten la ruta single-repo sin batch y validan el `reason_ref` correspondiente a la autoridad usada. El guard de transición de
feedback/rework V179 permanece porque su `reason_ref` es el recibo de
supersession, no el id de TaskMergeBatch.

#### Validación SQLite reproducible

Validé el bloque V183 exacto anterior con `python3 -I` y `sqlite3` del runtime
(SQLite 3.46.1). El fixture inserta Project, Repo, dos Tasks de una sola repo y
dos filas `pr_metadata` antes de V109; aplica en orden las 180 migraciones
V001–V182 (V176 no existe); crea la segunda repo de prueba y añade las columnas
prerrequisito de operación/admission y la tabla mínima que usa el nuevo guard.
Captura las 15 columnas originales justo antes del bloque V183 y ejecuta ese
bloque completo, incluidos sus PRAGMA, transacción y restauración.

Resultados:

- **PASS**: se preservan ambas filas y sus 15 columnas originales (`id`,
  `task_id`, proveedor, PR, URL, ramas, estados, timestamps y operation IDs);
  ambas reciben `repo_id` de la repo de su Task al checkpoint V182.
- **PASS**: `PRAGMA foreign_key_check` devuelve cero filas después de restaurar
  `foreign_keys=ON`; `remote_pr_admission.metadata_id` sigue apuntando a
  `pr_metadata`.
- **PASS**: permanecen los seis triggers cross-table; los tres attached se
  recrean con sus nombres y se instalan los dos guards repo nuevos. La
  actualización de `admission_status` sin la
  admission exacta aborta por `pr_metadata_admission_status_guard`; cambiar el
  `repo_id` guardado aborta por `pr_metadata_repo_immutable`.
- **PASS**: `sqlite_master` no registra views que referencien `pr_metadata`;
  el índice explícito creado es repo-first y el compuesto único impide dos PR
  actuales para el mismo `(task_id,repo_id)`.
- **NOT RUN**: Cargo/pnpm/CI y casos de servicio/API. Esta comprobación solo
  valida migración, conservación, claves foráneas y activación de guards SQL.
- **ALCANCE**: ejecuté el bloque exacto de reconstrucción `pr_metadata` con
  las columnas V183 prerrequisito y la tabla referenciada por su guard. Las
  tablas V183 de revision/batch/workspace y los cuatro reemplazos de triggers
  cross-table descritos a continuación aún no forman parte de ese bloque
  comprobado; requieren validación de integración al implementar V183.

### Datos, tipos y contratos de wire

#### V183 y versión de repo-set

V183 añade `repo_id` nullable a `task_integration_operation`,
`remote_pr_admission` y `pr_metadata`; DB triggers exigen repo en todo insert
nuevo y el backfill aborta si una admission histórica no puede mapearse desde
su `pr_provider_config.repo_id`. Los campos Rust reflejan nullability física:
`Option<String>` para esos tres `repo_id`, porque SQLite no declara `NOT NULL`
en `ALTER TABLE` y metadata legacy puede carecer de repo. Los paths nuevos
rechazan NULL antes de escribir/actuar.

El M0 `task_repository_merge_batch_repo` no debe depender de que el role/orden
de `task_repository` mutable describa la historia tras un replan. V183 crea las
tablas de revisión/pin antes del release y añade:

- `task_repository_set_revision(task_id, revision, digest, state, created_at,
  source_kind, source_ref, decision_id)` con `state IN ('building','current',
  'superseded')`, y `task_repository_set_revision_repo(task_id, revision,
  repo_id, role, position)`; las revisiones `current`/`superseded` son
  inmutables, existe una revisión current por Task y el digest es SHA-256
  canónico de los miembros ordenados/roles. El revision 1 backfilled conserva
  digest NULL como evidencia histórica y `source_kind='migration'`.
- Backfill revision 1 desde las filas V181 de todas las Tasks, incluida una
  revisión vacía para Task repo-less; pin de todos los WorkUnits existentes a
  su revision 1 en `work_unit_repository_set_revision`.
- `task_repository_merge_batch.repository_set_revision`; cada batch repo
  referencia un miembro de esa revisión. Todo WorkUnit nuevo pinnea la revisión
  vigente al materializarse. Los selectores de repos para creación de
  WorkUnit/Gate/Batch/dependency usan esta tabla de revisión como autoridad.
- No borrar filas históricas de `task_repository`: hay FKs RESTRICT desde
  `work_unit_repository`, `work_unit_review_subject_repository` y
  `work_unit_result_requirement_repository`. `task_repository_context_snapshot`
  y `workspace_scope` además quedan protegidos por guards/recibos, aunque no
  tengan FK a esa tabla. La revisión versionada pasa a ser la única fuente de
  roles/pertenencia actual e histórica; `task_repository` queda como catálogo
  de identidad legado para FKs y proyección compatible. M3 actualiza guards y
  lectores que hoy infieren autorización de `task_repository.role`; ninguno
  puede tratar una fila retenida como miembro de la revisión vigente. El
  `task.repo_id` escalar continúa siendo el espejo primary de la revisión
  vigente.
- `workspace_scope` de V091/M2 necesita `repo_id` y una unicidad de Integration
  por `(task_id, repo_id)`: su índice actual `idx_workspace_scope_integration`
  permite solo un integration Workspace por Task y su guard compara contra
  `task.repo_id`. V183 backfillea el repo desde `workspace.repo_id`, conserva
  el scope Workspace por repo y cambia el guard a comparar el repo exacto con
  la revisión Task/WorkUnit. `WorkspaceRepo::get_integration_by_task_id` se
  reemplaza por lookup `(task_id,repo_id)` más listados por Task.
- No reconstruir `work_unit_integration`: esa fila V091 es el parent idempotent
  de una llamada y `UNIQUE(task_id,operation_idempotency_key)` sigue siendo
  correcto. V183 añade `work_unit_repository_integration`, una hija durable por
  `(parent integration, repo_id)`, con Execution/workspace/source/target SHAs,
  estado/version/timestamps propios. `task_repository_merge_batch_repo` fija la
  hija exacta mediante `work_unit_repository_integration_id`; el campo M0
  `work_unit_integration_id` no identifica una fila por repo y se retira.

V183 mantiene todas las 15 columnas existentes de `pr_metadata` y agrega
`repo_id`; `PrMetadata` usa `repo_id: Option<String>`. `RemotePrAdmission` y
`TaskIntegrationOperation` incorporan también `repo_id: Option<String>` y el
segundo agrega `repository_merge_batch_id: Option<String>`. `TaskRepositorySetRevision`
y `TaskRepositorySetRevisionRepo` representan el header y los miembros
ordenados; `WorkUnit` fija su revisión en
`work_unit_repository_set_revision`. `WorkUnitIntegration` conserva la fila
parent idempotente; `WorkUnitRepositoryIntegration` modela los resultados por
repo. `TaskRepositoryMergeBatch` agrega `repository_set_revision: i64`; sus
filas por repo fijan la integración hija, operación/PR y `before_sha`/`after_sha`.
`PrMetadataRepo` expone `list_by_task(task_id)` y
`get_by_task_repo(task_id, repo_id)`; se retira el uso ambiguo de
`get_by_task_id` en servicios/MCP.

Backfill exacto de la identidad de admission (el fragmento M0 que usa
`task.repo_id` se reemplaza):

```sql
ALTER TABLE remote_pr_admission
    ADD COLUMN repo_id TEXT REFERENCES repo(id) ON DELETE RESTRICT;
UPDATE remote_pr_admission
   SET repo_id = (
       SELECT config.repo_id FROM pr_provider_config AS config
       WHERE config.id = remote_pr_admission.provider_config_id
   );
CREATE TEMP TABLE _v183_remote_pr_repo_backfill_guard (
    unresolved INTEGER NOT NULL CHECK (unresolved = 0)
);
INSERT INTO _v183_remote_pr_repo_backfill_guard
SELECT COUNT(*) FROM remote_pr_admission WHERE repo_id IS NULL;
DROP TABLE _v183_remote_pr_repo_backfill_guard;
CREATE INDEX idx_remote_pr_admission_repo_reconcile
    ON remote_pr_admission(repo_id, state, updated_at, task_merge_operation_id);
```

`remote_pr_admission.repo_id` se backfillea desde el
`pr_provider_config.repo_id` capturado por esa admission, no desde el mutable
`task.repo_id`; la migración falla cerrada si falta provider config/repo. El
`TaskIntegrationOperation.repo_id` histórico cae a la repo primaria de Task,
porque V182 todavía no emitía operaciones finales por repo. `work_unit_integration`
es parent; la fila nueva `work_unit_repository_integration` es el resultado
durable por repo y conserva cada éxito aunque el siguiente repo falle:

```sql
CREATE TABLE work_unit_repository_integration (
    id                         TEXT PRIMARY KEY,
    work_unit_integration_id   TEXT NOT NULL
        REFERENCES work_unit_integration(id) ON DELETE RESTRICT,
    task_id                    TEXT NOT NULL REFERENCES task(id) ON DELETE RESTRICT,
    work_unit_id               TEXT NOT NULL,
    execution_id               TEXT NOT NULL,
    repository_set_revision    INTEGER NOT NULL,
    repo_id                    TEXT NOT NULL REFERENCES repo(id) ON DELETE RESTRICT,
    source_workspace_id        TEXT NOT NULL,
    source_branch              TEXT NOT NULL CHECK (length(trim(source_branch)) > 0),
    source_sha                 TEXT NOT NULL CHECK (length(trim(source_sha)) > 0),
    target_workspace_id        TEXT NOT NULL,
    target_branch              TEXT NOT NULL CHECK (length(trim(target_branch)) > 0),
    target_before_sha          TEXT NOT NULL CHECK (length(trim(target_before_sha)) > 0),
    target_after_sha           TEXT,
    outcome                    TEXT NOT NULL CHECK (outcome IN
        ('running', 'success', 'conflict', 'failed', 'rejected')),
    conflict_metadata_json     TEXT CHECK (conflict_metadata_json IS NULL
        OR json_valid(conflict_metadata_json)),
    version                    INTEGER NOT NULL DEFAULT 1 CHECK (version >= 1),
    started_at                 TEXT NOT NULL,
    finished_at                TEXT,
    created_at                 TEXT NOT NULL,
    updated_at                 TEXT NOT NULL,
    UNIQUE (work_unit_integration_id, repo_id),
    UNIQUE (id, task_id, repo_id),
    FOREIGN KEY (work_unit_id, task_id)
        REFERENCES work_unit(id, task_id) ON DELETE RESTRICT,
    FOREIGN KEY (execution_id, task_id)
        REFERENCES execution(id, task_id) ON DELETE RESTRICT,
    FOREIGN KEY (source_workspace_id, task_id)
        REFERENCES workspace(id, task_id) ON DELETE RESTRICT,
    FOREIGN KEY (target_workspace_id, task_id)
        REFERENCES workspace(id, task_id) ON DELETE RESTRICT,
    FOREIGN KEY (task_id, repository_set_revision, repo_id)
        REFERENCES task_repository_set_revision_repo(task_id, revision, repo_id)
        ON DELETE RESTRICT,
    CHECK ((outcome = 'running' AND finished_at IS NULL AND target_after_sha IS NULL)
        OR (outcome != 'running' AND finished_at IS NOT NULL)),
    CHECK ((outcome = 'success' AND target_after_sha IS NOT NULL)
        OR (outcome != 'success' AND target_after_sha IS NULL))
);
CREATE INDEX idx_work_unit_repository_integration_task_repo
    ON work_unit_repository_integration(task_id, repo_id, started_at DESC, id DESC);
```

La hija tiene un solo registro por repo y llamada parent; un reintento reutiliza
la misma fila/idempotency y clasifica el target head antes de repetir Git. Los
Workspaces de integración son por `(Task,repo_id)`. M3 sustituye el índice
V091 `idx_workspace_scope_integration` por único `(task_id,repo_id)` parcial,
añade `workspace_scope.repo_id` y ajusta sus guards; el backfill y el rebuild
de este objeto se incluyen en V183 bajo la misma disciplina direct-connection,
legacy-alter-table y `foreign_key_check`.

#### DTOs, REST, MCP, SSE y forge-ctl

- Completar `TaskRepositoryResponse` existente (`api-types/src/task_repositories.rs`)
  con revisión/digest y el estado durable de cada repo; `TaskResponse.repositories`
  ya existe. `task_response_inner` rellena sus `merge` y `pull_request` actuales.
  Añadir `repository_set_revision` y `repository_set_digest` al Task y, por
  repositorio, `change_state: pending|changed|unchanged` más el resultado de
  integración. `unchanged` es terminal para esa revisión, no abre PR ni crea
  required checks; `pending` no se confunde con `pull_request: None`.
  Añadir `repository_merge_batches` con todos los batches y sus filas por repo,
  no solo el más reciente, para que un replan no oculte recuperación/PRs de una
  revisión anterior.
  Extender `WorkUnitResponse` con `integrations: Vec<WorkUnitRepositoryIntegrationResponse>`
  para que GET/list WorkUnit permita recuperar progreso parcial incluso si el
  POST de integración terminó con conflicto/timeout; la respuesta del POST
  conserva sus escalares primary y añade la misma lista `repositories`.
- Extender `MergeAfterGateResponse` (`api-types/src/gates.rs`) con
  `repository_merge_batch_id` y `repositories: Vec<TaskRepositoryMergeResponse>`;
  conservar los escalares legacy como proyección de la fila primary en Tasks
  single-repo. `WorkUnitIntegrationResponse` añade lista por repo y mantiene
  los campos actuales como proyección primary en la ruta single-repo.
- No se introduce endpoint REST distinto: se amplían respuestas de
  `GET/PATCH /tasks`, `POST /work-units/{id}/integrate`, `POST /tasks/{id}/merge`,
  Gate y listados PR existentes. La ruta merge sigue siendo acción Human.
  Feedback, external actions y rework se filtran por repo/admission; decisiones
  externas siguen sujetas a la aprobación Human/policy vigente.
- Las Tasks single-repo conservan la secuencia 0.9.1: mismo Workspace primary,
  Gate v1/v2, una sola PR, misma autorización Human y mismos valores escalares
  de integración/merge/lifecycle. Las listas nuevas son una proyección del
  mismo repo y no agregan checks, PR ni transiciones.
- MCP conserva nombres/schemas de tools de Task, WorkUnit y PR; el existente
  `forge_get_work_unit` también devuelve la lista de integraciones por repo,
  porque `work_unit_value_for` arma el JSON manualmente. Las respuestas y
  elementos de `forge_get_task`, `forge_list_task_pr_feedback`,
  `forge_list_task_pr_actions`, `forge_list_task_pr_rework` incluyen `repo_id` y
  nunca colapsan varios PRs a uno. Replan continúa por el coordinator/Human
  decision existente; no se agrega un tool que eluda esa autoridad.
- SSE conserva `execution.repository_results_recorded`; los eventos nuevos o
  ampliados de integración, batch, PR, feedback y acción externa llevan
  `task_id`, `repo_id`, identificador de batch/integration/admission y estado
  parcial. Ningún evento individual hace avanzar TaskLifecycle por sí solo.
  Nombres nuevos: `work_unit.repository_integration_changed`,
  `task.repository_merge_batch_changed` y `task.repository_pr_status_changed`;
  payload mínimo: `task_id`, `repo_id`, `repository_revision`,
  `repository_merge_batch_id` si existe, integration/admission/PR ID aplicable,
  estado/version. Extender `PublicEventType` y `project_event` mapping; eventos
  legacy single-repo siguen con su payload/proyección actuales.
- `forge-ctl` no ofrece hoy comandos de integración, merge, Gate, feedback,
  rework o edición de repo-set. No añadir flags/comandos M3. `task get/list`
  serializa el DTO de respuesta y conserva su uso single-repo.
- Actualizar `docs/api.md`, `docs/cli.md` (aclarar el alcance sin cambiar CLI),
  `docs/concepts/work-units.md`, `gates.md`, `review-and-validation.md`,
  `pr-feedback-loop.md`, `project-plan.md` y `task-operating-model.md` para
  integración/Gate/dependencias/replan, además de `CHANGELOG.md` bajo Unreleased
  si las respuestas REST/SSE cambian públicamente. README sigue corto y enlaza
  a esos contratos.

Las adiciones JSON quedan congeladas así: `TaskResponse` añade la revisión y el
digest activos; cada `TaskRepositoryResponse` conserva `repo_id`, role y
posición, y añade `change_state` (`pending|changed|unchanged`) e integración
por repo. `merge` existe solo para repo changed; `pull_request` existe solo
para repo changed con `work_mode=pull_request`. `TaskRepositoryPullRequestResponse`
incluye también `repo_id` y `admission_id` para que listados y acciones no
reconstruyan la identidad desde Task. `WorkUnitResponse` conserva
`repository_ids` y añade `integrations: Vec<WorkUnitRepositoryIntegrationResponse>`;
`WorkUnitIntegrationResponse` conserva todos sus campos escalares como
proyección primary y añade `repositories: Vec<WorkUnitRepositoryIntegrationResponse>`.
El hijo contiene `integration_id`, `parent_integration_id`, `task_id`,
`work_unit_id`, `execution_id`, `repository_set_revision`, `repo_id`, source y
target workspace/branch/SHA, `outcome`, `version`, `started_at` y `finished_at`.
`MergeAfterGateResponse` conserva los campos 0.9.1, añade
`repository_merge_batch_id` y `repositories: Vec<TaskRepositoryMergeResponse>`;
cada elemento identifica batch, repo, integración hija, TaskMerge, metadata,
SHA anterior/posterior y estado. Feedback, external action y rework mantienen
sus nombres actuales y añaden `repo_id` y `admission_id` en cada registro.
Single-repo responses conservan los campos y proyecciones escalares actuales;
los campos nuevos son aditivos y no cambian acciones/autorización.

Firmas DTO exactas para la implementación:

```rust
pub enum TaskRepositoryChangeState { Pending, Changed, Unchanged }
pub struct TaskResponseRepositoryFields {
    pub repository_set_revision: i64,
    pub repository_set_digest: Option<String>, // NULL solo en revision histórica
    pub repository_merge_batches: Vec<TaskRepositoryMergeBatchResponse>,
}
pub struct TaskRepositoryResponseFields {
    pub change_state: TaskRepositoryChangeState,
    pub integration: Option<TaskRepositoryIntegrationResponse>,
    pub merge: Option<TaskRepositoryMergeResponse>,
    pub pull_request: Option<TaskRepositoryPullRequestResponse>,
}
pub struct TaskRepositoryIntegrationResponse {
    pub repo_id: String,
    pub integration_id: Option<String>,
    pub outcome: String, // pending | success | unchanged | conflict | failed
    pub before_sha: Option<String>,
    pub after_sha: Option<String>,
}
pub struct WorkUnitResponseFields {
    pub integrations: Vec<WorkUnitRepositoryIntegrationResponse>,
}
pub struct WorkUnitIntegrationResponseFields {
    pub repositories: Vec<WorkUnitRepositoryIntegrationResponse>,
}
pub struct TaskRepositoryMergeResponseFields {
    pub repo_id: String,
    pub batch_id: String,
    pub state: TaskRepositoryMergeState,
    pub integration_id: Option<String>, // exact WorkUnitRepositoryIntegration.id
    pub task_merge_operation_id: Option<String>,
    pub pr_metadata_id: Option<String>,
    pub before_sha: Option<String>,
    pub after_sha: Option<String>,
}
pub struct TaskRepositoryMergeBatchResponseFields {
    pub id: String,
    pub task_id: String,
    pub gate_evaluation_id: String,
    pub repository_set_revision: i64,
    pub state: TaskRepositoryMergeBatchState,
    pub version: i64,
    pub repositories: Vec<TaskRepositoryMergeResponse>,
    pub created_at: String,
    pub updated_at: String,
}
```

Los nombres `*Fields` describen adiciones al DTO existente, no wrappers JSON
nuevos. `integration_id` en el batch row cambia al ID de la hija por repo; el
parent idempotente se expone como `parent_integration_id` en los resultados de
WorkUnit.

### Call graph que cambia

Los números son del HEAD M1/M2 revisado, antes de implementar M3. Los
implementadores deben fijar de nuevo referencias si ediciones anteriores en la
misma rama mueven líneas.

| Comportamiento | Puntos de entrada y archivos a tocar |
|---|---|
| Integración WorkUnit por repo, orden y parcialidad | `crates/services/src/work_unit_service.rs:2298` (`WorkUnitService::integrate`, hoy escalar; resultado/finish en `2582–2613`); `crates/services/src/task_service/workspace.rs:9` (`prepare_integration_workspace_for_work_unit`, hoy ata `task.repo_id`) y `:253` (`create_fresh_workspace`); `crates/db/src/repository.rs:717–729`/`crates/db/src/sqlite/workspace.rs:129–140` para el Workspace integration singular; `crates/db/src/repository.rs:894–910`/`crates/db/src/sqlite/work_unit.rs:1379–1459` para parent idempotente y persistencia; `crates/db/migrations/V091__workunit_dag_isolated_integration.sql:183–228` tiene el scope/unique actual por Task y guard `workspace.repo_id=task.repo_id`; V182 los amplía. M3 añade hijos por repo, repo_id al integration scope y lookup `(task,repo)`; `crates/api/src/routes/work_units.rs:367,398` agrega respuesta. El grupo de Workspace de WorkUnit está en `task_service/workspace.rs` y el claim/materialización en `task_service/execution/launch.rs:422–490`. |
| PR por repo: crear, sync/reconcile, feedback, rework, acciones | `crates/services/src/merge_service.rs:134` (`merge_after_gate`) y `:485` (`validate_gate_candidate`); `crates/services/src/task_integration_operation.rs` para claim/operaciones por Task y publicación; `crates/services/src/pr_service.rs:325` creación/publicación, `:1020` reconcile, `:1315` admissions pendientes; `crates/services/src/pr_feedback.rs:183` polling/ingest; `crates/services/src/pr_external_action.rs:134`, `:360`, `:508`, `:602`, `:646`, `:939` para aprobación/ejecución/reconciliación de acciones; `crates/services/src/pr_rework_watcher.rs:76,120`. DB: `crates/db/src/models.rs:365,387,879`, `crates/db/src/repository.rs:917–963,1785`, `crates/db/src/sqlite/pr_metadata.rs:1`, `crates/db/src/sqlite/task_integration_operation.rs:1`. REST: `crates/api/src/routes/pr_feedback.rs:101,195–315`; DTO: `crates/api-types/src/pr_feedback.rs`; los actions siguen admission-bound. |
| Gate, validación y ciclo de vida | `crates/services/src/validation_post_integration.rs:25`; `validation_check_runner.rs:83` (`resolve_subject`) y `:180` integration subject singular; `validation_service.rs:115` (`run_for_repository` ya acepta repo exacto); `gate_engine.rs:848` (`apply_merge_readiness_evaluation`) y `:1748` (`evaluate_required_checks`, hoy un workspace); `task_lifecycle.rs:1128–1163`; `crates/db/src/sqlite/task_integration_operation.rs:247–513` para las transiciones atómicas de lifecycle ligadas a operaciones. Gate/checks por repo no crean autoridad lifecycle nueva. |
| Dependencias entre Tasks | `crates/services/src/project_plan/scheduler.rs:272` (`verify_handoff_workspace_base`), `:1304` (preparar contexto) y `:1358` (`accepted_task_context_in_tx`); `crates/services/src/project_plan/graph.rs:1` y tests; cambiar el check escalar de `get_integration_by_task_id`/una base por la lista compartida de repos e IDs de PR/batch. `ProjectTaskDependencyReadiness` y `TaskLifecycle::Done` siguen siendo readiness authority. |
| Replan repo-set tras Execution | `crates/services/src/task_service/repositories.rs:45` mantiene el 409 y protege PATCH directo; `crates/api/src/routes/tasks/crud.rs:41` y `crates/mcp-server/src/tools/handlers.rs:1128` son callers del reemplazo directo; `crates/services/src/task_coordination.rs:584` (`WorkUnitPlan`) y `:2220` (`materialize`, transacción de materialización); `crates/services/src/task_coordination/replan.rs:65,249,447,605,708` (proponer, request, clasificar, elevar decisión Human, reanudar); `crates/services/src/replan_classifier.rs:16,65`; `crates/api/src/routes/task_coordination.rs:474,536–560` para turn/plan y `crates/api/src/routes/human_decisions.rs:168–200` para responder la decisión exacta; `crates/api/src/errors.rs:325` mantiene `task.repositories.replan_required`. El cambio añade `repository_set_revision/digest` exactos a `ReplanOriginalScope` (hoy `repo_id/current_repo_id` escalares). `V181:1–128` crea el catálogo, scope WorkUnit y guards; `V182:40–384,453–849,850–1180` fija snapshots/resultados, Workspace scope, admisión y leases. No se editan: V183 reemplaza en la nueva migración los guards/readers que validan pertenencia con `task_repository.role`, preserva los FKs/historia y fija WUs nuevos a la revisión aprobada. El resume exacto valida que base revision/digest y HumanDecision siguen vigentes dentro de materialización; no cambia pins anteriores. |
| REST, MCP, SSE, web y docs | `crates/api-types/src/task_repositories.rs:24–72`, `gates.rs:170–179`, `work_unit.rs:160–223`, `public_events.rs:137–206`; `crates/api/src/routes/mod.rs:164–230`, `routes/work_units.rs:72–109,367–410`, `tasks/gates.rs:377`, `routes/events.rs:101,521`, `routes/pr_feedback.rs:101–195`; `crates/mcp-server/src/tools/descriptors.rs:15,18,20,23,88–90`, `handlers.rs:46,285,684–695,914–941,1128`; `web/src/components/task-detail/task-repository-panel.tsx:4`, `TaskDetailPage.tsx:46,305,686`, `task-coordination-panel.tsx:62–63,1605–1606`, `task-pr-actions-panel.tsx:14–23`, `task-pr-feedback-panel.tsx:16–18`, `task-rework-panel.tsx:75`, `web/src/features/project-plan/ProjectPlanPage.tsx:496`, `web/src/types/generated/`; `docs/api.md`, `docs/cli.md`, `docs/concepts/work-units.md`, `gates.md`, `review-and-validation.md`, `pr-feedback-loop.md`, `project-plan.md`, `task-operating-model.md`, `CHANGELOG.md`. |

### Lanes paralelas y ownership disjunto

Los paths pertenecen a una sola lane. Interfaces cruzadas quedan congeladas en
la tabla siguiente antes de abrir trabajo paralelo. No se permite que dos lanes
editen el mismo test inline o archivo de módulo; mover tests al archivo de su
lane si el código los mezcla.

| Lane | Archivos exclusivos | Alcance y tests nombrados |
|---|---|---|
| A — Persistencia V183/repos | `crates/db/migrations/V183__multirepo_pr_batches.sql` (nuevo); `crates/db/src/models.rs`; `repository.rs`; `sqlite.rs`; `sqlite/pr_metadata.rs`; `sqlite/task_integration_operation.rs`; `sqlite/task_repository_merge_batch.rs` (nuevo); `sqlite/task_repository.rs`; `sqlite/work_unit.rs`; `sqlite/workspace.rs`; `crates/db/src/tests.rs` (solo registro de módulo); `crates/db/src/tests/multirepo_m3.rs` (nuevo, `--lib`). | Rebuild/data preservation, FK y triggers; repo/batch/revision/workspace repositories, constraints, current/history snapshots. `multirepo_pr_v183_preserves_single_repo_metadata`; `multirepo_pr_v183_preserves_cross_guards_recreates_attached`; `multirepo_integration_repository_revision_pins_existing_work_units`; `multirepo_lifecycle_batch_state_is_monotonic`; `multirepo_integration_single_repo_task_repository_migration_regression`. |
| B — Integración, checks y lifecycle | `crates/services/src/work_unit_service.rs`; `task_service/workspace.rs`; `gate_engine.rs`; `validation_check_runner.rs`; `validation_post_integration.rs`; `validation_service.rs`; `task_lifecycle.rs`; sus archivos de tests exclusivos de servicios. | Iterar repo primary-first, workspace por repo, recovery parcial, Gate/checks exactos por commit y agregación batch antes de lifecycle. `multirepo_integration_primary_first_then_affected_position`; `multirepo_integration_conflict_preserves_success_and_retries_pending_only`; `multirepo_lifecycle_gate_checks_only_changed_repositories`; `multirepo_lifecycle_all_repository_prs_required_for_ready_to_merge`; `multirepo_integration_single_repo_preserves_0_9_1_workspace_and_response`. |
| C — PR provider, feedback, rework y external actions | `crates/services/src/merge_service.rs`; `task_integration_operation.rs`; `pr_service.rs`; `pr_feedback.rs`; `pr_external_action.rs`; `pr_rework_watcher.rs`; `crates/services/src/task_rework.rs` y `task_rework/`; tests solo de estos archivos. No edita DB ni REST. | Un PR/metadata/admission por repo cambiado; exact repo provider config; sync idempotente, partial/unknown recovery y feedback/action/rework admission-bound; aprobación Human actual. `multirepo_pr_one_admission_per_changed_repository`; `multirepo_pr_reconcile_partial_batch_without_duplicate_pr`; `multirepo_pr_feedback_and_external_action_are_repo_bound`; `multirepo_pr_rework_preserves_repo_and_batch_authority`; `multirepo_pr_single_repo_legacy_admission_regression`. |
| D — Replan/dependencies y superficies | `crates/services/src/project_plan/scheduler.rs`; `project_plan/graph.rs`; `task_service/repositories.rs`; `task_coordination.rs`; `task_coordination/replan.rs`; `replan_classifier.rs`; `crates/api-types/src/`; `crates/api/src/routes/`; `crates/mcp-server/src/tools/`; `web/src/`; `web/src/types/generated/`; `docs/api.md`, `docs/cli.md`, docs/concepts relevantes, `CHANGELOG.md`, `.github/workflows/build-local-linux.yml`; tests API/MCP/web de esos paths. | Replan solo con decisión Human exacta; dependencias esperan batch Done en todos los repos relevantes; exponer parcialidad y mantener single-repo wire behavior. `multirepo_lifecycle_successor_waits_for_every_relevant_predecessor_pr`; `multirepo_lifecycle_replan_after_execution_requires_exact_human_decision`; `multirepo_lifecycle_direct_repository_patch_keeps_typed_409`; `multirepo_lifecycle_partial_batches_visible_and_retryable`; `multirepo_lifecycle_single_repo_task_response_and_sse_regression`; `multirepo_pr_api_mcp_web_show_repo_identity`. |

Los tests API se ubican en los bins existentes: `crates/api/tests/work_units.rs`,
`coordinator_verification_pr_loop.rs`, `project_plan.rs`, `pr12_public_surface.rs`
y `happy_path.rs`; MCP en `crates/mcp-server/src/tests.rs`; web en los tests
existentes de los paneles listados. Cada nombre de test debe empezar exactamente
por uno de los tres prefijos convenidos: `multirepo_integration_`,
`multirepo_pr_`, `multirepo_lifecycle_`.

### Firmas congeladas entre lanes

```rust
// DB: la lista es una revisión inmutable, ordenada y pinneada.
TaskRepositorySetRepo::get_current(task_id: &str)
    -> db::Result<Option<TaskRepositorySetRevision>>;
TaskRepositorySetRepo::get_revision(task_id: &str, revision: i64)
    -> db::Result<Option<TaskRepositorySetRevision>>;
TaskRepositorySetRepo::pin_work_unit(work_unit_id: &str, task_id: &str, revision: i64)
    -> db::Result<()>;
WorkspaceRepo::get_integration_by_task_repo(task_id: &str, repo_id: &str)
    -> db::Result<Option<Workspace>>;
WorkspaceRepo::list_integrations_by_task(task_id: &str)
    -> db::Result<Vec<Workspace>>;
PrMetadataRepo::list_by_task(task_id: &str) -> db::Result<Vec<PrMetadata>>;
PrMetadataRepo::get_by_task_repo(task_id: &str, repo_id: &str)
    -> db::Result<Option<PrMetadata>>;

// Existing work_unit_integration remains the idempotent parent.
WorkUnitRepositoryIntegration {
    id, work_unit_integration_id, task_id, work_unit_id, execution_id,
    repository_set_revision, repo_id, source_workspace_id, target_workspace_id,
    source_sha, target_before_sha, target_after_sha, outcome, version,
}

// Integration lane owns this API; every result has exact source/target repo.
WorkUnitService::integrate(
    source: CollaborationActorSource,
    work_unit_id: &str,
    execution_id: &str,
    idempotency_key: &str,
) -> Result<WorkUnitIntegrationBatch>;
// Results are ordered primary then affected.position; context is excluded.
WorkUnitIntegrationBatch { task_id, work_unit_id, repository_revision, items };
RepositoryIntegrationResult {
    repo_id, integration_id, source_sha, before_sha, after_sha, outcome,
}

// PR lane owns the merge/PR service; one batch has only changed writable repos.
MergeService::merge_after_gate(
    task_id: &str,
    gate_evaluation_id: &str,
) -> services::Result<MergeAfterGateResult>; // batch + primary legacy outcome
PrService::list_task_prs(task_id: &str) -> services::Result<Vec<TaskRepositoryPrStatus>>;
// Every provider mutation/query takes or resolves this exact durable key.
RemotePrAdmissionKey { admission_id: String, task_id: String, repo_id: String };

// Gate/lifecycle consumes stable IDs and writes one TaskLifecycle transition.
GateEngine::evaluate_required_checks_by_repo(
    task_id: &str,
    changed_repositories: &[RepositoryValidationSubject],
) -> Result<GateEvaluation>;
RepositoryValidationSubject { repo_id, workspace_id, commit_sha };
RequiredCheckV3 {
    repo_id: String, check_key: String, definition_digest: String,
    required_outcome: ValidationRunStatus,
};
// GatePolicyDocument v1/v2 stay unchanged; v3 owns repo-scoped checks.
ProjectPlanService::verify_handoffs_by_repository(
    predecessor_task_id: &str,
    successor_task_id: &str,
) -> services::Result<Vec<ProjectPlanRepositoryHandoff>>;
```

Para el flujo multirepo, `TaskLifecycle::Merging.reason_kind =
"task_repository_merge_batch"` y `reason_ref = batch.id` son el único lifecycle
receipt del merge batch. La operación legacy single-repo sin batch conserva
`TaskMerge.id` como `reason_ref`. La lane PR no escribe lifecycle por repo; la
lane Gate/lifecycle agrega filas/decisiones por repo y escribe una transition
usando la GateEvaluation/batch exactos. Una replan aprobada materializa revisión
nueva y WUs nuevos en la misma transacción; el PATCH directo conserva `409
task.repositories.replan_required`.

### Filtros CI requeridos (incorporados en la integración)

Los grupos siguientes están incorporados en `build-local-linux.yml`, junto a
los filtros M1/M2. El workflow también cubre el regresivo API de lifecycle y
deja los tests web nombrados bajo su paso `pnpm test`; ningún filtro se ejecutó
en esta integración.

```yaml
- if: ${{ !cancelled() }}
  run: cargo test --locked -p db --lib multirepo_integration_
- if: ${{ !cancelled() }}
  run: cargo test --locked -p db --lib multirepo_pr_
- if: ${{ !cancelled() }}
  run: cargo test --locked -p db --lib multirepo_lifecycle_
- if: ${{ !cancelled() }}
  run: cargo test --locked -p services --lib multirepo_integration_
- if: ${{ !cancelled() }}
  run: cargo test --locked -p services --lib multirepo_pr_
- if: ${{ !cancelled() }}
  run: cargo test --locked -p services --lib multirepo_lifecycle_
- if: ${{ !cancelled() }}
  run: cargo test --locked -p api --test work_units multirepo_integration_
- if: ${{ !cancelled() }}
  run: cargo test --locked -p api --test work_units multirepo_lifecycle_
- if: ${{ !cancelled() }}
  run: cargo test --locked -p api --test repos_endpoints multirepo_lifecycle_
- if: ${{ !cancelled() }}
  run: cargo test --locked -p api --test coordinator_verification_pr_loop multirepo_pr_
- if: ${{ !cancelled() }}
  run: cargo test --locked -p api --test project_plan multirepo_lifecycle_
- if: ${{ !cancelled() }}
  run: cargo test --locked -p api --test pr12_public_surface multirepo_pr_
- if: ${{ !cancelled() }}
  run: cargo test --locked -p api --test happy_path multirepo_lifecycle_
- if: ${{ !cancelled() }}
  run: cargo test --locked -p mcp-server --lib multirepo_integration_
- if: ${{ !cancelled() }}
  run: cargo test --locked -p mcp-server --lib multirepo_pr_
- if: ${{ !cancelled() }}
  run: cargo test --locked -p mcp-server --lib multirepo_lifecycle_
```

Los workflow steps conservan el guard `if: ${{ !cancelled() }}` y formato
`run:`; los tests web nombrados quedan bajo `pnpm test`/e2e existente.

### Cierre del freeze M0 (estado anterior a M3)

- **PASS**: contratos M0 de M3 contrastados con modelos, repositorios, rutas,
  servicios y ownership M1/M2; discrepancias y firmas congeladas arriba.
- **PARTIAL**: prueba SQLite del bloque exacto de rebuild `pr_metadata` sobre
  esquema aplicado hasta V182: preservación de dos filas single-repo, FK,
  catálogo de triggers y ejecución de guards. Las tablas V183 restantes y los
  cuatro cuerpos cross-table repo-scoped aún requieren validación al implementar.
- **NOT RUN por instrucción**: Cargo, pnpm, builds, suites y CI.
- **NO-GO para implementar desde el borrador M0**: usar la definición SQL V183
  corregida de esta sección y respetar el ownership/test naming antes de abrir
  las lanes.

### Anexo M3 — integración

- **Integrado**: V183 y los repositorios DB; integración WorkUnit por repo,
  checks/Gate por los repositorios cambiados, batch de merge y admisiones PR
  exactas por repo, feedback/rework/acciones asociadas a su repo, dependencias
  entre Tasks, replan aprobado con prueba Human exacta, REST/MCP/SSE/web y sus
  contratos generados. Los eventos de batch y estado PR incluyen los IDs de
  Task, repo, revisión, batch y admisión cuando ya existe la admisión.
- **Writer de revisión**: `TaskRepositorySetRepo::create_revision` y
  `create_revision_in_tx` generan la siguiente revisión bajo la comparación de
  revisión/digest esperada. PATCH y replan comparten ese writer; en replan se
  valida la decisión Human dentro de la misma transacción que crea la revisión
  y fija los WorkUnits.
- **Orden de lifecycle**: el camino de un repo conserva el recibo
  `ReadyToMerge` anterior a la operación. Para varios repos, primero se crea el
  batch exacto desde `Active`; las operaciones hijas enlazan el lifecycle a
  `Merging` con `reason_ref = batch.id`. La preparación del batch deja fuera los
  repositorios sin cambios; se representan como `unchanged`, sin PR ni checks.
  Los conflictos directos siguen devolviendo `details` y `files` en la respuesta
  escalar compatible.
- **Desviación**: el formato actual del plan de WorkUnits no expresa un nuevo
  conjunto de repositorios. Una replan aprobada que cambia alcance/autoridad
  crea atómicamente una nueva revisión del conjunto actual, prueba la decisión
  Human contra el Artifact/digest/request exactos y fija allí los WorkUnits
  materializados. La edición del conjunto sigue pasando por PATCH directo antes
  de Execution o requerirá una extensión explícita del formato de plan.
- **Riesgos residuales**: Cargo, pnpm y CI quedan pendientes para CI, conforme a
  la instrucción de esta integración; los flujos provider multi-repo requieren
  la validación de CI para cerrar la evidencia de publicación parcial,
  reconciliación y reintento. El estado local de estos cambios es fuente
  integrada sin commit.

## Anexo M3 — cierre CI

`build-local-linux.yml` con `run_checks=true` en verde: run 45217368759
(build y rust-checks). Correcciones tras la integración, en orden:
compilación y clippy; guards V183 que conservan el camino single-repo sin batch
(TaskMerge, PublishPr, admission, rework e historial PR terminal resuelven la
identidad legacy desde la Task y su único repo primario) mientras multi-repo
exige su fila exacta de batch; los índices de workspace conservan sus nombres
históricos y las lecturas/escrituras son conscientes de la versión de esquema
(fallback pre-V183 también en lifecycle y Gate); teardown de Project borra las
filas V183 en orden de dependencias; la revisión 1 del conjunto de repos se
crea de forma perezosa y los WorkUnits se fijan a ella; la transición
`Active → Merging` multi-repo exige la revisión actual y la fila exacta del
batch; el fence de TaskMerge activo es por repo dentro de un batch y por Task
sin batch; el claim concurrente vuelve a mapear busy/locked a
`TaskIntegrationOperationBusy`; la recuperación de merge interrumpido conserva
el SHA base original y registra el conflicto como en 0.9.1; el merge
single-repo con Gate obsoleto vuelve a responder 409. Una Task multi-repo
permanece `active` hasta admitir su batch y pasa directamente a `merging`.

## M3b — replan del conjunto de repos

**Contrato congelado para implementación.** `WorkUnitPlan` admite el campo
opcional `repository_set: [{repo_id, role}]`, con roles `primary`, `affected` y
`context`; el orden del array fija `position` después del primary. Si se omite,
la serialización conserva exactamente el formato actual. REST y MCP muestran el
campo en el plan de coordinación, y el schema del tool de coordinación lo
describe. Solo se acepta en un coordinator replan. El conjunto debe tener IDs
únicos, exactamente un primary cuando no está vacío y todas las repos deben
pertenecer al Project. Cambiar el primary se rechaza con
`task.repositories.replan_primary_immutable`: el espejo `task.repo_id`, los
guards V181 y las referencias históricas requieren una operación de cambio de
primary aparte.

Cada `PlanWorkUnit` también admite `repository_ids: [repo_id]`, como subconjunto
único de los repos `primary`/`affected` del conjunto efectivo: usa el conjunto
actual si `repository_set` se omite, y el conjunto propuesto si se incluye. Un
ID desconocido o `context` produce `coordination.invalid_plan`. Campo ausente o
array vacío conserva exactamente la serialización/digest previa y significa
todos los repos writable. El coordinador recibe una lista del conjunto actual
con `id`, `name` y `role`. En una replan, cambiar `repository_ids` de un
WorkUnit con la misma clave estable cuenta como cambio de alcance y requiere la
decisión Human exacta; Rust conserva las selecciones anteriores en el snapshot
de clasificación y materializa la selección nueva junto al pin de revisión.

Rust compara roles, miembros y orden propuestos con la revisión/digest base; un
cambio es `scope_or_authority_change` y exige la decisión Human exacta. En
resume, `BEGIN IMMEDIATE` revalida revisión/digest, Artifact, request y decisión;
comprueba que cada repo retirada, y cada repo writable degradada a `context`,
no tenga WorkUnit no terminal fijado a ella, Execution en curso, lease activo,
batch activo ni PR/admission no terminal;
si no es demostrablemente seguro, falla con
`task.repositories.replan_removal_blocked`. Luego sincroniza el catálogo
`task_repository` según guards V183, escribe la revisión con el writer existente
y fija allí los WorkUnits nuevos. Filas de catálogo referenciadas por historia
se conservan como `context`; pins, Executions, resultados, reviews, batches y
PRs anteriores no se alteran. PATCH directo conserva su 409.

La materialización publica `task.repository_set_replanned` con Task,
revisiones anterior/nueva, digest nueva, Artifact y HumanDecision; no incluye
texto provider. Tests requeridos: `multirepo_lifecycle_replan_` para propuesta
que requiere decisión exacta, resume aprobado con revisión/WU nuevos y pins
anteriores intactos, base obsoleta, retirada insegura, replan sin cambio y PATCH
directo tras Execution. Añadir `multirepo_lifecycle_plan_` para selección,
validación, clasificación de alcance y fallo de wake previo al intento; incluir
el filtro `cargo test -p services --test task_coordination multirepo_lifecycle_`
en CI para la elegibilidad de autorización. Añadir casos DB para la
sincronización y guards, y caso API si cabe; CI es el único validador de
ejecución.

### Anexo M3b — cierre CI

`build-local-linux.yml` con `run_checks=true` en verde: run 45281082483
(build y rust-checks). Correcciones tras la implementación: import de
`TaskRepository` en el módulo SQLite; los fixtures DB que añadían repos
escriben una revisión nueva del conjunto con el writer existente en lugar de
mutar el catálogo directamente; la sincronización del catálogo conserva como
`context` las filas retiradas que la historia todavía referencia.

## Hallazgos del E2E FP-2026-003

E2E real sobre seis repositorios FirePOS (servidor de pruebas, modo
`pull_request` real). Cada hallazgo se valida solo en CI.

### Corregidos en 0.9.2

| # | Hallazgo | Corrección |
|---|---|---|
| E1 | No se podían añadir repos no primarios por REST (409) | `POST /projects/{id}/repos` crea repos adicionales no primarios |
| E2 | La UI no listaba ni añadía repos adicionales | Settings → Repos lista todos y permite añadir |
| E3 | `PATCH` de RoleMembership devolvía 500 | La ruta extrae el segmento del rol |
| E4 | Los planes del coordinador no acotaban WorkUnits por repo | `repository_ids` por WorkUnit, validado contra el conjunto escribible |
| E5 | Una petición de coordinación quedaba atascada sin avisar si el wake fallaba antes del intento | La petición pasa a `failed` con código y mensaje |
| E6 | La documentación decía que Claude Code podía coordinar | Coordinación automática solo con Codex CLI |
| E7 | Los agentes hacían commit con la identidad global de la máquina | Autor y committer por Agent y rol, trailers `Agent-Role`/`Agent-Run`, identidad fija para los commits de Forge |
| E8 | Retirar un miembro obligaba a reautorizar la coordinación | El estrechamiento puro refresca la autorización en la misma transacción |
| E9 | Un plan con `repository_ids` y `requires_integration: false` se aceptaba y habría ejecutado implementadores sin workspace | Validación del plan y plantilla del coordinador que explica el campo |

### Pendientes (0.9.3 o posterior)

| # | Hallazgo | Dirección |
|---|---|---|
| P1 | Settings no muestra qué Agents trabajan en un Project | Vista de Agents por Project |
| P2 | El equipo se monta Task a Task | Equipo por defecto en el Project |
| P3 | Las decisiones humanas llegan como interrupciones sueltas | Bandeja de decisiones con fichas preparadas |
| P4 | La autonomía es todo o nada | Nivel de autonomía por Project o Task ligado a la clase de riesgo |
| P5 | Los agentes comparten la credencial del host con el humano | Identidad nivel 2: cuenta o token propio por Agent |
| P6 | La decisión humana de alcance del ledger no llega al coordinador como referencia | Pasar el identificador de la decisión en el contexto de coordinación |

## Correcciones tras el CI completo de 0.9.2

Sustituyen lo indicado en el SQL congelado de V183:

- El backfill de `task_integration_operation.repo_id` levanta
  `task_integration_operation_binding_immutable` y lo restaura sin cambios en la
  misma transacción; sin ello V183 abortaba en cualquier instalación con
  operaciones de integración.
- Las operaciones en curso **no** se agrupan en un batch. Una TaskMerge anterior
  a V183 sigue sin batch, con `reason_kind = task_merge` y
  `reason_ref = TaskMerge.id`, hasta terminar. Se retiran el guard
  `_v183_running_merge_scope_guard` y las inserciones de batch.
- Los eventos `pr.status_changed` incluyen `repo_id`. El guard de historial PR
  admite su ausencia solo en una operación sin batch sobre el repo primario
  (eventos anteriores a V183).
