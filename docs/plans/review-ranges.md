# Plan — Revisión por rangos de commits y cobertura por repo

Fecha: 2026-11-25. Baseline inspeccionado: `feat/multirepo` en `3909fb37`
(0.9.1, esquema hasta V183). Documento de diseño: no declara capacidad
implementada. No se ejecutó Cargo, pnpm ni CI. Las referencias a código y a
triggers vigentes deben revalidarse al congelar contratos (fase C0).

**Estado:** decisiones DR1–DR14 resueltas (ver «Decisiones del Human»); pendiente congelar contratos (C0).

Metodología elegida:
`agentic-engineering/model/authority-centred-delivery.md`, sección “Review
ranges and coverage”.

## Problema

Hoy una revisión formal se ata a exactamente un resultado de WorkUnit
(`work_unit_review_subject`, con filas hijas por repo desde V182). Por eso:

- el número de WorkUnits dicta el número y alcance de las revisiones;
- revisar el último WorkUnit de una cadena secuencial solo cubre su propio diff;
- un packet que toca varios WorkUnits o repos no se revisa de forma coherente;
- merge readiness no puede saber si cada línea del candidato de un repo fue
  revisada.

## Objetivo

La revisión pasa a ser un cursor sobre la rama de integración de cada repo:

| Unidad | Qué es | Quién la decide |
|---|---|---|
| Execution | lo que hace un Actor sobre un WorkUnit | plan / dispatcher |
| Integración | rama de integración por `(task, repo)` | operación serializada |
| Revisión | rango `base..head` de esa rama | política de riesgo y tamaño |
| Aceptación | una PR por repo, merge Human | Human |

Regla de cobertura: el candidato de cada repo cambiado debe estar cubierto por
rangos revisados desde la base de la Task, sin hallazgos P0/P1 abiertos. Es
determinista y es condición de merge readiness.

## No objetivos

- Revisión progresiva de commits de una rama de WorkUnit antes de integrarlos.
  El head de un rango es siempre un commit integrado (ver DR9).
- Equivalencia por patch-id tras reescribir historia (rebase = volver a revisar).
- Cobertura por `git blame`. El blame solo se usa para enrutar correcciones.
- Merge automático, o impedir un merge hecho directamente en GitHub.
- Retirar las revisiones de resultado de WorkUnit.

## Baseline verificado en el código

1. `ReviewReport.findings` no tiene estructura: `SubmitReviewReportRequest`
   usa `Vec<String>` y `parse_review_result` (`FORGE_RESULT` v1,
   `collaboration_service.rs`) acepta cualquier JSON. **No existe severidad ni
   cierre de hallazgos**; hay que crearlos.
2. El triage `review_findings` decide una disposición por report completo
   (`correct|recheck|human`, `task_rework/triage.rs`).
3. La vista `task_review_coordination_source_current` (V168) exige
   `work_unit_review_subject` y `task_rework_automation_intent`.
4. Los triggers `work_unit_review_independent_execution` y
   `..._allocation` (V151/V162) rechazan una Execution de un rol
   `formal_review` sin sujeto de resultado congelado. Hay que reemplazarlos.
5. `validate_plan` exige `review_subject_key` en todo WorkUnit
   `formal_review`.
6. La integración usa `git merge --no-edit` (puede ser fast-forward) y registra
   `source_sha`, `target_before_sha`, `target_after_sha` por repo en
   `work_unit_repository_integration` (V183).
7. `MergeService::validate_gate_candidate` exige que cada `review_report` y
   `validation_run` del Gate nombren exactamente el commit candidato. Un rango
   delta no cumple esa forma; la cobertura necesita un input propio.
8. La base por repo sale de `workspace.before_sha`, que es opcional; en un repo
   único se recurre a `integration.target_before_sha`.
9. No existe clase de riesgo en Task ni Project.

## Invariantes tocados

| INV | Efecto |
|---|---|
| INV-007 | La cobertura es un hecho derivado; no se añade ningún estado de TaskLifecycle. |
| INV-008 | Rangos, reports y resoluciones son inmutables o append-only; ningún WorkUnit antiguo se reabre. |
| INV-009 | `subject.kind=commit_range` clasifica; la autoridad sigue siendo `formal_review`. |
| INV-016 | El WorkUnit revisor tiene un alcance exacto: un repo y un rango congelado. |
| INV-017 | Las correcciones usan workspaces aislados; el workspace revisor es de solo lectura y sin lease. |
| INV-018 / INV-020 | Solo un revisor crea hallazgos, fija severidad o declara arreglado. El coordinador enruta; no cierra ni rebaja. |
| INV-019 / INV-021 / INV-022 | El coordinador pide rangos y dirige correcciones con salidas tipadas y acotadas, por eventos. |
| INV-027 | El report sigue siendo un Artifact `review_report`; el ledger es su proyección indexada, escrita en la misma transacción. |
| INV-028 | La cobertura no sustituye ValidationRun ni al revés. |
| INV-029 | El rework son hallazgos exactos más WorkUnits enlazados, sin reescribir prompts. |
| INV-030 | Cobertura, independencia, severidad bloqueante y waivers los aplica Rust/SQLite. |
| INV-031 | Los disparadores son política explícita y versionada, no un workflow DSL. |
| INV-032 | Commits no registrados, historia reescrita o base desconocida fallan cerrado con motivo persistido. |
| INV-033 | Cada rango conserva política, contribuyentes y hallazgos de entrada congelados. |
| INV-034 | Una sola autoridad para la política (`task_review_policy_revision`); Gate, envelope y plan solo la referencian por versión y digest. |
| INV-035 / INV-036 | Migraciones aditivas, nada se retira; la documentación cambia en la misma entrega. |

