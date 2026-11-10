# PR0 — Contrato de coordinación de tareas

Estado: contrato PR0 aceptado para implementación. No es una aceptación
funcional ni autoriza lanzar la Task FirePOS real.
Fecha: 2026-11-07.
Baseline de código: `ae3b2287` sobre `e5b8d394`; migración última verificada:
`V157__advance_rework_line_from_exact_reviewed_result.sql`. Este contrato se
publicó antes de implementar PR1–PR6. La implementación usa migraciones
aditivas V158 y V159; ningún número histórico se reutiliza.

Decisiones de dirección: [ADR 0016](../adr/0016-task-coordination-turn.md).
Modelo operativo objetivo: [Task operating model](../concepts/task-operating-model.md).
Plan del programa: [Task coordination and guided experience](task-coordination-and-guided-experience.md).

## 1. Alcance

PR0 fija las fronteras compartidas por backend, UX y recuperación. No cambia
el runtime. Su salida es este contrato, el ADR, la matriz de autoridad, los
contratos versionados y los bocetos de recorrido.

Queda dentro del programa:

- admitir un turno de coordinación de Task sin WorkUnits de implementación
  previos;
- producir un plan de desglose inspeccionable y versionado;
- materializar WorkUnits sólo mediante una operación determinista e idempotente;
- distinguir preparación, aprobación, materialización, dispatch y supervisión;
- conservar la autoridad exacta de Actor, TaskRole, WorkUnit, WorkspaceLease,
  revisión, Gate y TaskLifecycle.

Queda fuera y no se modifica por PR0:

- TaskLifecycle como autoridad única del estado agregado;
- la admisión de workers, el ceiling de cuatro Executions por Task y la
  preparación de Workspaces;
- la revisión formal, ReviewReport, ValidationRun, Evidence y Gate;
- la autoridad de Project, sus owners y miembros;
- Project Copilot como gestión de Project;
- cualquier operación sobre una Task real, proveedor real o instancia compartida.

## 2. Baseline comprobado

| Frontera | Hecho actual comprobado | Consecuencia para el contrato |
|---|---|---|
| Arranque | `TaskLifecycleService::launch_preflight` bloquea `no_work_units`; `launch_task` registra `task_dispatch` y el dispatcher admite WorkUnits asignadas. | El turno de coordinación no puede depender de `launch_task` ni de una WorkUnit de implementación previa. |
| Wakes | `orchestrator_wake` exige evento durable, TaskRole con `orchestration`, versión/policy exactas y membership activa (V152). | El turno reutiliza esa autoridad; no crea un scheduler ni una autoridad de rol paralela. |
| Intentos | `orchestrator_wake_execution` pasa por `reserved -> start_requested -> running -> completed/failed/uncertain`; el Execution es workspace-free. | El recibo de Provider Start, la incertidumbre y el replay se heredan sin cambios. |
| Acciones | `OrchestratorResponse` sólo acepta `message`, `handoff`, `create_work_unit`, `proposal`; `orchestrator_action` tiene un `CHECK` cerrado. | El plan de coordinación necesita un contrato de salida propio, no una acción genérica más. |
| WorkUnits | `WorkUnitService` crea, asigna y prepara scopes; no hay comando de creación en `forge-ctl`. | La materialización pasa por los mismos servicios y no crea autoridad por texto. |
| Copilot | `project_copilot_run` es Project-scoped, con `context_task_id` opcional y autoridad de Project. | No se copia su autoridad; como máximo se reutiliza su plumbing de harness/MCP en un contrato Task-scoped posterior. |
| Colaboración | Proposal y Decision ya son registros durables; Decision no ejecuta la acción. | La aprobación en modo propuesta usa esos registros, no un tercer mecanismo silencioso. |
| Lifecycle/Gate | TaskLifecycle y GateEvaluation son las autoridades deterministas; no se derivan del último Execution. | El turno y la materialización no escriben TaskLifecycle ni Gate. |

### 2.1 Inventario de fronteras y consumidores

