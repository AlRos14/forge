# Plan — Soporte multirepo por Task (0.9.2)

Fecha: 2026-11-18. Baseline inspeccionado: `main` en `a8946cdd` (0.9.1, esquema
hasta V180). Este documento es un plan: no declara ninguna capacidad como
implementada ni aceptada. Ninguna evidencia de este plan se ha obtenido
compilando o ejecutando tests localmente; la validación será solo en CI.

**Estado:** M0 (congelación de contratos) **PENDIENTE DE REVISIÓN** por el dueño.

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

1. **Repo de la Task.** `task.repo_id` es único. `TaskService::create` lo toma
   del padre o de `project.primary_repo_id`
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
4. **Leases.** `workspace_lease` tiene un `workspace_id` y un
   `repository_binding_id`: un lease por workspace.
5. **Resultado de Execution.** `execution.workspace_id`, `before_sha` y
   `after_sha` son únicos. Igual en `work_unit_review_subject` y
   `work_unit_result_requirement` (`result_commit_sha`, `result_workspace_id`,
   `workspace_snapshot_digest`).
6. **Integración.** `work_unit_integration` ya es por par de workspaces
   (origen/destino) con SHA exactos, así que una fila por repo encaja sin
   cambio de forma.
7. **PR.** `pr_metadata.task_id` es `UNIQUE`: hoy una Task solo puede tener una
   PR. `remote_pr_admission` se indexa por `task_merge_operation_id` e incluye
   `remote_repo_identity`. `pr_provider_config` y la política de feedback ya son
   por repo.
8. **Validación.** `validation_check_definition.repo_id` y
   `validation_run.workspace_id/commit_sha` ya son por repo.
9. **Dependencias del plan de proyecto.** `project_plan_handoff` fija el commit
   integrado del predecesor y el sucesor debe partir de una base que lo
   contenga. Solo tiene sentido dentro del mismo repo.
10. **Replanes.** `replan_classifier` ya compara `repo_id` con
    `current_repo_id` y clasifica el cambio como de alcance.

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
- `primary` es también escribible (se trata como `affected`);
- todos los repos pertenecen al Project de la Task;
- un repo no puede tener dos roles en la misma Task;
- `context` nunca recibe escrituras, integraciones ni PR.

Las Tasks existentes se rellenan con una fila `primary` desde `task.repo_id`.
Las subtareas heredan el conjunto del padre salvo que la petición indique otro.

### Alcance de repos del WorkUnit

Columna o tabla `work_unit_repository(work_unit_id, repo_id)`: subconjunto de
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
  exactos del resultado. Un repo sin cambios no exige checks salvo que la
  política lo pida.

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

Un solo bloque nuevo, `V181__multirepo_task_repositories.sql` (o V181–V183 si
se separa por fase), solo aditivo salvo la reconstrucción de `pr_metadata`:

- crear `task_repository`, `work_unit_repository`, `workspace_group`,
  `execution_repository_result` y las tablas hijas de review subject y result
  requirement;
- rellenar `task_repository` con `primary` desde `task.repo_id`;
- reconstruir `pr_metadata` con `repo_id` (relleno desde `task.repo_id`) y
  `UNIQUE(task_id, repo_id)`, conservando todas las filas;
- añadir `repo_id` a `remote_pr_admission` y rellenarlo.

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

- **DM1:** un implementador por packet por defecto, con alcance por WorkUnit
  opcional para paralelizar (propuesta de este plan). Pendiente de confirmar si
  algún packet necesita implementadores paralelos desde el inicio.
- **DM2:** los repos sin cambios no abren PR ni exigen checks (propuesta).
- **DM3:** orden de integración: `primary` primero (propuesta).

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