## Modelo

### Política de revisión

Revisión inmutable y versionada por Task, `task_review_policy_revision`:

```json
{
  "schema_version": 1,
  "risk_class": "r1",
  "coverage_required": true,
  "trigger_mode": "auto",
  "reviewer_role": "reviewer",
  "required_reviewers": 1,
  "require_human": false,
  "size_threshold": { "changed_lines": 800, "changed_files": 25 },
  "coordinator_may_request": true,
  "max_checkpoints_per_repo": 12
}
```

| Clase | `coverage_required` | Disparadores automáticos |
|---|---|---|
| R0 | no | ninguno; petición Human, o umbral de tamaño |
| R1 | sí | baseline por repo antes de la PR; deltas tras correcciones; umbral |
| R2 | sí | tras los WorkUnits nombrados; delta final; deltas; umbral |

Reglas:

- **Dónde vive.** El Project guarda un valor por defecto que se **copia** a la
  Task al crearla; no hay herencia viva. La Task es la autoridad.
- **Quién la cambia.** Owner/admin, con `expected_version`. El coordinador y el
  plan nunca la relajan; solo pueden **añadir** checkpoints.
- **Sin política** (todas las Tasks existentes): comportamiento actual, sin
  condición de cobertura. No hay backfill.
- Aunque `coverage_required=false`, un hallazgo P0/P1 abierto de cualquier rango
  de la Task bloquea merge readiness.
- `trigger_mode=manual`: Forge crea el checkpoint `pending` y un Human asigna el
  revisor. En `auto`, Forge asigna solo si hay exactamente un miembro elegible e
  independiente para `reviewer_role`; si no, el checkpoint queda `blocked` y se
  levanta una decisión Human.

> **DECISIÓN HUMANA DR1 — lugar de la política.** Recomendado: tabla propia por
> Task, referenciada por Gate y plan. Alternativa: dentro del envelope de
> coordinación (no sirve a Tasks manuales y mezcla autoridad de dos sujetos).
>
> **DECISIÓN HUMANA DR2 — defecto.** Recomendado: opt-in por Project (sin valor
> por defecto, nada cambia). Alternativa: R1 para Tasks nuevas con repo
> (cambio de comportamiento que iría en CHANGELOG como **Changed, breaking**).
>
> **DECISIÓN HUMANA DR3 — R0.** Recomendado: sin cobertura obligatoria; el merge
> Human es la aceptación. Esto relaja la frase “every line in a pull request
> lies inside a reviewed range” para R0. Alternativa: R0 exige un rango revisado
> por Human antes de `ready_to_merge`.
>
> **DECISIÓN HUMANA DR4 — umbrales.** Valores propuestos 800 líneas / 25
> ficheros, sin exclusiones de ficheros generados. Los binarios cuentan como
> fichero.
>
> **DECISIÓN HUMANA DR5 — quórum R2.** Recomendado: 1 revisor, Human no
> obligatorio, mismo perfil de Agent permitido con sesión nativa aislada (regla
> actual). Alternativa para R2: perfil distinto o Human en el delta final.

### Base de la Task por repo

`task_review_base(task_id, repo_id, base_commit, pinned_at, source)`, inmutable.
Se fija al activar la política si ya hay integraciones (el `target_before_sha`
de la primera integración con éxito de ese repo) o, si no, al crear/preparar el
workspace de integración (`workspace.before_sha`). Si no se puede determinar,
la cobertura es `blocked: review.coverage.base_unknown`; nunca se infiere del
HEAD actual.

### Sujeto de rango

```text
ReviewRange {
  id, task_id, repo_id, repository_set_revision,
  base_commit, head_commit,
  base_tree, head_tree,
  range_digest,                 // sha256 canónico de todo lo anterior
  integration_workspace_id,
  head_integration_id           // integración con éxito cuyo after_sha = head
}
```

- `head_commit` es el `target_after_sha` de una integración con éxito de ese
  `(task, repo)`.
- `base_commit` es la base de la Task (baseline, o re-baseline completo) o el
  head de un rango ya cubierto (delta). Es ancestro estricto de `head_commit`.
- Forge resuelve ambos commits. Ninguna petición (Human, MCP o coordinador)
  suministra SHAs.
- `range_digest` identifica el rango. El `workspace_snapshot_digest` del
  checkout revisor se sigue calculando con la función actual y viaja en el
  report, separado.

### Checkpoint

`review_checkpoint` es la intención durable:

- estados `queued → pending → in_review → reviewed`, más `blocked` y
  `superseded`;
- razones (lista, se acumulan): `baseline`, `delta`, `after_work_unit`,
  `size_threshold`, `requested`, `final`, `recheck`;
- **como máximo un checkpoint activo** (`pending|in_review`) por `(task, repo)`
  (índice único parcial). Los siguientes quedan `queued` con su head objetivo; su
  base se resuelve al activarse, a partir de la cobertura vigente;
- un `pending` sin Execution cuyo head quedó atrás puede sustituirse por otro con
  head más nuevo (CAS por `version`); un `in_review` nunca se cancela porque el
  head avance;
- idempotencia: `UNIQUE(task_id, repo_id, base_commit, head_commit, attempt)`;
- al pasar a `pending` crea en la misma transacción `required_reviewers`
  WorkUnits revisores (IDs deterministas), `requires_integration=false`, alcance
  `work_unit_repository={repo}`, y congela contribuyentes y hallazgos abiertos de
  entrada (`input_findings_digest`).

Son WorkUnits creados explícitamente por una política autorizada por un Human,
como hoy los de rework. No consumen `max_work_units` del plan; los limita
`max_checkpoints_per_repo`. Agotado el límite, decisión Human.