| Frontera | Código / persistencia observada | Superficies que deben cambiar juntas |
|---|---|---|
| Wake y consumidor de eventos | `crates/services/src/orchestrator_runtime.rs` (`classify_event`, `admit_signal`, `dispatch_one`, `apply_completed_actions`), `crates/services/src/task_service/orchestrator.rs`, `crates/db/src/sqlite.rs` | Servicio, tests de runtime, eventos SSE y documentación de orquestación. |
| Persistencia de wake | `orchestrator_wake`, `orchestrator_wake_execution`, `orchestrator_action`; guards y triggers de V094, V142, V152 | Migración aditiva, repos, mappers, fixtures y pruebas de replay. |
| Dispatch y lifecycle | `crates/services/src/task_lifecycle.rs`, `task_dispatcher.rs`, `task_dispatcher/dispatch.rs`, `active_recovery.rs`; `task_dispatch` V141/V145 | Launch preflight, dispatch, pause/resume/cancel, receipts y recuperación. |
| Roles y capacidades | `memberships.rs`, `roles.rs`, `task_role_configuration.rs`, `role_capabilities.rs`; V149–V152 | Team snapshot, autorización, DTOs y UI de equipo. |
| WorkUnits | `work_unit_service.rs`, `work_unit_service/`; `WorkUnitRepo`, dependencias, review subject y Artifact inputs | Creación atómica del plan, preparación de Workspace y dispatch. |
| Colaboración y planes | `collaboration_service.rs`, `plan_artifact.rs`; `artifact`, `proposal`, `decision`, `execution_artifact_output` V090/V098 | Nuevo kind `work_unit_plan`, Proposal/Decision de aprobación y proyección UI. |
| Project Copilot | `project_copilot.rs`, `copilot_mcp.rs`, `ProjectCopilotDock`; V139/V140 | No cambia su autoridad; sólo se documenta el límite y una eventual delegación Task-scoped. |
| REST/DTO | `crates/api-types/src/task_dispatch.rs`, `roles.rs`, `work_unit.rs`, `collaboration.rs`; `crates/api/src/routes/tasks/*`, `task_dispatch.rs` | DTOs nuevos, permisos, errores tipados y docs/api.md. |
| MCP/CLI | `crates/mcp-server/src/tools/descriptors.rs`, `handlers.rs`; `crates/forge-client/src/` | Herramientas/comandos de lectura y mutación con el mismo servicio. |
| SSE y eventos públicos | `crates/events/`, `crates/api-types/src/public_events.rs` | Eventos nuevos con IDs/estado/digest, sin contenido de plan ni credenciales. |
| Logs y trazas | `crates/services/src/task_service/logs.rs`, adapters, JSONL de harness | El plan no se trata como secreto ni como ruta de confianza; la traza sigue el contrato actual. |
| Cleanup/teardown | `project_deletion.rs`, guardas de borrado, `workspace_cleanup.rs` | Nuevos registros con FK/cascada y guards que preserven historia fuera del teardown. |
| Recovery/shutdown | `recovery.rs`, `shutdown.rs`, `task_failure_retry.rs`, `task_rework.rs` | El request y el wake deben sobrevivir reinicios; no se replaya un Start incierto. |

## 3. Invariantes afectados y respuesta

| Invariante | Impacto | Respuesta del contrato |
|---|---|---|
| INV-001/002 | Nuevo participante de coordinación | Humano y Agent son Actors; el coordinador es un Agent harness-bound seleccionado por el equipo, no una clase nueva. |
| INV-003/004 | Identidad del coordinador | Se congela Agent, harness, profile/config snapshot, TaskRole ID/version y membership. Un cambio de identidad exige nueva autorización, no mutación silenciosa. |
| INV-005/006 | Roles multi-Actor | El TaskRole puede tener varios miembros; la autorización selecciona un coordinador exacto. El mismo Actor puede tener otros roles, pero cada Execution conserva su rol. |
| INV-007 | Estado agregado | `task_coordination_request.state` y el estado del plan son estados de preparación, no un sustituto de TaskLifecycle. |
| INV-008 | Execution histórica | El turno es un Execution real con Actor, rol, Task, snapshot y provider receipts. No es una WorkUnit. |
| INV-009 | Sin purpose | La identidad del intento se deriva de `task_coordination_request` y su vínculo con wake/Execution, no de una clasificación de propósito. |
| INV-010/011 | Continuidad | No se infiere HarnessSession del rol, Task o último Execution. Un coordinador Humano no recibe sesión ficticia. |
| INV-012/013 | Capacidad de harness | El turno exige un adapter con sandbox read-only probado; un host no soportado falla cerrado. |
| INV-014/015/016 | Plan y WorkUnit | El plan es un Artifact producido por el Execution; la materialización crea WorkUnits reales y no introduce un plan engine canónico. |
| INV-017 | Escritura paralela | El turno no tiene Workspace ni escribe repositorio. Los workers conservan workspaces aislados. |
| INV-018/019/020/021 | Revisión vs coordinación | Coordinar no es revisar. Una revisión formal sigue exigiendo `formal_review` y un sujeto exacto. |
| INV-022/023/024/025 | Wakes y modos | El turno nace de un evento durable y respeta el modo de coordinación; no hay polling de tokens. Las acciones disruptivas siguen pasando por Proposal/Decision. |
| INV-026 | Vocabulario pequeño | Se reutilizan Message, Handoff, Proposal y Decision; se añade un plan de WorkUnits tipado, no un workflow general. |
| INV-027 | Artifact genérico | Se añade el kind `work_unit_plan` con contenido JSON versionado, producer Execution y digest exacto. |
| INV-028/029/030 | Autoridad determinista | La materialización valida en código; el modelo no concede permisos. Gates siguen evaluando entradas inmutables. |
| INV-031/032 | Cognición vs política | El modelo propone; Forge valida. Lo no soportado se informa; no se simula steering ni éxito. |
| INV-033/034/035 | Historia y fuentes | No se borran ni reetiquetan wakes/Executions históricos. Una sola autoridad por hecho; migraciones aditivas. |
| INV-038/039/040 | Project/Task | La autorización de Task exige autoridad de mutación vigente; el parentesco de Task y el owner canónico no cambian. |

## 4. Autoridad y modos

### 4.1 Matriz de autoridad

| Actor / autoridad | Puede | No puede |
|---|---|---|
| Humano con autoridad de mutación vigente sobre la Task | Leer; configurar modo y coordinador; solicitar turno; aprobar/rechazar plan; revocar autorización; operar WorkUnits y dispatch manuales; revisar si su membership lo permite | Conceder autoridad de worker por texto; saltarse TaskLifecycle, Gate, revisión o lease; actuar sobre otra cuenta |
| Agente coordinador seleccionado | Leer Task, equipo, WorkUnits, Artifacts, revisiones y estado; ejecutar el turno read-only; producir el plan; proponer; solicitar decisión humana; materializar sólo dentro del envelope autorizado mediante el servicio determinista | Crear roles o memberships; ampliar capacidades; escribir repositorio; iniciar un worker por sí mismo; aprobar su propio plan; ejecutar Decisions; mergear |
| Agente trabajador | Ejecutar la WorkUnit exacta asignada, con su lease y límites | Coordinar la Task, elegir otros Actores, ampliar su scope, revisar su propia entrega |
| Agente revisor | Producir ReviewReport sobre el sujeto exacto con `formal_review` | Corregir la entrega revisada, aprobar una entrega ajena a su línea, decidir una excepción de negocio |
| TaskLifecycleService | Validar y aplicar transiciones agregadas, incluido el launch intent | Interpretar el plan como estado de Task o iniciar workers por sí mismo |
| GateEngine | Evaluar policy e inputs exactos | Ejecutar validación, revisión, integración o merge |
| Dispatcher | Admitir WorkUnits elegibles, asignadas y con lease | Elegir Actores, crear WorkUnits o convertir un plan en trabajo |

### 4.2 Modos

| Modo | Turno | Plan | Materialización | Dispatch |
|---|---|---|---|---|
| `manual` | No automático | Humano edita WorkUnits; puede producir un Plan Artifact existente | Humano | Launch manual existente |
| `proposal` | Sí, read-only | `work_unit_plan` inspeccionable; Proposal creada | Sólo tras Decision humana favorable | Sólo tras materialización y launch humano |
| `prepare_and_execute` | Sí, read-only | `work_unit_plan` inspeccionable | Automática si el plan cae dentro del envelope; si no, Proposal y espera humana | Launch determinista bajo la autorización humana, tras materializar y preparar lo requerido |

`prepare_and_execute` no concede al coordinador acceso directo al endpoint de
launch ni a Git. El materializador determinista registra el launch intent con
el Humano autorizante como `requested_by` y un idempotency key derivado del
request y del plan digest. Cada worker sigue pasando por asignación exacta,
admisión, capacidad y lease.

En v1 el turno automático exige un coordinador Agent. Un Humano puede ser
responsable del equipo y editar el plan manualmente, pero no se crea un
Execution de harness ficticio para él (INV-011). Autorizar un Humano como
coordinador y solicitar un turno devuelve
`422 coordination.human_turn_unsupported` hasta que exista un contrato de turno
humano explícito. El modo `manual` sigue siendo la vía humana completa.

Un plan fuera del envelope no se recorta ni se aplica parcialmente. Se convierte
en Proposal; el Humano puede aprobarlo, ajustarlo o rechazarlo.

### 4.3 Envelope inicial

El envelope se guarda con la autorización y no lo puede ampliar el coordinador:

```json
{
  "schema_version": 1,
  "max_work_units": 4,
  "allow_new_task_roles": false,
  "allow_membership_changes": false,
  "allow_capability_changes": false,
  "allow_repository_write": true,
  "allow_external_actions": false,
  "allow_integration": true,
  "allow_merge": false,
  "allowed_review_roles": ["reviewer"],
  "require_independent_review": true
}
```

Los máximos iniciales reutilizan el límite actual de cuatro WorkUnits por
Execution de orquestación. Aumentarlo requiere una decisión de producto y no
forma parte de este programa.

## 5. Modelo durable propuesto

Los nombres son contrato de PR0; el número de migración se fija al implementar.
Todas las tablas son aditivas. No se modifica ninguna migración histórica.

### 5.1 `task_coordination`

Una fila por Task. Es la autorización vigente, no el historial de cada turno.

```text
task_id                    TEXT PRIMARY KEY -> task(id)
mode                       TEXT manual|proposal|prepare_and_execute
coordinator_task_role_id   TEXT NULL -> task_role(id)
coordinator_actor_kind     TEXT NULL human|agent
coordinator_actor_id       TEXT NULL
authorization_version      INTEGER >= 1
team_snapshot_json         TEXT NOT NULL
team_digest                TEXT NOT NULL (length 64)
envelope_json              TEXT NOT NULL
envelope_digest            TEXT NOT NULL (length 64)
requested_by               TEXT NULL -> user(id)
turn_count                 INTEGER NOT NULL DEFAULT 0
quality_request_count      INTEGER NOT NULL DEFAULT 0
max_turns                  INTEGER NOT NULL DEFAULT 8
max_quality_requests       INTEGER NOT NULL DEFAULT 8
created_at, updated_at     TEXT NOT NULL
revoked_at                 TEXT NULL
revoke_reason              TEXT NULL
```

Reglas:

- `mode = manual` puede conservar una selección de coordinador para la UI, pero
  no la usa como autoridad; los modos `proposal` y `prepare_and_execute` exigen
  rol y Actor no nulos al solicitar un turno.