### Disparadores

Un `ReviewScheduler` reacciona a hechos durables (integración con éxito,
WorkUnit terminal, report, cambio de política, replan) y tiene un escáner de
recuperación. EventBus solo despierta.

Por repo cambiado, con `frontier` = nodo cubierto más avanzado:

1. **after_work_unit (R2):** todos los WorkUnits nombrados por un checkpoint del
   plan están integrados en el repo → head = último `after_sha` de ellos.
2. **size_threshold:** `numstat(frontier..head)` supera líneas o ficheros.
3. **baseline/final/delta (R1, R2):** no queda ningún WorkUnit de escritura no
   terminal ni integración pendiente cuyo alcance incluya el repo, y
   `frontier != head`.
4. **requested:** Human, o coordinador si `coordinator_may_request`.

Un repo sin cambios (árbol igual a la base) no genera checkpoints ni exige
cobertura (coherente con DM2).

> **DECISIÓN HUMANA DR6 — checkpoints R2 bloqueantes.** Recomendado: no
> bloquean a los dependientes (la revisión corre en paralelo y el delta corrige).
> Alternativa: `blocking=true` por checkpoint, que impide admitir los
> dependientes hasta no tener P0/P1 abiertos; más seguro, serializa el plan.

### Independencia y contribuyentes

Para cada commit de `rev-list(head ^base)`:

1. si pertenece a `rev-list(target_after ^target_before)` de una integración con
   éxito del repo, se atribuye al WorkUnit de la **primera** integración que lo
   introdujo (incluye el merge commit de Forge);
2. si es ancestro de la rama por defecto del repo, es `upstream`;
3. cualquier otro es un commit no registrado: el checkpoint queda `blocked`
   (`review.range.unrecorded_commits`).

`review_range_contributor` materializa todos los Actors con Execution en esos
WorkUnits. Un Human contribuyente no puede revisar el rango; un Agent del mismo
perfil exige sesión nativa nueva, sin fork del historial contribuyente (misma
regla V162). Se aplica en asignación, admisión y entrega del report.

> **DECISIÓN HUMANA DR7 — commits externos** (alguien hace commit a mano en la
> rama de integración). Recomendado: bloquear; salida por decisión Human
> owner/admin que los declare contribución de un Actor concreto (excluido como
> revisor). Alternativa: tratarlos como revisables sin atribución.

### Report de rango y hallazgos

Los reports de rango exigen `FORGE_RESULT` `schema_version: 2` (Agent) o
`structured_findings` (Human):

```json
{
  "schema_version": 2, "kind": "review", "verdict": "fail",
  "summary": "…", "criteria": [], "questions": [], "evidence_considered": [],
  "findings": [{
    "label": "F-1", "severity": "P1", "title": "…", "evidence": "…",
    "invariant": "tenant scope enforced in data access",
    "locations": [{"path": "src/a.rs", "line_start": 120, "line_end": 131}]
  }],
  "resolutions": [{"finding_id": "…", "outcome": "fixed", "evidence": "…"}]
}
```

El sujeto del Artifact añade `kind: "commit_range"` y los campos de
`ReviewRange` (con `repo_id`). `severity ∈ {P0,P1,P2,P3}`; P0 bloquea release,
P1 bloquea merge, P2 se arregla o acepta, P3 backlog. Bloqueantes: P0 y P1.

Validación al ingerir (todo o nada):

- sujeto exactamente igual al checkpoint; revisor independiente y autorizado;
- `resolutions` solo nombra hallazgos del conjunto de entrada congelado, y
  **cada** bloqueante de entrada recibe `fixed`, `not_reproduced` o
  `still_open`;
- `pass` no admite P0/P1 nuevos ni de entrada `still_open`;
- `request_changes` exige al menos un hallazgo nuevo o `still_open`;
- `questions` no cubre nada: el checkpoint pasa a `blocked` con decisión Human;
- cada `path` existe en el árbol head o aparece borrado en el diff; como máximo
  100 hallazgos y 16 ubicaciones por hallazgo.

Un report inválido sigue la ruta de fallo técnico de revisión; no es un veredicto.

### Cierre de hallazgos

`review_finding` (inmutable, ID asignado por Forge, `anchor_commit` = head del
rango) y `review_finding_resolution` (append-only):

| Resolución | Quién | Ancla |
|---|---|---|
| `fixed` | report de un rango cuyo head desciende del ancla | head de ese rango |
| `not_reproduced` | report `recheck` independiente | head de ese rango |
| `waived` | decisión Human exacta, owner/admin | candidato al decidir |

Un hallazgo está **abierto para un candidato C** si no tiene ninguna resolución
cuya ancla sea ancestro (o igual) de C. Consecuencias:

- reescribir historia y perder el arreglo **reabre** el hallazgo;
- los hallazgos nunca desaparecen porque su rango quede huérfano;
- la disposición del coordinador no cierra nada;
- un `still_open` posterior no necesita fila: la ausencia de resolución basta.
  Si un hallazgo ya resuelto reaparece, el revisor crea uno nuevo.

> **DECISIÓN HUMANA DR8 — waivers y recheck.** Recomendado: P1 waivable por
> owner/admin; **P0 no waivable**; `not_reproduced` puede emitirlo cualquier
> revisor independiente en sesión nueva, incluido el autor original del
> hallazgo. Alternativa: exigir un revisor distinto.

### Regla de cobertura

Por cada repo cambiado, con base `B` y candidato `C` (HEAD de la rama de
integración, igual al último `after_sha` registrado):