- Toda mutación avanza `authorization_version` y exige un
  `task_coordination_receipt` con idempotency key y digest de request.
- `team_snapshot_json` fija TaskRole IDs/versiones/configuraciones y membership
  IDs/estados activos. No crea una segunda autoridad de equipo: es evidencia
  exacta para comparar contra las filas vigentes.
- `requested_by` debe ser el Humano autenticado con autoridad de mutación
  vigente al autorizar. Puede quedar nulo sólo en la fila `manual` creada por
  backfill o por el trigger de nuevas Tasks; no se acepta un Agent como
  autorizante.
- Revocar no borra la fila: marca `revoked_at` y avanza versión. Una Task puede
  volver a autorizar con otra versión.
- `turn_count` y `quality_request_count` son presupuestos globales de la Task.
  Cambiar de ruta, categoría o rol no los reinicia. Los defaults son ocho
  turnos y ocho solicitudes de calidad; una Task existente no gana turnos
  retroactivos.

### 5.2 `task_coordination_receipt`

```text
id                    TEXT PRIMARY KEY
task_id               TEXT NOT NULL -> task(id)
operation             TEXT authorize|revoke
idempotency_key       TEXT NOT NULL
request_digest        TEXT NOT NULL (length 64)
authorization_version INTEGER NOT NULL
created_at            TEXT NOT NULL
UNIQUE(task_id, idempotency_key)
```

Replay con la misma clave y digest devuelve el recibo; distinto contenido es
`409 coordination.idempotency_conflict`.

### 5.3 `task_coordination_request`

```text
id                    TEXT PRIMARY KEY
task_id               TEXT NOT NULL -> task(id)
authorization_version INTEGER NOT NULL
mode                  TEXT proposal|prepare_and_execute
brief_json            TEXT NOT NULL (JSON object bounded by the service)
objective_digest      TEXT NOT NULL (length 64)
request_digest        TEXT NOT NULL (length 64)
idempotency_key       TEXT NOT NULL
state                 TEXT requested|running|plan_ready|awaiting_approval|
                           materializing|materialized|failed|cancelled|uncertain
wake_id               TEXT NULL -> orchestrator_wake(id)
execution_id          TEXT NULL -> execution(id)
plan_artifact_id      TEXT NULL -> artifact(id)
failure_code          TEXT NULL
failure_message       TEXT NULL
created_at, updated_at, completed_at
UNIQUE(task_id, idempotency_key)
```

`request_digest` cubre Task, objective/constraints, autorización, envelope y
team digest. `brief_json` conserva el brief exacto para el turno y para
auditoría autorizada. `objective_digest` cubre ese mismo brief; los eventos
públicos sólo publican el digest y el estado.
Los estados `plan_ready`, `awaiting_approval`, `materialized` y `failed` son
terminales o de espera explícita; ninguno es TaskLifecycle.

### 5.4 `task_coordination_plan`

El plan es un Artifact `work_unit_plan`; esta fila añade el estado de aprobación
y materialización sin duplicar el contenido.

```text
artifact_id           TEXT PRIMARY KEY -> artifact(id)
task_id               TEXT NOT NULL -> task(id)
request_id            TEXT NOT NULL -> task_coordination_request(id)
authorization_version INTEGER NOT NULL
plan_digest           TEXT NOT NULL (length 64)
state                 TEXT proposed|approved|rejected|superseded|materialized
proposal_id           TEXT NULL -> proposal(id)
decision_id           TEXT NULL -> decision(id)
version               INTEGER >= 1
created_at, updated_at
```

- El Artifact `work_unit_plan` es inline, JSON, con producer Execution y digest
  exacto. Su creación usa el binding `execution_artifact_output` (equivalente
  al Plan Artifact actual, no el writer genérico) y commitea junto con la fila
  de plan. El trigger de output debe aceptar el kind nuevo sin relajar los
  guards de Plan/ReviewReport.
- En `proposal`, el coordinador produce el plan y Forge crea una Proposal con
  `target_kind = task`, `action = materialize_work_unit_plan`,
  `target_digest = plan_digest` y `artifact_ids = [artifact_id]`.
- En `prepare_and_execute`, Forge evalúa el envelope. Si encaja, el plan pasa a
  `approved` y se materializa; si no, se crea la misma Proposal y queda
  `awaiting_approval`.
- Una Decision favorable sobre la Proposal exacta es la aprobación humana. Un
  cambio de contenido o de versión de Proposal es `409`, no una aprobación
  implícita.
- El plan nunca se modifica. Una corrección produce otro Artifact y otra fila,
  con `supersedes` explícito.

### 5.5 `task_coordination_materialization`

```text
id                    TEXT PRIMARY KEY
task_id               TEXT NOT NULL -> task(id)
request_id            TEXT NOT NULL -> task_coordination_request(id)
plan_artifact_id      TEXT NOT NULL -> artifact(id)
plan_digest           TEXT NOT NULL
authorization_version INTEGER NOT NULL
task_version_before   INTEGER NOT NULL
task_version_after    INTEGER NOT NULL
work_unit_ids_json    TEXT NOT NULL
receipt_digest        TEXT NOT NULL
created_by            TEXT NOT NULL -> user(id)
created_at            TEXT NOT NULL
UNIQUE(plan_artifact_id)
```

La tabla es un recibo de efecto, no una nueva autoridad. `work_unit_ids_json`
es un array ordenado de IDs creados; el digest cubre plan, envelope, request y
versión de Task. Un replay del mismo plan devuelve el recibo; no crea WorkUnits
nuevas.

### 5.6 Artifact `work_unit_plan`

Se añade el kind `work_unit_plan` al catálogo generado de Artifacts. El
contenido es JSON, no Markdown de checklist. El `Plan` Artifact existente
sigue siendo el plan cognitivo/legible; no se reutiliza para este contrato.

Contenido v1 (límites: 256 KiB, 4 WorkUnits, textos acotados por el servicio):

```json
{
  "schema_version": 1,
  "request_id": "...",
  "task_id": "...",
  "task_version": 12,
  "authorization_version": 3,
  "team_digest": "...",
  "envelope_digest": "...",
  "objective_digest": "...",
  "work_units": [
    {
      "key": "api",
      "title": "Implementar API",
      "scope": "Contrato REST y pruebas",
      "role": "implementer",
      "assigned_actor": {"kind": "agent", "id": "..."},
      "parent_key": null,
      "depends_on": ["design"],
      "requires_integration": true,
      "execution_instructions": "Instrucciones preparadas para el harness",
      "review_subject_key": null,
      "acceptance": ["Los tests del contrato pasan"],
      "artifact_inputs": []
    }
  ],
  "delivery": {
    "kind": "work_units",
    "requires_approval": false
  },
  "review": [],
  "limits": {"max_work_units": 4}
}
```

Reglas de validación:

- `schema_version` exacto y digest del contenido.
- `task_id`, `request_id`, `task_version`, `authorization_version`,
  `team_digest`, `envelope_digest` y `objective_digest` deben coincidir con el
  request y la autorización vigentes al materializar.
- `key` único y estable; el ID de WorkUnit se genera en la materialización, no
  lo elige el modelo.
- `role` debe existir ya en la Task; el plan no crea TaskRole.
- `assigned_actor` debe ser miembro activo elegible del rol o quedar nulo para
  trabajo humano pendiente; no se selecciona al azar.
- `depends_on` y `parent_key` deben referenciar claves del mismo plan o
  WorkUnits existentes de la misma Task; el grafo debe ser acíclico.
- `review_subject_key` debe apuntar a una WorkUnit del plan; el rol revisor
  debe tener `formal_review` y el Actor debe ser independiente.
- `artifact_inputs` debe referenciar Artifacts exactos de la misma Task, con
  digest y producer; una lista de enlaces no es un input.
- `requires_integration = true` exige un repo de Task y preparación posterior
  del Workspace; no autoriza escritura por sí mismo.
- `allow_new_task_roles`, `allow_membership_changes` y
  `allow_capability_changes` deben ser `false` en v1. Cualquier intento de
  usarlos es `422 coordination.unsupported_plan_item`.
- Textos: title <= 512, scope <= 4096, instructions <= 64 KiB, acceptance
  <= 32 items, dependencies <= 32, plan <= 256 KiB.

### 5.7 `task_quality_route` — añadido por V161

El límite global de calidad de `task_coordination` no basta para impedir que
una misma cadena consuma todas sus rondas ni para detectar alternancia entre
destinatarios. V161 añade un registro por cadena con identidad estable,
`request_count`, `budget_used` y `max_requests` (cuatro solicitudes por defecto).
El ID se basa en el `work_unit_review_line` cuando el origen es
un ReviewReport con línea; en otro caso queda anclado al tipo, ID y digest del
origen exacto. Categoría y TaskRole destino no forman parte de la identidad.

La inserción, el presupuesto global y el de ruta, el orden de la solicitud, la
decisión de escalado y el evento se confirman bajo `BEGIN IMMEDIATE`. El
reintento por la misma clave devuelve la fila existente sin consumir rondas.
Si se agota el presupuesto global o el de ruta, Forge conserva la solicitud en
`requires_human`, sin superar el contador limitado, e incluye el código y la
razón concreta.

La detección compara los IDs exactos de TaskRole de las solicitudes aún
pendientes dentro de esa ruta. Si las dos últimas rutas pendientes se dirigen
a A y B y la nueva vuelve a A (A != B), Forge persiste la tercera como
`requires_human` con los tres IDs de solicitud y roles. Una escalación sin
resolver mantiene las nuevas solicitudes de esa ruta en espera humana. La
resolución/cancelación humana permite continuar, pero no reinicia el presupuesto
de ruta. Esto no sustituye la cadena inmutable ReviewReport → rework exacto →
nueva revisión ni cambia el límite separado de ocho rondas de rework.

`TaskQualityRequestResponse.quality_route` expone `id`, `round`, `limit`,
`source_task_role_id` y una `escalation` opcional con `reason_code`, `reason`,
`cycle_request_ids` y `cycle_task_role_ids`. La migración crea accounting para
las solicitudes existentes sin reescribir sus estados ni reproducir efectos.

## 6. Admisión, Start, recuperación y carreras