- nodos: commits; `B` es alcanzable;
- arista `base → head` por cada rango **válido**: `required_reviewers` Actors
  distintos entregaron report `pass` o `request_changes` sobre el mismo
  `(base, head)` (con Human si `require_human`), `base` es ancestro de `head` y
  `head` es ancestro o igual de `C`;
- **cubierto** ⇔ `C` es alcanzable desde `B` siguiendo aristas;
- `frontier` = nodo alcanzable con menos commits hasta `C` (desempate por SHA).

El repo **satisface** la condición si: está cubierto, no hay hallazgos P0/P1
abiertos para `C` (de cualquier rango de la Task en ese repo), no hay checkpoint
activo del repo, y ninguna línea de revisión de resultado vigente que toque el
repo tiene último veredicto `request_changes`. Con `coverage_required=false`
solo se exigen las condiciones de hallazgos y checkpoint activo.

Un rango `request_changes` **sí** cuenta como revisado: sus líneas se vieron; lo
que bloquea son sus hallazgos abiertos. Así un delta cierra la corrección sin
repetir el baseline. Los solapes son válidos. No basta que un rango empiece en un
ancestro cualquiera: su base debe ser un límite ya cubierto, para que un revert
intermedio no quede fuera de todo diff.

Estados proyectados: `not_required`, `unchanged`, `uncovered`, `partial`,
`in_review`, `findings_open`, `history_rewritten`, `blocked`, `satisfied`.

**El candidato avanza:** la cobertura pasa a `partial` (`frontier..C`), la
GateEvaluation queda obsoleta y el scheduler crea el delta.

**Historia reescrita:** las aristas cuyo head no es ancestro de `C` dejan de
contar (no se borran). La cobertura reinicia desde el nodo alcanzable que siga
siendo ancestro; tras un rebase completo, desde una base nueva, que requiere una
revisión explícita de `task_review_base` con decisión Human. Si `B` deja de ser
ancestro de `C`: `blocked: review.coverage.base_not_ancestor`.

**La rama por defecto se mergea en la de integración:** esos cambios entran en el
diff del siguiente delta y se revisan (conservador).