### 6.1 Secuencia

1. El Humano autoriza modo, coordinador, equipo y envelope con
   `expected_task_version` y `expected_coordination_version`.
2. El Humano solicita un turno con objetivo/restricciones e idempotency key.
3. En una transacción se revalidan Task, autorización, team digest, lifecycle,
   membership, capacidad del Agent y envelope; se escribe el request y el
   `task.coordination_requested` event.
4. El consumidor de orquestación clasifica el evento como wake de Actor exacto
   y admite una sola wake para el TaskRole/Actor autorizados.
5. `dispatch_one` revalida la wake, la autorización y el TaskRole snapshot;
   reserva el intento y crea el Execution workspace-free.
6. El Provider Start usa el recibo existente. Un resultado incierto deja el
   request en `uncertain` y no autoriza otro Start.
7. Al completar, el contrato de salida del turno se valida y se materializa en
   un Artifact `work_unit_plan` + fila de plan + Proposal o aprobación de
   envelope, en una transacción.
8. La materialización determinista crea WorkUnits y dependencias; en
   `prepare_and_execute`, prepara lo requerido y registra el launch intent bajo
   la autorización humana.
9. El dispatcher existente admite workers. Un fallo de preparación de
   Workspace deja un blocker visible; no se salta.

### 6.2 Tabla de carreras

| Carrera | Resultado requerido |
|---|---|
| Dos solicitudes con la misma clave y digest | Una sola request/wake/Execution; replay devuelve la misma. |
| Misma clave, distinto contenido | `409 coordination.idempotency_conflict`; no se crea trabajo. |
| Autorización revocada o cambiada antes del wake | La wake falla cerrada; no hay Execution. |
| Team/TaskRole/membership cambia antes del Start | `coordination_authorization_stale`; no hay Provider Start. |
| Pausa antes de materializar | El turno read-only puede terminar; la materialización y el launch quedan pendientes hasta resume. |
| Cancelación antes de materializar | No se crea WorkUnit ni launch; el request queda cancelado. |
| Cancelación durante el turno | El intento puede terminar como evidencia; no se aplica ningún efecto nuevo. |
| Dos materializadores | `BEGIN IMMEDIATE` + `UNIQUE(plan_artifact_id)`; uno crea, el otro devuelve el mismo recibo. |
| Plan alterado tras aprobación | `409 coordination.plan_digest_mismatch`; no se materializa. |
| Decision sobre Proposal vieja | `409`; una aprobación antigua no libera un plan nuevo. |
| Provider incierto | `uncertain`; reconciliación humana; sin segundo Start. |
| Crash entre plan y estado | Replay del mismo Execution/digest devuelve el mismo Artifact/fila; no duplica. |
| WorkUnit ya materializada | El recibo exacto se devuelve; no se crea otra. |
| Task cambia de versión durante la materialización | `409`; se revalida y se reintenta con el plan exacto. |

La pausa no impide que un turno ya iniciado termine su lectura. No se admite un
nuevo Provider Start de worker ni una materialización nueva mientras la Task
esté pausada. El resume revalida autorización, team, plan y lifecycle.

## 7. Contratos REST, MCP y CLI

### 7.1 REST

Todas las rutas viven bajo `/api/v1/tasks/{task_id}`. GET exige visibilidad de
Task; las mutaciones exigen la autoridad de mutación vigente y versiones
esperadas.

| Método y ruta | Efecto |
|---|---|
| `GET /coordination` | Autorización, equipo resuelto, envelope, blockers, último request y plan. |
| `POST /coordination/authorize` | Crea o reemplaza la autorización con `expected_coordination_version`, modo, coordinador, team digest e idempotency key. |
| `POST /coordination/revoke` | Revoca la autorización vigente; no cancela Executions ya iniciadas. |
| `POST /coordination/requests` | Solicita un turno `proposal` o `prepare_and_execute`; devuelve request y preflight. |
| `GET /coordination/requests/{request_id}` | Estado, wake/Execution, plan y bloqueo. |
| `POST /coordination/plans/{artifact_id}/approve` | Aprueba o rechaza un plan exacto con `decision` e idempotency key. |
| `POST /coordination/plans/{artifact_id}/materialize` | Materializa un plan aprobado; idempotente por digest. |
| `GET /coordination/plans/{artifact_id}` | Plan inspeccionable, WUs propuestas, Proposal/Decision y recibo. |

Ejemplos:

```http
POST /api/v1/tasks/task_123/coordination/authorize
```

```json
{
  "expected_task_version": 12,
  "expected_coordination_version": 1,
  "mode": "proposal",
  "coordinator_task_role_id": "role_orchestrator",
  "coordinator_actor": {"kind": "agent", "id": "agent_lead"},
  "idempotency_key": "authorize-firepos-planning-1"
}
```

```json
{
  "task_id": "task_123",
  "mode": "proposal",
  "authorization_version": 2,
  "coordinator": {
    "task_role_id": "role_orchestrator",
    "role": "orchestrator",
    "actor": {"kind": "agent", "id": "agent_lead"},
    "name": "Lead"
  },
  "team_digest": "…64 hex…",
  "envelope": {"schema_version": 1, "max_work_units": 4},
  "state": "ready",
  "blockers": [],
  "latest_request": null,
  "latest_plan": null
}
```

```http
POST /api/v1/tasks/task_123/coordination/requests
```

```json
{
  "expected_task_version": 12,
  "expected_coordination_version": 2,
  "objective_digest": "…64 hex del brief…",
  "idempotency_key": "coordinate-firepos-planning-1"
}
```

```json
{
  "request": {
    "id": "coord_req_1",
    "state": "requested",
    "mode": "proposal",
    "authorization_version": 2,
    "wake_id": null,
    "execution_id": null,
    "plan_artifact_id": null
  },
  "preflight": {
    "can_request": true,
    "blockers": []
  }
}
```

```http
POST /api/v1/tasks/task_123/coordination/plans/artifact_plan_1/approve
```

```json
{
  "expected_coordination_version": 2,
  "expected_plan_digest": "…64 hex…",
  "decision": "approve",
  "idempotency_key": "approve-firepos-plan-1"
}
```

```json
{
  "artifact_id": "artifact_plan_1",
  "task_id": "task_123",
  "request_id": "coord_req_1",
  "digest": "…64 hex…",
  "state": "proposed",
  "authorization_version": 2,
  "proposal_id": "proposal_plan_1",
  "decision_id": null,
  "work_units": [
    {
      "key": "design",
      "title": "Diseñar contrato de solo lectura",
      "role": "planner",
      "assigned_actor": {"kind": "agent", "id": "agent_planner"},
      "depends_on": [],
      "requires_integration": false
    }
  ],
  "materialization": null
}
```

Errores tipados mínimos:

| Código HTTP | Code | Cuándo |
|---|---|---|
| 403 | `coordination.not_authorized` | El Actor no tiene autoridad de mutación o no es miembro elegible. |
| 409 | `coordination.version_conflict` | Task/coordinación/plan cambiaron. |
| 409 | `coordination.authorization_stale` | Team, TaskRole, membership o envelope ya no coinciden. |
| 409 | `coordination.plan_digest_mismatch` | El contenido no es el aprobado. |
| 409 | `coordination.idempotency_conflict` | Misma clave con otro contenido. |
| 409 | `coordination.already_materialized` | El plan ya tiene recibo; se devuelve el existente si coincide. |
| 422 | `coordination.invalid_plan` | Esquema, referencias, ciclo, rol o Actor inválidos. |
| 422 | `coordination.unsupported_plan_item` | El plan pretende roles, memberships, capacidades o efectos fuera del envelope. |
| 503 | `coordination.host_unavailable` | No hay host/adapter read-only verificado. |

### 7.2 MCP

Las herramientas comparten el mismo servicio y permisos. Nombres propuestos:

- `forge_get_task_coordination`
- `forge_authorize_task_coordination`
- `forge_revoke_task_coordination`
- `forge_request_task_coordination`
- `forge_approve_task_coordination_plan`
- `forge_materialize_task_coordination_plan`

Una herramienta MCP no puede ampliar el envelope ni actuar como otro Actor. El
MCP de Project Copilot no recibe estas herramientas por defecto.

### 7.3 CLI

`forge-ctl` expone el mismo contrato cuando el backend esté implementado:

```text
forge-ctl task coordination show <task>
forge-ctl task coordination authorize <task> --mode proposal --role <role> --agent <agent>
forge-ctl task coordination revoke <task>
forge-ctl task coordination request <task> --idempotency-key <key>
forge-ctl task coordination approve <task> --plan <artifact>
forge-ctl task coordination materialize <task> --plan <artifact>
```

La CLI no crea WorkUnits directamente ni presenta un botón que finja ejecutar
antes de que el servicio exista.

## 8. Proyecciones y recorrido UX

### 8.1 Vista principal de Task

La vista principal responde, en este orden:

1. Objetivo y restricciones.
2. Equipo y coordinador, con nombres resueltos y harness/host; IDs sólo en
   detalles.
3. Modo y límites; quién autorizó y cuándo.
4. Estado de preparación: sin plan, preparando, plan listo, esperando
   aprobación, materializado, bloqueado.
5. Plan: WorkUnits propuestas, roles, Actores, dependencias, entregables,
   criterios, revisión y condiciones de avance.
6. Ejecución y entrega: WUs reales, Executions, Artifacts, revisiones, Gate y
   próxima acción.
7. Intervención humana: aprobar, ajustar, rechazar, pausar, revocar, pedir
   decisión.

La edición manual de roles, memberships, WorkUnits, intentos y logs vive en
**Advanced**, no desaparece.

### 8.2 Estados y acción principal