> **DECISIÓN HUMANA DR9 — alcance de cobertura v1.** Recomendado: (a) las
> revisiones de resultado de WorkUnit **no** crean aristas, ni siquiera tras
> fast-forward (una sola regla; en R1 el baseline las hace redundantes); (b) sin
> revisión pre-integración; un WorkUnit indivisible grande se revisa por umbral
> tras integrar. Alternativa (a'): contar un `pass` de resultado cuando
> `after_sha == source_sha` y `before_sha` es un nodo cubierto.

## Triage y enrutado de correcciones

Un report de rango con veredicto `request_changes` despierta al coordinador con
la solicitud existente `review_findings` (sin kind nuevo, sin rebuild de la
tabla). El contexto incluye rango, hallazgos con severidad y ubicaciones,
atribución y cobertura por repo.

**Atribución** (determinista, calculada al ingerir y persistida en
`review_finding_location`): `git blame` de las líneas en `anchor_commit`; cada
commit se mapea a su integración como arriba. Resultado por ubicación: WorkUnit,
Execution (por `execution_repository_result.before/after`) y Actor; o
`preexisting`, `upstream`, `unattributed` (hallazgo sin línea, borrado o commit
sin integración). El autor primario es el de más líneas; en empate, la
integración más reciente. Los trailers `Agent-Run` son informativos, no
autoridad.

Salida tipada nueva (`schema_version: 3` para sujetos de rango; 1 y 2 intactas):

```json
{ "schema_version": 3, "request_id": "…", "task_id": "…",
  "authorization_version": 4,
  "range_review": {
    "review_report_artifact_id": "…", "report_digest": "…",
    "findings": [{"finding_id": "…", "disposition": "correct",
                  "rationale": "…", "instructions": "…"}]
  },
  "outputs": [] }
```

- Cada P0/P1 nuevo o `still_open` recibe exactamente una disposición
  `correct|recheck|human`; P2/P3 admiten además `defer`.
- El coordinador no elige commits, workspaces ni Actor.
- `correct`: Rust agrupa por WorkUnit autor y crea **un WorkUnit de corrección
  nuevo** por grupo, asignado al mismo Actor y TaskRole si siguen elegibles; si
  no (o `unattributed`/`preexisting`), al TaskRole implementador sin Actor, o
  decisión Human si no hay uno único. Nunca se reabre el WorkUnit original.
- La corrección parte del **HEAD de integración vigente**, no del resultado
  antiguo, para integrar limpio y que el delta contenga solo la corrección. No
  crea `work_unit_result_requirement`; usa `review_correction_link`. Varias
  correcciones del mismo repo se encadenan con dependencias.
- `recheck`: checkpoint `recheck` con el mismo rango, `attempt+1`.
- `human`: decisión Human ligada a los hallazgos.
- Todo (disposiciones, WorkUnits, enlaces, una ronda de rework por report) es una
  transacción idempotente. Autorización, pausa y revocación se revalidan dentro.
- Sin coordinador (modo manual), `request_rework` acepta un report de rango y
  aplica la misma atribución.

> **DECISIÓN HUMANA DR10 — micro-fix sobre rangos.** Recomendado: no en v1
> (`coordination.output_not_permitted`), porque sus límites se miden desde un
> resultado revisado. Alternativa: medir desde el HEAD base de la corrección.

## Cambios al coordinador (plan y prompt)

`WorkUnitPlan` gana campos opcionales, omitidos si están vacíos (misma
serialización y digest actuales):

```json
"review_checkpoints": [
  {"key": "after-contract", "after_work_unit_keys": ["contract"],
   "repository_ids": [], "reason": "otros WorkUnits dependen del contrato"}
],
"review_policy_digest": "…"
```

Validación: máximo 8; claves únicas; los WorkUnits nombrados existen, tienen
`requires_integration=true` y escriben los repos pedidos; `repository_ids` vacío
= todos los escritos por ellos; el digest debe ser la política vigente al
materializar. Una política R2 con dos o más WorkUnits dependientes y sin ningún
checkpoint se rechaza con `coordination.plan_review_checkpoint_required`.
Los checkpoints materializados se guardan en `review_plan_checkpoint`. En una
replan, quitar o mover uno pendiente se clasifica como
`scope_or_authority_change` y exige la decisión Human exacta; añadir no.

Salida tipada nueva, admitida solo en turnos tipados y solo si figura
explícitamente en `allowed_actions` del rol (política asimétrica actual):

```json
{"type": "request_range_review", "repo_id": "…", "reason": "≤1024 bytes"}
```

Sin commits, rutas ni revisor. Un `head` ya cubierto responde
`review.range.nothing_to_review`.

Prompt del coordinador, cuando la Task tiene política con cobertura:

- Forge crea las revisiones del código por repo; **no** planificar WorkUnits
  `review_subject_key` para trabajo de repositorio;
- en R2, declarar `review_checkpoints` tras los WorkUnits sobre los que otros
  construyen;
- `review_subject_key` sigue válido para entregables sin repo.

`validate_plan` rechaza en ese modo un `review_subject_key` que apunte a un
WorkUnit con `requires_integration=true`
(`coordination.plan_result_review_redundant`).

> **DECISIÓN HUMANA DR11.** Rechazar (recomendado, evita revisiones duplicadas)
> frente a permitirlo como revisión adicional que no cuenta para cobertura.

## Gate y merge readiness

Gate policy **`schema_version: 4`** = v3 (checks por repo, válido también con un
repo) más:

```json
"review_coverage": {"review_policy_revision": 2, "review_policy_digest": "…"}
```

- v1–v3 no cambian; no aceptan `review_coverage`.
- Un input `review_coverage` por repo cambiado: `input_id=repo_id`,
  `input_version` = revisión del ledger de cobertura, `input_digest` = sha256 de
  la instantánea canónica (base, candidato, aristas con IDs y digests de report,
  hallazgos abiertos, resoluciones, checkpoint activo).
- `task_review_coverage_state(task_id, repo_id, revision)` se incrementa en la
  misma transacción que escriba integración, checkpoint, report, resolución o
  política; `gate_evaluation_inputs_are_current` la compara.
- **Fail closed:** si la Task tiene una política vigente, una evaluación
  `merge_readiness` cuya política no sea v4 con ese digest nunca produce
  `ready_to_merge` ni admite merge (`gate.review_coverage_missing`). Activar o
  cambiar la política revisa el Gate en la misma operación. Todos los
  generadores automáticos de Gates (rework, micro-fix, check runner,
  integración) conservan `review_coverage`.
- `review` (quórum sobre reports concretos) pasa a ser opcional en v4; si está,
  se exige además. Un report de rango no satisface un requisito `review` v1–v3.
- `validate_gate_candidate` sigue exigiendo la WorkUnitIntegration exacta y los
  ValidationRuns sobre el candidato; en v4 recalcula la cobertura bajo el
  `task_integration_operation` claim y exige igual digest. La igualdad
  “commit del report = candidato” se aplica solo a inputs `review_report`.
- La comprobación se repite **antes de cada push** de la rama de PR (publicación
  y rework). Un batch multirepo exige todos los repos cambiados satisfechos antes
  de admitirse.
- Si el head remoto de la PR difiere del candidato cubierto, la UI y la ficha de
  decisión lo muestran. Forge no puede impedir un merge hecho en GitHub; lo
  observado externamente se registra como hoy.

## Migraciones

Siguientes números tras V183. No se edita ninguna histórica. Aditivas y sin
backfill de políticas, rangos ni severidades.

**V184 — modelo de rangos**

| Tabla | Claves y restricciones |
|---|---|
| `project_review_policy` | `project_id` PK, `version`, `policy_json`, `digest` |
| `task_review_policy_revision` | PK `(task_id, revision)`, único `current` por Task, JSON válido, digest de 64, inmutable salvo supersesión |
| `task_review_base` | PK `(task_id, repo_id, revision)`, inmutable, `decision_id` obligatorio si `revision>1` |
| `review_range` | `UNIQUE(task_id, repo_id, base_commit, head_commit)`, `base≠head`, inmutable |
| `review_checkpoint` | FK rango (NULL en `queued`), `state`, `reasons_json`, `attempt`, `version`; único parcial activo por `(task_id, repo_id)` |
| `review_plan_checkpoint` | PK `(task_id, plan_artifact_id, key)`, WorkUnits y repos |
| `work_unit_review_range` | PK `review_work_unit_id`, FK checkpoint y rango, `input_findings_json` + digest, inmutable |
| `review_range_contributor` | `(range_id, work_unit_id, execution_id, actor_kind, actor_id)` |
| `review_range_report` | PK `report_artifact_id`, FK rango/checkpoint/Execution, `verdict`, digest |
| `review_finding` | ID, report, repo, ordinal, label, severidad CHECK, `anchor_commit`, texto acotado; inmutable |
| `review_finding_location` | ruta, líneas, atribución congelada |
| `review_finding_resolution` | append-only; `fixed/not_reproduced` exigen report, `waived` exige decisión y severidad ≠ P0 (según DR8) |
| `review_finding_triage` | PK `finding_id`+`request_id`, disposición inmutable |
| `review_correction_link` | `(correction_work_unit_id, finding_id)`, `base_commit` |
| `task_review_coverage_state` | PK `(task_id, repo_id)`, `revision` monótona |

Guards: todo mismo Task/repo/revisión de conjunto; repos `context` rechazados; un
WorkUnit tiene `work_unit_review_subject` **o** `work_unit_review_range`, nunca
ambos; borrado solo en teardown de Project (`project_deletion_guard`), que se
actualiza en orden de dependencias.

**V185 — adaptar autoridad existente**

- Reemplazar `work_unit_review_independent_allocation` y `..._execution` para
  aceptar sujeto de rango (listo = checkpoint `pending|in_review` y
  contribuyentes congelados) y aplicar independencia sobre
  `review_range_contributor`. La rama de resultado queda semánticamente igual.
- Revisar y, si hace falta, reemplazar los guards de V162 (aislamiento de
  sesión) y de V182 sobre workspace group del revisor.
- Recrear `task_review_coordination_source_current` y
  `..._request_current` con `UNION` de la rama de rango (sin
  `task_rework_automation_intent`), conservando sus condiciones de autorización,
  lifecycle y dispatch.
- Rebuild de `gate_evaluation_input` (patrón V155) para `review_coverage`, y del
  catálogo de `schema_version` de políticas si está restringido por CHECK,
  recreando índices y triggers y comprobando preservación de filas.

El implementador parte de la definición **efectiva** de cada trigger sobre el
esquema V183, no de la de su primera migración.

## DTOs, API, MCP, CLI y SSE

REST (cursores keyset y `items` en listas):

| Ruta | Efecto |
|---|---|
| `GET/PUT /tasks/{id}/review-policy` | leer; escribir con `expected_version` (owner/admin) |
| `PATCH /projects/{id}` | `default_review_policy` |
| `GET /tasks/{id}/review-coverage` | por repo: base, candidato, frontier, estado, rangos, contadores P0–P3, motivo |
| `GET /tasks/{id}/review-checkpoints` | lista |
| `POST /tasks/{id}/review-checkpoints` | `{repo_id, reason, idempotency_key}`; Forge resuelve commits |
| `GET /tasks/{id}/review-findings` | filtros repo, severidad, abierto |
| `POST /tasks/{id}/review-findings/{finding_id}/waiver` | `{expected_coverage_revision, rationale}`; crea la Decision exacta |

DTOs nuevos en `crates/api-types/src/review_ranges.rs` (tipos TS generados).
Extensiones compatibles:

- `SubmitReviewReportRequest`: `structured_findings?`, `resolutions?`. En rango
  son obligatorios y `findings` debe ir vacío; en resultado de WorkUnit se
  rechazan en v1.
- `WorkUnitResponse`: `review_range?` junto a `review_subject?`.
- El submit Human usa la ruta `review-report` existente del WorkUnit revisor.
- Gate DTOs: documento v4.

MCP: `forge_get_review_coverage`, `forge_set_task_review_policy`,
`forge_request_range_review`, `forge_waive_review_finding` (nunca disponible a
Project Copilot ni a Agents); `forge_submit_work_unit_review` amplía su schema.
forge-ctl: sin cambios en v1 (**DECISIÓN HUMANA DR12** si se quiere
`forge-ctl task review-coverage`).

Errores tipados: `review.policy.version_conflict` (409),
`review.range.nothing_to_review`, `review.range.checkpoint_active` (409),
`review.range.unrecorded_commits`, `review.range.head_not_integrated`,
`review.report.subject_mismatch` (409), `review.report.invalid_findings` (422),
`review.coverage.base_unknown`, `review.coverage.base_not_ancestor`,
`review.finding.not_waivable`, `gate.review_coverage_missing`.

SSE y eventos de dominio (solo IDs, estados, severidades y contadores; nunca
texto de hallazgos, rutas ni diffs):

`task.review_policy_updated`, `review.checkpoint_created`,
`review.checkpoint_state_changed`, `review.range_reported`,
`review.findings_triaged`, `review.finding_resolved`,
`review.coverage_changed`.

Compatibilidad: no hay cambio breaking si DR2 es opt-in. CHANGELOG
`Unreleased` → **Added** (rangos, cobertura, severidades, Gate v4, rutas) y
**Changed** (solo para Tasks con política: planes con revisión de resultado
sobre trabajo de repo se rechazan).

## Concurrencia, fallos y carreras

Patrón común: los hechos git (SHA, ancestry, rev-list, blame, numstat) se
calculan fuera de la transacción; `BEGIN IMMEDIATE` revalida las filas y
versiones de las que dependen antes de escribir. Ninguna transacción espera a
git.

| Carrera / fallo | Resolución |
|---|---|
| Dos schedulers crean el mismo checkpoint | único activo por repo + idempotencia; replay devuelve el existente |
| Integración durante la creación | la tx exige que `head` siga siendo un `after_sha` registrado; un head más antiguo es válido, el delta sigue después |
| Integración durante la revisión | el rango es inmutable; el report cuenta; cobertura `partial` |
| Política cambia durante la revisión | el report se guarda; la arista se evalúa con la política **vigente** (quórum/Human); el Gate queda obsoleto |
| Report tardío de checkpoint `superseded` o sesión revocada | se rechaza como `subject_mismatch` |
| Dos reports concurrentes | cada uno atómico (Artifact, filas, resoluciones, estado, revisión de cobertura); CAS de `checkpoint.version` |
| Waiver contra candidato que se movió | `expected_coverage_revision` → 409 |
| Coverage cambia entre Gate y merge | digest recalculado bajo claim de integración → 409 y nueva evaluación |
| Caída entre report y triage | escáner de solicitudes pendientes existente |
| Caída al preparar workspace revisor | checkpoint `pending`; preparación idempotente por WorkUnit |
| Execution revisora falla o proveedor incierto | reglas actuales de retry/fallback del mismo WorkUnit; sin arista |
| Revisor no disponible o no independiente | `blocked` + decisión Human; nunca aprobación automática |
| Commit del rango recolectado por git tras rewrite | ancestry no demostrable = no ancestro; fail closed |
| Repo retirado del conjunto por replan | checkpoints abiertos `superseded`; historial intacto; replan bloqueado con checkpoint `in_review` |
| Task pausada/cancelada | sin admisiones nuevas; el scheduler no crea checkpoints |
| Límite de checkpoints o rondas agotado | decisión Human |

El workspace revisor es un grupo de solo lectura con el repo en `head_commit`
(detached) más los repos `context` fijados; sin WorkspaceLease y con
comprobación posterior de que no hubo escrituras. El prompt recibe base, head,
diffstat, contribuyentes y hallazgos de entrada; el diff se lee con git dentro
del workspace, sin truncado silencioso.

## UI

En el panel de coordinación de la Task (`task-coordination-panel.tsx` monta un
componente nuevo `task-review-coverage-panel.tsx`):

- una fila por repo: estado, `base → frontier → candidato` (SHA cortos),
  segmentos de rangos con revisor y veredicto, líneas sin revisar;
- contadores P0–P3 abiertos y lista de hallazgos con ubicación, autor atribuido,
  disposición, corrección y resolución;
- “Pedir revisión” (Human) y, para owner/admin, waiver con confirmación y motivo;
- aviso explícito en `history_rewritten`, `blocked` y head remoto distinto;
- selector de clase de riesgo y umbrales (creación de Task y ajustes);
- el panel de revisión del WorkUnit muestra el sujeto de rango y un formulario de
  hallazgos estructurados y resoluciones;
- sin política: el panel no aparece.

## Plan de pruebas

Prefijo `review_range_`. Solo CI valida ejecución.

**db** (`crates/db/src/tests/review_ranges.rs`): migración desde V183 preserva
filas de Gate y coordinación; único checkpoint activo; inmutabilidad; guards de
mismo Task/repo/contexto; sujeto exclusivo; independencia Human y sesión Agent
con rango; el sujeto de resultado conserva sus rechazos; P0 no waivable;
revisión de cobertura monótona; teardown de Project.

**services:** cobertura (baseline, delta, hueco, solape, base no cubierta,
revert intermedio, merge de upstream, rebase, candidato avanzado, quórum);
cierre y reapertura tras rewrite; disparadores R0/R1/R2/umbral/cola; parser v2
(rechazos); atribución (merge commit, fast-forward, preexistente, borrado,
empate); triage v3 idempotente y corrección sobre HEAD vigente; rondas
agotadas; plan con checkpoints y replan que los quita; Gate v4 fail closed,
obsolescencia, push tras rework; multirepo con un repo sin cubrir; regresión sin
política (misma conducta, planes y Gates actuales).

**api / mcp:** policy 409 y autorización; petición sin SHAs; flujo extremo a
extremo R1 (integrar → baseline → P1 → corrección → delta → `ready_to_merge`);
waiver; SSE sin texto sensible; regresión de revisión de resultado y
`happy_path`.

**web:** estados del panel, hallazgos, formulario Human, ausencia sin política.

Filtros a añadir en `.github/workflows/build-local-linux.yml`
(`if: ${{ !cancelled() }}`):

```text
cargo test --locked -p db --lib review_range_
cargo test --locked -p db --test task_coordination_schema review_range_
cargo test --locked -p git --lib review_range_
cargo test --locked -p services --lib review_range_
cargo test --locked -p services --test task_coordination review_range_
cargo test --locked -p api --test work_unit_reviews review_range_
cargo test --locked -p api --test gate_engine_api review_range_
cargo test --locked -p api --test coordinator_verification_pr_loop review_range_
cargo test --locked -p api --test happy_path review_range_
cargo test --locked -p mcp-server --lib review_range_
```

Más los pasos actuales de clippy, fmt, `pnpm lint|typecheck|test` y e2e.

## Fases y lanes

- **C0** — resolver DR1–DR14; congelar SQL de V184/V185, DTOs y firmas.
- **C1** — lane A y `api-types` (lane F, primer paso).
- **C2** — lanes B, C, E en paralelo; D cuando B publique cobertura y atribución.
- **C3** — resto de F, CI y E2E real con un packet multirepo.

Hasta cerrar C3 no se expone la escritura de políticas: una Task con cobertura
sin triage o Gate completos quedaría bloqueada.

Cada ruta pertenece a una sola lane; si un test vive en el mismo fichero, es de
su dueño. Los módulos nuevos se registran por su lane dueña.

| Lane | Ficheros exclusivos | Alcance |
|---|---|---|
| A — Persistencia | `crates/db/**` (V184, V185, `models.rs`, `repository.rs`, `sqlite*`, tests) | esquema, guards, writers transaccionales |
| B — Núcleo de rangos | `crates/git/**`; `crates/services/src/review_range/**` (nuevo: policy, coverage, scheduler, attribution, resolution); `services/src/lib.rs` | cobertura, disparadores, atribución |
| C — Ejecución y report | `collaboration_service.rs`; `task_service/{review,governance,workspace}.rs`; `task_service/execution/**`; `work_unit_service.rs` y `work_unit_service/**`; `task_dispatcher/**` | workspace revisor, prompt, independencia, ingestión v2 |
| D — Coordinación y rework | `task_coordination.rs` y `task_coordination/**`; `task_rework.rs` y `task_rework/**`; `orchestrator_runtime.rs` y sus tests; `coordinator_outputs/**`; `replan_classifier.rs`; `services/tests/task_coordination.rs` | plan, prompt, salidas, triage, correcciones |
| E — Gate y merge | `gate_engine.rs`; `merge_service.rs`; `task_lifecycle.rs`; `pr_service.rs`; `pr_rework_watcher.rs`; `task_integration_operation.rs`; `validation_check_runner.rs` | Gate v4, fail closed, comprobación pre-push |
| F — Superficies | `crates/api-types/**`; `crates/api/**`; `crates/mcp-server/**`; `web/**`; `docs/**` salvo este plan; `CHANGELOG.md`; `.github/workflows/build-local-linux.yml` | contratos, UI, docs, CI |

`micro_fix.rs` crea Gates: la lane D debe usar el constructor de políticas de la
lane E que preserva `review_coverage`.

Firmas a congelar en C0 (orientativas):

```rust
ReviewPolicyService::current(task_id) -> Option<ReviewPolicyRevision>;
ReviewCoverageService::compute(task_id, repo_id, candidate) -> RepoCoverage;
ReviewCoverageService::snapshot_in_tx(tx, task_id, &[RepoCandidate]) -> CoverageSnapshot;
ReviewScheduler::process_domain_event(&DomainEvent) -> usize;
ReviewScheduler::request(source, task_id, repo_id, reason, idempotency_key) -> ReviewCheckpoint;
ReviewAttribution::attribute(range, &[FindingLocation]) -> Vec<Attribution>;
ReviewRangeReportRepo::ingest(report, findings, resolutions, expected_checkpoint_version);
merge_policy_preserving_coverage(current, next) -> GatePolicyDocumentV4;
```

Documentación a actualizar en la entrega: `architecture.md`,
`migration/architecture-v2.md` (si C0 decide anotar INV), `concepts/`
(`review-and-validation`, `gates`, `work-units`, `orchestration`,
`pr-feedback-loop`, `task-operating-model`), `api.md`.

## Decisiones del Human (2026-11-25)

Resueltas; sustituyen a las recomendaciones de la tabla siguiente donde difieran.

| ID | Decisión |
|---|---|
| DR1 | Tabla de política por Task; Gate y plan la referencian |
| DR2 | Opt-in por Project |
| DR3 | En R0 la revisión del Human en el merge es la revisión que cubre; no se exige revisión de Agent |
| DR4 | 800 líneas cambiadas o 25 ficheros, sin exclusiones |
| DR5 | Un revisor; independencia = sesión nueva sin el contexto del productor. Otra familia de modelo es opcional. En R2 con proveedores externos se exige contraste con respuestas reales grabadas (evidencia, no regla de identidad) |
| DR6 | Los checkpoints R2 no bloquean trabajo dependiente; la cobertura bloquea el merge |
| DR7 | Commits no registrados bloquean la cobertura hasta que un Human los atribuya |
| DR8 | P1 con waiver de Human y referencia; P0 nunca; recheck por cualquier revisor independiente |
| DR9 | Revisiones de resultado y pre-integración no cubren en v1 |
| DR10 | Sin micro-fix sobre rangos en v1 |
| DR11 | Rechazar revisiones de resultado de WorkUnit de repositorio en planes con cobertura |
| DR12 | Sin comando forge-ctl en v1 |
| DR13 | Re-baseline tras rebase completo: decisión owner/admin |
| DR14 | Entrega en 0.9.3, rama propia tras la release 0.9.2 (multirepo); el piloto FP-2026-003 se ejecuta sobre 0.9.3 |

## Decisiones pendientes del Human (propuesta original)

| ID | Decisión | Recomendación |
|---|---|---|
| DR1 | Dónde vive la política | tabla por Task; Gate y plan referencian |
| DR2 | Defecto para Tasks nuevas | opt-in por Project |
| DR3 | R0 sin cobertura obligatoria | sí |
| DR4 | Umbral de tamaño | 800 líneas / 25 ficheros, sin exclusiones |
| DR5 | Quórum e independencia en R2 | 1 revisor, regla de sesión actual |
| DR6 | Checkpoints R2 bloqueantes | no |
| DR7 | Commits no registrados | bloquear; atribuir por decisión Human |
| DR8 | Waivers y recheck | P1 sí, P0 no; cualquier revisor independiente |
| DR9 | Revisiones de resultado y pre-integración | no cubren; fuera de v1 |
| DR10 | Micro-fix sobre rangos | no en v1 |
| DR11 | Revisión de resultado en planes con cobertura | rechazar en trabajo de repo |
| DR12 | Comando forge-ctl | no en v1 |
| DR13 | Re-baseline tras rebase completo | revisión de base con decisión owner/admin |
| DR14 | Versión de entrega (el producto sigue siendo 0.9.x) | posterior a multirepo, sin mezclar en su PR |

## Riesgos

- **Revisiones grandes.** Un baseline R1 multi-WorkUnit puede exceder el contexto
  del revisor; el umbral lo mitiga, pero un solo commit enorme no se divide.
- **Coste de bucle.** Cada corrección genera un delta; se acota con rondas de
  rework y `max_checkpoints_per_repo`.
- **Interacciones entre deltas.** Un delta no relee lo ya revisado; es el
  compromiso de la metodología. Un Human puede pedir re-baseline completo.
- **Triggers SQLite.** V185 toca independencia y coordinación; es el punto de
  mayor riesgo de regresión sobre revisiones actuales.
- **Atribución.** `blame` sigue mal movimientos de código; solo afecta al
  enrutado, nunca a la cobertura.