| Estado | Texto principal | Acción primaria | Acciones secundarias |
|---|---|---|---|
| Sin coordinador | «Falta un coordinador con capacidad de orquestación.» | Configurar equipo | Ver roles/plantillas |
| Coordinador no elegible | «El coordinador ya no es miembro activo.» | Elegir otro | Revisar equipo |
| Listo para preparar | «Describe el resultado; el equipo prepara el desglose.» | Preparar propuesta | Preparar y ejecutar |
| Preparando | «El coordinador está preparando el plan.» | Ver intento | Pausar Task |
| Plan listo | «Plan listo para revisar; no se ha creado trabajo.» | Aprobar y materializar | Ajustar objetivo / Rechazar |
| Esperando aprobación | «Hay trabajo fuera del límite autorizado.» | Revisar excepción | Rechazar / Decidir |
| Materializado | «Trabajo preparado; el dispatcher decidirá qué puede arrancar.» | Ver WorkUnits | Iniciar dispatch |
| Worker bloqueado | «Falta Workspace, lease, capacidad o dependencia.» | Resolver bloqueo | Ver blocker técnico |
| Task pausada | «No se admitirán nuevos workers hasta reanudar.» | Reanudar | Ver qué sigue activo |
| Autorización revocada | «La coordinación está revocada; el trabajo existente no se borra.» | Volver a autorizar | Ver historial |
| Error de host | «El harness no ofrece el sandbox read-only requerido.» | Elegir otro Agent | Ver capacidades |
| Decisión humana | «Se requiere una decisión de negocio o de alcance.» | Ver opciones | Pedir a Copilot contexto |

### 8.3 Reglas de presentación

- No mostrar «Ejecutar» si el backend sólo puede preparar. No mostrar «Listo»
  si no hay plan exacto.
- Mostrar «0 WorkUnits» sin presentarlo como error: es el estado que el turno
  debe resolver.
- IDs, digests y versiones en un disclosure de detalles; nunca como sustituto
  del nombre del Actor.
- Errores junto al campo y con la próxima acción; no cerrar un diálogo con una
  mutación fallida.
- La aprobación de un plan muestra el digest exacto y las WUs que se crearán.
- En móvil y dark, la acción principal debe seguir accesible; nombres largos
  deben envolver sin recortar el botón.

## 9. Transición y conservación

- Migración aditiva después de V157, previa re-verificación del head instalado.
- Backfill de `task_coordination` en `manual` para todas las Tasks existentes,
  versión 1, sin coordinador. Trigger para nuevas Tasks.
- Ningún wake, Execution, acción, Proposal, Decision o Artifact histórico se
  reetiqueta, borra o reinterpreta.
- No se arranca automáticamente ningún turno al migrar. La automatización no es
  retroactiva.
- Las Tasks existentes conservan su flujo manual y sus WorkUnits.
- Los nuevos registros se borran con el teardown guardado del Project o con la
  Task según el contrato que fije la migración; el historial de wakes y
  Executions mantiene sus guards actuales.
- SSE publica `task.coordination_authorized`, `task.coordination_requested`,
  `task.coordination_plan_proposed`, `task.coordination_plan_approved` y
  `task.coordination_plan_materialized` con IDs, estados, versiones y digests;
  nunca el contenido del plan, prompts, credenciales ni rutas.
- JSONL: el Execution del turno conserva su traza normal; el plan no se escribe
  como credencial ni como ruta de confianza.
- Project Copilot no cambia; no se le añade autoridad de Task por esta
  migración.

## 10. Pruebas y aceptación de PR0

PR0 se acepta como contrato si:

- [x] Architecture y Architecture V2 fueron leídos y los invariantes afectados
      están enumerados con su respuesta.
- [x] El inventario de callers/almacenamiento/SSE/snapshots/recovery está
      identificado para las fronteras tocadas.
- [x] La dirección de arranque está decidida y la alternativa de WorkUnit de
      coordinación está comparada.
- [x] La autorización de Task, el team digest, los modos y el envelope están
      fijados.
- [x] El contrato de salida del plan y la materialización atómica están
      definidos.
- [x] Hay ejemplos de request/response y estados UX.
- [x] La transición conservadora no reescribe historia ni arranca trabajo.
- [ ] Implementación y pruebas de runtime: PR1–PR3.
- [ ] Aceptación E2E, móvil/dark/teclado y migración sobre copia real: PR6.

Pruebas obligatorias antes de cerrar PR2/PR3:

- DB: migración desde V157 con fixtures; backfill `manual`; guards de versión,
  cascade/teardown y replay.
- Servicios: autorización, team drift, request, wake exacto, plan, aprobación,
  materialización, pausa/cancelación, proveedor incierto y revocación.
- API/MCP/CLI: permisos, DTOs, errores tipados, idempotencia y SSE.
- UI: dos modos, estados vacíos, blockers, aprobación y edición avanzada;
  cuatro viewports, light/dark y teclado.
- Carreras: barreras deterministas, no sleeps; dos consumidores y dos
  materializadores.
- No se sustituye aceptación con proveedores reales por fixtures locales.

## 11. Gate de diseño

Una Task sin WorkUnits puede solicitar preparación sin conceder escritura.
Todo worker sigue exigiendo WorkUnit creada, asignación exacta, membership
activa, admisión y WorkspaceLease cuando corresponda. El turno de coordinación
no crea un Execution de worker, no abre Workspace y no escribe TaskLifecycle,
Gate ni revisión. La materialización es la única frontera que crea WorkUnits, y
lo hace desde un plan exacto, aprobado o dentro del envelope, con recibo
idempotente.

Cualquier implementación que necesite una WorkUnit ficticia, un segundo
scheduler, autoridad de Project copiada, o una aprobación implícita del modelo
queda fuera de este contrato y requiere una nueva decisión de producto.
