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

## Contratos congelados (C0)

Congelado el 2026-11-26 sobre `feat/review-ranges` (`bc784b74`). Último fichero
de `crates/db/migrations`: `V183__multirepo_pr_batches.sql`; V176 no existe.
Siguientes: **V184** y **V185**. Donde esta sección difiere de las secciones
anteriores, manda esta, subordinada a «Decisiones del Human». El SQL se ha
contrastado leyendo las definiciones vigentes; **no se ha ejecutado**. La lane A
lo valida con sus tests; si un statement no compila, corrige la sintaxis sin
cambiar semántica y actualiza esta sección en el mismo cambio. Ningún otro
contrato cambia sin acuerdo del coordinador de lanes.

### C0.1 Resoluciones de ambigüedades

| # | Resolución |
|---|---|
| R1 | `required_reviewers` solo admite `1` en v1 (DR5). El esquema admite 1–3 para no reconstruir tablas después. «Sesión nueva sin contexto del productor» se impone en SQL y Rust. La familia de modelo no se comprueba. |
| R2 | `coverage_required` es derivado y se valida: `r0 ⇒ false`; `r1`, `r2 ⇒ true` (DR3). En R0 la revisión que cubre es el merge Human; no se crea fila de rango sintética. |
| R3 | Contraste con respuestas reales grabadas (DR5, R2 con proveedores externos): es evidencia de la Task, no identidad ni input nuevo del Gate. La lane C lo incluye en el prompt revisor R2 como criterio exigido; su ausencia es un hallazgo del revisor. |
| R4 | Un checkpoint `blocked` **es** la decisión Human pendiente. No se escribe `human_decision_request` (su `source_kind` y `resume_binding_json` no admiten checkpoints). La salida es una ruta Human dedicada (atribución, re-baseline, retry, política, waiver); las que cambian autoridad crean Proposal + Decision exactas. La disposición `human` del coordinador sí usa `human_decision_request` con `source_kind='coordination_request'`. |
| R5 | La independencia por asignación de rangos usa un trigger nuevo; `work_unit_review_independent_allocation` (V162) no se toca porque solo se activa con `work_unit_review_subject`. Igual para `work_unit_execution_admission_guard` (V183): sus ramas no alcanzan un WorkUnit de rango; se añade un guard aparte. |
| R6 | `task_review_coordination_request_current` (V168) no cambia: hereda la rama de rango a través de la vista `source_current`, que sí se recrea. |
| R7 | La revisión de cobertura la incrementan triggers SQLite (V184), no cada writer. |
| R8 | El input `review_coverage` se emite para **todo repo escribible con al menos una WorkUnitRepositoryIntegration `success`**; un árbol igual a la base lleva estado `unchanged`. |
| R9 | Veredicto `needs_human` exige `findings` y `resolutions` vacíos. |
| R10 | `not_reproduced` solo se admite en un checkpoint con razón `recheck`; `fixed` exige head descendiente **estricto** del ancla. Waiver: owner/admin, P1–P3, con `reference` no vacía (DR8). P0 nunca. |
| R11 | El umbral dispara con `changed_lines >= N` (añadidas + borradas) o `changed_files >= M`. Binario: 1 fichero, 0 líneas. Sin exclusiones (DR4). |
| R12 | El WorkUnit revisor tiene `requires_integration=false`, `provenance=NULL`, `created_by` = Human autor de la revisión de política vigente, un único `work_unit_repository` y `Execution.workspace_id = NULL` (siempre WorkspaceGroup). |
| R13 | La política no se borra; para relajar se escribe otra revisión (p. ej. R0). |
| R14 | Digests: SHA-256 hex minúsculas de `serde_json::to_vec` del struct canónico con los campos en el orden declarado aquí; colecciones ordenadas como se indique. |

### C0.2 Definiciones vigentes revalidadas

| Objeto | Origen efectivo | Acción |
|---|---|---|
| trigger `work_unit_review_subject_same_task_insert` | V162 | sin cambio; V184 añade exclusividad |
| trigger `work_unit_review_subject_reviewer_role_immutable` | V162 | sin cambio; análogo de rango en V185 |
| trigger `work_unit_review_independent_allocation` | V162 | sin cambio (R5) |
| trigger `work_unit_review_independent_execution` | V162 | **reemplazar** en V185 |
| triggers `execution_role_session_isolation_insert`/`_update`, `harness_session_role_ancestry_immutable` | V162 | sin cambio |
| trigger `same_agent_review_requires_isolated_session` | V162 | sin cambio; análogos de rango en V185 |
| trigger `work_unit_execution_admission_guard` | V183 | sin cambio; guard adicional en V185 |
| trigger `work_unit_completion_readiness_guard` | V183 | sin cambio (revisor sin integración ni dependencias) |
| trigger `workspace_scope_insert_guard` | V183 | **reemplazar** en V185 (hoy rechaza el scope del revisor de rango) |
| trigger `work_unit_repository_insert_guard`, `work_unit_repository_set_revision_insert_guard` | V183 | sin cambio; el writer de activación los respeta |
| triggers `completed_review_execution_requires_report`, `execution_artifact_output_review_guard_insert`, `review_report_artifact_required_insert`, `review_execution_subject_*` | V152 | sin cambio; el sujeto de rango usa `workspace_id`/SHAs/`workspace_snapshot_digest` nulos |
| trigger `review_report_subject_commit_immutable` | V099 | sin cambio |
| triggers `work_unit_review_line_*` | V155 / V157 | sin cambio; un report de rango no tiene sujeto y no toca líneas |
| trigger `task_rework_automation_intent_insert_guard` | V153 | sin cambio; Rust no crea intents para reports de rango |
| vista `task_review_coordination_source_current` | V168 | **recrear** en V185 con `UNION ALL` |
| vista `task_review_coordination_request_current` | V168 | sin cambio (R6) |
| trigger `task_review_triage_current_request_guard` | V168 | recrear idéntico tras reconstruir la tabla |
| tabla `task_review_triage` y triggers `task_review_triage_immutable`, `..._immutable_delete` | V164 | **reconstruir** para admitir `defer` |
| trigger `task_coordination_request_triaged_guard` | V173 | sin cambio; el rango escribe la fila agregada |
| triggers `task_coordination_request_review_source_guard`, `project_plan_batch_coordination_request_scope_guard` | V172 | sin cambio; leen la vista |
| trigger `micro_fix_receipt_insert_guard` | V180 | sin cambio; guard adicional DR10 |
| trigger `proposal_actor_target_guard_insert` | V177 | **reemplazar** (nuevos target kinds) |
| tabla `gate_evaluation_input` | V155 | **reconstruir** |
| triggers `gate_evaluation_input_same_task_insert`, `..._immutable_update`, `..._immutable_delete`, `..._review_line_guard`; índice `idx_gate_evaluation_input_ref` | V155 | recrear; solo cambia `same_task_insert` |
| vista `gate_evaluation_stale_input` | V169 | **recrear** con rama `review_coverage` |
| vista `gate_evaluation_currentness`, triggers `pr9_gate_current_inputs_lifecycle_guard`, `pr9_task_merge_current_inputs_guard` | V142 | sin cambio |
| triggers `task_merge_gate_admission_guard`, `task_lifecycle_transition_guard_insert`, `task_publish_pr_gate_admission_guard` | V183 | sin cambio; guards fail-closed adicionales |
| triggers `pr_external_action_insert_guard`, `pr_external_action_scope_guard` (y recreaciones V175) | V173 / V175 | sin cambio; guard adicional `push_update` |
| `gate_policy_revision.schema_version CHECK (>= 1)` | V100 | no restringe v4; sin rebuild |

### C0.3 `V184__review_ranges.sql`

Migración transaccional normal (sin sentencias PRAGMA, tampoco en comentarios:
`migration_requires_direct_connection` busca el texto).

```sql
CREATE TABLE project_review_policy (
    project_id          TEXT PRIMARY KEY REFERENCES project(id) ON DELETE CASCADE,
    version             INTEGER NOT NULL DEFAULT 1 CHECK (version >= 1),
    policy_json         TEXT CHECK (policy_json IS NULL OR (json_valid(policy_json)
                            AND json_type(policy_json) = 'object' AND length(policy_json) <= 4096)),
    digest              TEXT CHECK (digest IS NULL OR length(digest) = 64),
    updated_by_user_id  TEXT NOT NULL REFERENCES user(id) ON DELETE RESTRICT,
    created_at          TEXT NOT NULL,
    updated_at          TEXT NOT NULL,
    CHECK ((policy_json IS NULL) = (digest IS NULL))
);
CREATE TRIGGER project_review_policy_update_guard
BEFORE UPDATE ON project_review_policy
WHEN NEW.project_id IS NOT OLD.project_id OR NEW.created_at IS NOT OLD.created_at
  OR NEW.version != OLD.version + 1
BEGIN SELECT RAISE(ABORT, 'Project review policy advances exactly one version'); END;

CREATE TABLE task_review_policy_revision (
    task_id             TEXT NOT NULL REFERENCES task(id) ON DELETE RESTRICT,
    revision            INTEGER NOT NULL CHECK (revision >= 1),
    state               TEXT NOT NULL CHECK (state IN ('current', 'superseded')),
    policy_json         TEXT NOT NULL CHECK (json_valid(policy_json)
                            AND json_type(policy_json) = 'object' AND length(policy_json) <= 4096
                            AND json_extract(policy_json, '$.schema_version') = 1),
    digest              TEXT NOT NULL CHECK (length(digest) = 64),
    risk_class          TEXT NOT NULL CHECK (risk_class IN ('r0', 'r1', 'r2')),
    coverage_required   INTEGER NOT NULL CHECK (coverage_required IN (0, 1)),
    source              TEXT NOT NULL CHECK (source IN ('project_default', 'owner')),
    created_by_user_id  TEXT NOT NULL REFERENCES user(id) ON DELETE RESTRICT,
    created_at          TEXT NOT NULL,
    superseded_at       TEXT,
    PRIMARY KEY (task_id, revision),
    CHECK ((state = 'superseded') = (superseded_at IS NOT NULL)),
    CHECK ((risk_class = 'r0') = (coverage_required = 0))
);
CREATE UNIQUE INDEX idx_task_review_policy_current
    ON task_review_policy_revision(task_id) WHERE state = 'current';
CREATE TRIGGER task_review_policy_revision_insert_guard
BEFORE INSERT ON task_review_policy_revision
WHEN NEW.state != 'current'
  OR NEW.revision != COALESCE((SELECT MAX(revision) FROM task_review_policy_revision
                               WHERE task_id = NEW.task_id), 0) + 1
  OR json_extract(NEW.policy_json, '$.risk_class') IS NOT NEW.risk_class
  OR json_extract(NEW.policy_json, '$.coverage_required') IS NOT NEW.coverage_required
BEGIN SELECT RAISE(ABORT, 'Task review policy revision must be the next exact current revision'); END;
CREATE TRIGGER task_review_policy_revision_update_guard
BEFORE UPDATE ON task_review_policy_revision
WHEN OLD.state != 'current' OR NEW.state != 'superseded'
  OR NEW.task_id IS NOT OLD.task_id OR NEW.revision IS NOT OLD.revision
  OR NEW.policy_json IS NOT OLD.policy_json OR NEW.digest IS NOT OLD.digest
  OR NEW.risk_class IS NOT OLD.risk_class OR NEW.coverage_required IS NOT OLD.coverage_required
  OR NEW.source IS NOT OLD.source OR NEW.created_by_user_id IS NOT OLD.created_by_user_id
  OR NEW.created_at IS NOT OLD.created_at
BEGIN SELECT RAISE(ABORT, 'Task review policy revisions are immutable except supersession'); END;

CREATE TABLE task_review_base (
    task_id      TEXT NOT NULL REFERENCES task(id) ON DELETE RESTRICT,
    repo_id      TEXT NOT NULL REFERENCES repo(id) ON DELETE RESTRICT,
    revision     INTEGER NOT NULL CHECK (revision >= 1),
    base_commit  TEXT NOT NULL CHECK (length(trim(base_commit)) > 0),
    base_tree    TEXT NOT NULL CHECK (length(trim(base_tree)) > 0),
    source       TEXT NOT NULL CHECK (source IN
                     ('first_integration', 'workspace_prepare', 'human_rebaseline')),
    decision_id  TEXT REFERENCES decision(id) ON DELETE RESTRICT,
    pinned_at    TEXT NOT NULL,
    PRIMARY KEY (task_id, repo_id, revision),
    CHECK ((revision > 1) = (source = 'human_rebaseline')),
    CHECK ((source = 'human_rebaseline') = (decision_id IS NOT NULL))
);
CREATE TRIGGER task_review_base_insert_guard
BEFORE INSERT ON task_review_base
WHEN NEW.revision != COALESCE((SELECT MAX(revision) FROM task_review_base
        WHERE task_id = NEW.task_id AND repo_id = NEW.repo_id), 0) + 1
  OR NOT EXISTS (
        SELECT 1 FROM task_repository_set_revision v
        JOIN task_repository_set_revision_repo m
          ON m.task_id = v.task_id AND m.revision = v.revision
        WHERE v.task_id = NEW.task_id AND v.state = 'current'
          AND m.repo_id = NEW.repo_id AND m.role IN ('primary', 'affected'))
  OR (NEW.decision_id IS NOT NULL AND NOT EXISTS (
        SELECT 1 FROM decision d
        JOIN proposal p ON p.id = d.proposal_id AND p.task_id = d.task_id
        JOIN decision_actor a ON a.decision_id = d.id AND a.actor_kind = 'human'
        JOIN task t ON t.id = d.task_id
        JOIN project_member pm ON pm.project_id = t.project_id AND pm.user_id = a.actor_id
          AND pm.role IN ('owner', 'admin')
        WHERE d.id = NEW.decision_id AND d.task_id = NEW.task_id AND d.outcome = 'approve'
          AND p.target_kind = 'review_base' AND p.target_id = NEW.repo_id))
BEGIN SELECT RAISE(ABORT, 'Review base must be sequential, writable and Human-authorized after revision 1'); END;

CREATE TABLE review_commit_attribution (
    id           TEXT PRIMARY KEY,
    task_id      TEXT NOT NULL REFERENCES task(id) ON DELETE RESTRICT,
    repo_id      TEXT NOT NULL REFERENCES repo(id) ON DELETE RESTRICT,
    commit_sha   TEXT NOT NULL CHECK (length(trim(commit_sha)) > 0),
    actor_kind   TEXT NOT NULL CHECK (actor_kind IN ('human', 'agent')),
    actor_id     TEXT NOT NULL CHECK (length(trim(actor_id)) > 0),
    decision_id  TEXT NOT NULL REFERENCES decision(id) ON DELETE RESTRICT,
    created_at   TEXT NOT NULL,
    UNIQUE (task_id, repo_id, commit_sha)
);
CREATE TRIGGER review_commit_attribution_insert_guard
BEFORE INSERT ON review_commit_attribution
WHEN (NEW.actor_kind = 'human' AND NOT EXISTS (SELECT 1 FROM user u WHERE u.id = NEW.actor_id))
  OR (NEW.actor_kind = 'agent' AND NOT EXISTS (SELECT 1 FROM agent_identity a WHERE a.id = NEW.actor_id))
  OR NOT EXISTS (
        SELECT 1 FROM decision d
        JOIN proposal p ON p.id = d.proposal_id AND p.task_id = d.task_id
        JOIN decision_actor a ON a.decision_id = d.id AND a.actor_kind = 'human'
        JOIN task t ON t.id = d.task_id
        JOIN project_member pm ON pm.project_id = t.project_id AND pm.user_id = a.actor_id
          AND pm.role IN ('owner', 'admin')
        WHERE d.id = NEW.decision_id AND d.task_id = NEW.task_id AND d.outcome = 'approve'
          AND p.target_kind = 'review_commit' AND p.target_id = NEW.repo_id)
BEGIN SELECT RAISE(ABORT, 'Commit attribution requires an exact owner/admin Decision'); END;

CREATE TABLE review_range (
    id                       TEXT PRIMARY KEY,
    task_id                  TEXT NOT NULL REFERENCES task(id) ON DELETE RESTRICT,
    repo_id                  TEXT NOT NULL REFERENCES repo(id) ON DELETE RESTRICT,
    repository_set_revision  INTEGER NOT NULL,
    base_commit              TEXT NOT NULL CHECK (length(trim(base_commit)) > 0),
    head_commit              TEXT NOT NULL CHECK (length(trim(head_commit)) > 0),
    base_tree                TEXT NOT NULL CHECK (length(trim(base_tree)) > 0),
    head_tree                TEXT NOT NULL CHECK (length(trim(head_tree)) > 0),
    range_digest             TEXT NOT NULL CHECK (length(range_digest) = 64),
    integration_workspace_id TEXT NOT NULL REFERENCES workspace(id) ON DELETE RESTRICT,
    head_integration_id      TEXT NOT NULL
                                 REFERENCES work_unit_repository_integration(id) ON DELETE RESTRICT,
    created_at               TEXT NOT NULL,
    UNIQUE (task_id, repo_id, base_commit, head_commit),
    UNIQUE (id, task_id, repo_id),
    CHECK (base_commit != head_commit),
    FOREIGN KEY (task_id, repository_set_revision, repo_id)
        REFERENCES task_repository_set_revision_repo(task_id, revision, repo_id) ON DELETE RESTRICT
);
CREATE TRIGGER review_range_insert_guard
BEFORE INSERT ON review_range
WHEN NOT EXISTS (SELECT 1 FROM task_review_policy_revision p
                 WHERE p.task_id = NEW.task_id AND p.state = 'current')
  OR NOT EXISTS (
        SELECT 1 FROM task_repository_set_revision v
        JOIN task_repository_set_revision_repo m
          ON m.task_id = v.task_id AND m.revision = v.revision
        WHERE v.task_id = NEW.task_id AND v.state = 'current'
          AND v.revision = NEW.repository_set_revision
          AND m.repo_id = NEW.repo_id AND m.role IN ('primary', 'affected'))
  OR NOT EXISTS (SELECT 1 FROM workspace_scope s
        WHERE s.workspace_id = NEW.integration_workspace_id AND s.task_id = NEW.task_id
          AND s.repo_id = NEW.repo_id AND s.scope_kind = 'integration')
  OR NOT EXISTS (SELECT 1 FROM work_unit_repository_integration i
        WHERE i.id = NEW.head_integration_id AND i.task_id = NEW.task_id
          AND i.repo_id = NEW.repo_id AND i.outcome = 'success'
          AND i.target_after_sha = NEW.head_commit
          AND i.target_workspace_id = NEW.integration_workspace_id)
BEGIN SELECT RAISE(ABORT, 'Review range must end at an exact successful integration of a writable repository'); END;

CREATE TABLE review_checkpoint (
    id                   TEXT PRIMARY KEY,
    task_id              TEXT NOT NULL REFERENCES task(id) ON DELETE RESTRICT,
    repo_id              TEXT NOT NULL REFERENCES repo(id) ON DELETE RESTRICT,
    state                TEXT NOT NULL CHECK (state IN
                             ('queued', 'pending', 'in_review', 'reviewed', 'blocked', 'superseded')),
    reasons_json         TEXT NOT NULL CHECK (json_valid(reasons_json)
                             AND json_type(reasons_json) = 'array'
                             AND json_array_length(reasons_json) BETWEEN 1 AND 7),
    attempt              INTEGER NOT NULL CHECK (attempt >= 1),
    base_commit          TEXT,
    head_commit          TEXT NOT NULL CHECK (length(trim(head_commit)) > 0),
    range_id             TEXT,
    policy_revision      INTEGER NOT NULL,
    blocked_code         TEXT,
    blocked_detail_json  TEXT CHECK (blocked_detail_json IS NULL OR (json_valid(blocked_detail_json)
                             AND json_type(blocked_detail_json) = 'object'
                             AND length(blocked_detail_json) <= 16384)),
    version              INTEGER NOT NULL DEFAULT 1 CHECK (version >= 1),
    created_at           TEXT NOT NULL,
    updated_at           TEXT NOT NULL,
    UNIQUE (id, task_id, repo_id),
    FOREIGN KEY (range_id, task_id, repo_id)
        REFERENCES review_range(id, task_id, repo_id) ON DELETE RESTRICT,
    FOREIGN KEY (task_id, policy_revision)
        REFERENCES task_review_policy_revision(task_id, revision) ON DELETE RESTRICT,
    CHECK (state != 'queued' OR (range_id IS NULL AND base_commit IS NULL)),
    CHECK (state NOT IN ('pending', 'in_review', 'reviewed')
           OR (range_id IS NOT NULL AND base_commit IS NOT NULL)),
    CHECK ((state = 'blocked') = (blocked_code IS NOT NULL))
);
CREATE UNIQUE INDEX idx_review_checkpoint_active
    ON review_checkpoint(task_id, repo_id) WHERE state IN ('pending', 'in_review');
CREATE UNIQUE INDEX idx_review_checkpoint_exact
    ON review_checkpoint(task_id, repo_id, base_commit, head_commit, attempt)
    WHERE base_commit IS NOT NULL;
CREATE UNIQUE INDEX idx_review_checkpoint_queued
    ON review_checkpoint(task_id, repo_id, head_commit) WHERE state = 'queued';
CREATE INDEX idx_review_checkpoint_task
    ON review_checkpoint(task_id, created_at DESC, id DESC);
CREATE TRIGGER review_checkpoint_insert_guard
BEFORE INSERT ON review_checkpoint
WHEN NEW.version != 1 OR NEW.state NOT IN ('queued', 'blocked')
  OR NOT EXISTS (SELECT 1 FROM task_review_policy_revision p
        WHERE p.task_id = NEW.task_id AND p.revision = NEW.policy_revision AND p.state = 'current')
  OR (NEW.range_id IS NOT NULL AND NOT EXISTS (SELECT 1 FROM review_range r
        WHERE r.id = NEW.range_id AND r.base_commit = NEW.base_commit
          AND r.head_commit = NEW.head_commit))
BEGIN SELECT RAISE(ABORT, 'Review checkpoint must start queued or blocked under the current policy'); END;
CREATE TRIGGER review_checkpoint_update_guard
BEFORE UPDATE ON review_checkpoint
WHEN NEW.version != OLD.version + 1
  OR NEW.id IS NOT OLD.id OR NEW.task_id IS NOT OLD.task_id OR NEW.repo_id IS NOT OLD.repo_id
  OR NEW.attempt IS NOT OLD.attempt OR NEW.head_commit IS NOT OLD.head_commit
  OR NEW.policy_revision IS NOT OLD.policy_revision OR NEW.created_at IS NOT OLD.created_at
  OR (OLD.base_commit IS NOT NULL AND NEW.base_commit IS NOT OLD.base_commit)
  OR (OLD.range_id IS NOT NULL AND NEW.range_id IS NOT OLD.range_id)
  OR (NEW.range_id IS NOT NULL AND NOT EXISTS (SELECT 1 FROM review_range r
        WHERE r.id = NEW.range_id AND r.base_commit = NEW.base_commit
          AND r.head_commit = NEW.head_commit))
  OR NOT (
        (OLD.state = NEW.state AND OLD.state IN ('queued', 'pending', 'in_review'))
     OR (OLD.state = 'queued' AND NEW.state IN ('pending', 'blocked', 'superseded'))
     OR (OLD.state = 'pending' AND NEW.state IN ('in_review', 'reviewed', 'blocked', 'superseded'))
     OR (OLD.state = 'in_review' AND NEW.state IN ('reviewed', 'blocked'))
     OR (OLD.state = 'blocked' AND NEW.state = 'superseded'))
BEGIN SELECT RAISE(ABORT, 'Review checkpoint transition or identity is invalid'); END;

CREATE TABLE review_checkpoint_request (
    task_id          TEXT NOT NULL REFERENCES task(id) ON DELETE RESTRICT,
    idempotency_key  TEXT NOT NULL CHECK (length(trim(idempotency_key)) BETWEEN 1 AND 256),
    checkpoint_id    TEXT NOT NULL REFERENCES review_checkpoint(id) ON DELETE RESTRICT,
    source_kind      TEXT NOT NULL CHECK (source_kind IN ('human', 'coordinator', 'scheduler')),
    source_ref       TEXT NOT NULL CHECK (length(trim(source_ref)) > 0),
    reason           TEXT NOT NULL CHECK (length(CAST(reason AS BLOB)) BETWEEN 1 AND 1024),
    created_at       TEXT NOT NULL,
    PRIMARY KEY (task_id, idempotency_key)
);

CREATE TABLE review_plan_checkpoint (
    task_id                 TEXT NOT NULL REFERENCES task(id) ON DELETE RESTRICT,
    plan_artifact_id        TEXT NOT NULL REFERENCES artifact(id) ON DELETE RESTRICT,
    plan_key                TEXT NOT NULL CHECK (length(trim(plan_key)) BETWEEN 1 AND 64),
    work_unit_ids_json      TEXT NOT NULL CHECK (json_valid(work_unit_ids_json)
                                AND json_type(work_unit_ids_json) = 'array'
                                AND json_array_length(work_unit_ids_json) >= 1),
    repository_ids_json     TEXT NOT NULL CHECK (json_valid(repository_ids_json)
                                AND json_type(repository_ids_json) = 'array'
                                AND json_array_length(repository_ids_json) >= 1),
    reason                  TEXT NOT NULL CHECK (length(CAST(reason AS BLOB)) BETWEEN 1 AND 1024),
    state                   TEXT NOT NULL CHECK (state IN ('active', 'superseded')),
    created_at              TEXT NOT NULL,
    superseded_at           TEXT,
    PRIMARY KEY (task_id, plan_artifact_id, plan_key),
    CHECK ((state = 'superseded') = (superseded_at IS NOT NULL))
);
CREATE TRIGGER review_plan_checkpoint_update_guard
BEFORE UPDATE ON review_plan_checkpoint
WHEN OLD.state != 'active' OR NEW.state != 'superseded'
  OR NEW.task_id IS NOT OLD.task_id OR NEW.plan_artifact_id IS NOT OLD.plan_artifact_id
  OR NEW.plan_key IS NOT OLD.plan_key OR NEW.work_unit_ids_json IS NOT OLD.work_unit_ids_json
  OR NEW.repository_ids_json IS NOT OLD.repository_ids_json
  OR NEW.reason IS NOT OLD.reason OR NEW.created_at IS NOT OLD.created_at
BEGIN SELECT RAISE(ABORT, 'Plan review checkpoints are immutable except supersession'); END;

CREATE TABLE review_range_contributor (
    id              TEXT PRIMARY KEY,
    task_id         TEXT NOT NULL REFERENCES task(id) ON DELETE RESTRICT,
    range_id        TEXT NOT NULL REFERENCES review_range(id) ON DELETE RESTRICT,
    work_unit_id    TEXT REFERENCES work_unit(id) ON DELETE RESTRICT,
    execution_id    TEXT REFERENCES execution(id) ON DELETE RESTRICT,
    attribution_id  TEXT REFERENCES review_commit_attribution(id) ON DELETE RESTRICT,
    actor_kind      TEXT NOT NULL CHECK (actor_kind IN ('human', 'agent')),
    actor_id        TEXT NOT NULL CHECK (length(trim(actor_id)) > 0),
    created_at      TEXT NOT NULL,
    CHECK ((execution_id IS NOT NULL AND work_unit_id IS NOT NULL AND attribution_id IS NULL)
        OR (execution_id IS NULL AND work_unit_id IS NULL AND attribution_id IS NOT NULL))
);
CREATE UNIQUE INDEX idx_review_range_contributor_execution
    ON review_range_contributor(range_id, execution_id) WHERE execution_id IS NOT NULL;
CREATE UNIQUE INDEX idx_review_range_contributor_attribution
    ON review_range_contributor(range_id, attribution_id) WHERE attribution_id IS NOT NULL;
CREATE INDEX idx_review_range_contributor_actor
    ON review_range_contributor(range_id, actor_kind, actor_id);

CREATE TABLE work_unit_review_range (
    review_work_unit_id    TEXT PRIMARY KEY,
    task_id                TEXT NOT NULL,
    checkpoint_id          TEXT NOT NULL,
    range_id               TEXT NOT NULL,
    repo_id                TEXT NOT NULL,
    input_findings_json    TEXT NOT NULL CHECK (json_valid(input_findings_json)
                               AND json_type(input_findings_json) = 'array'),
    input_findings_digest  TEXT NOT NULL CHECK (length(input_findings_digest) = 64),
    created_at             TEXT NOT NULL,
    FOREIGN KEY (review_work_unit_id, task_id)
        REFERENCES work_unit(id, task_id) ON DELETE RESTRICT,
    FOREIGN KEY (checkpoint_id, task_id, repo_id)
        REFERENCES review_checkpoint(id, task_id, repo_id) ON DELETE RESTRICT,
    FOREIGN KEY (range_id, task_id, repo_id)
        REFERENCES review_range(id, task_id, repo_id) ON DELETE RESTRICT
);
CREATE INDEX idx_work_unit_review_range_checkpoint
    ON work_unit_review_range(checkpoint_id, review_work_unit_id);

CREATE TRIGGER review_range_contributor_insert_guard
BEFORE INSERT ON review_range_contributor
WHEN NOT EXISTS (SELECT 1 FROM review_range r WHERE r.id = NEW.range_id AND r.task_id = NEW.task_id)
  OR EXISTS (SELECT 1 FROM work_unit_review_range w WHERE w.range_id = NEW.range_id)
  OR (NEW.execution_id IS NOT NULL AND NOT EXISTS (SELECT 1 FROM execution e
        WHERE e.id = NEW.execution_id AND e.task_id = NEW.task_id
          AND e.work_unit_id = NEW.work_unit_id
          AND e.actor_kind = NEW.actor_kind AND e.actor_id = NEW.actor_id))
  OR (NEW.attribution_id IS NOT NULL AND NOT EXISTS (SELECT 1 FROM review_commit_attribution a
        JOIN review_range r ON r.id = NEW.range_id AND r.repo_id = a.repo_id
        WHERE a.id = NEW.attribution_id AND a.task_id = NEW.task_id
          AND a.actor_kind = NEW.actor_kind AND a.actor_id = NEW.actor_id))
BEGIN SELECT RAISE(ABORT, 'Range contributors are exact, same-Task and frozen before review'); END;

CREATE TRIGGER work_unit_review_range_insert_guard
BEFORE INSERT ON work_unit_review_range
WHEN EXISTS (SELECT 1 FROM work_unit_review_subject s WHERE s.review_work_unit_id = NEW.review_work_unit_id)
  OR EXISTS (SELECT 1 FROM execution e WHERE e.work_unit_id = NEW.review_work_unit_id)
  OR NOT EXISTS (
        SELECT 1 FROM work_unit w
        JOIN task_role role ON role.task_id = w.task_id AND role.role = w.role
        JOIN task_role_capability_configuration configuration ON configuration.task_role_id = role.id
        WHERE w.id = NEW.review_work_unit_id AND w.task_id = NEW.task_id
          AND w.status = 'open' AND w.requires_integration = 0
          AND EXISTS (SELECT 1 FROM json_each(configuration.configuration_json, '$.capabilities')
                      WHERE type = 'text' AND value = 'formal_review'))
  OR NOT EXISTS (SELECT 1 FROM review_checkpoint c
        WHERE c.id = NEW.checkpoint_id AND c.range_id = NEW.range_id AND c.state = 'pending')
  OR NOT EXISTS (SELECT 1 FROM work_unit_repository s
        WHERE s.work_unit_id = NEW.review_work_unit_id AND s.repo_id = NEW.repo_id)
  OR EXISTS (SELECT 1 FROM work_unit_repository s
        WHERE s.work_unit_id = NEW.review_work_unit_id AND s.repo_id != NEW.repo_id)
  OR EXISTS (SELECT 1 FROM work_unit w
        JOIN review_range_contributor c ON c.range_id = NEW.range_id
        WHERE w.id = NEW.review_work_unit_id AND w.assigned_actor_kind = 'human'
          AND c.actor_kind = 'human' AND c.actor_id = w.assigned_actor_id)
BEGIN SELECT RAISE(ABORT, 'Range review requires an exact independent formal-review WorkUnit'); END;

CREATE TRIGGER work_unit_review_subject_excludes_range_insert
BEFORE INSERT ON work_unit_review_subject
WHEN EXISTS (SELECT 1 FROM work_unit_review_range r WHERE r.review_work_unit_id = NEW.review_work_unit_id)
BEGIN SELECT RAISE(ABORT, 'A WorkUnit reviews either a result or a commit range'); END;

CREATE TABLE review_range_report (
    report_artifact_id   TEXT PRIMARY KEY REFERENCES artifact(id) ON DELETE RESTRICT,
    task_id              TEXT NOT NULL REFERENCES task(id) ON DELETE RESTRICT,
    repo_id              TEXT NOT NULL,
    range_id             TEXT NOT NULL,
    checkpoint_id        TEXT NOT NULL,
    review_work_unit_id  TEXT NOT NULL
                             REFERENCES work_unit_review_range(review_work_unit_id) ON DELETE RESTRICT,
    execution_id         TEXT NOT NULL REFERENCES execution(id) ON DELETE RESTRICT,
    actor_kind           TEXT NOT NULL CHECK (actor_kind IN ('human', 'agent')),
    actor_id             TEXT NOT NULL,
    verdict              TEXT NOT NULL CHECK (verdict IN ('pass', 'request_changes', 'questions')),
    report_digest        TEXT NOT NULL CHECK (length(trim(report_digest)) > 0),
    created_at           TEXT NOT NULL,
    UNIQUE (checkpoint_id, execution_id),
    FOREIGN KEY (range_id, task_id, repo_id)
        REFERENCES review_range(id, task_id, repo_id) ON DELETE RESTRICT,
    FOREIGN KEY (checkpoint_id, task_id, repo_id)
        REFERENCES review_checkpoint(id, task_id, repo_id) ON DELETE RESTRICT
);
CREATE INDEX idx_review_range_report_range ON review_range_report(range_id, verdict);
CREATE TRIGGER review_range_report_insert_guard
BEFORE INSERT ON review_range_report
WHEN NOT EXISTS (
    SELECT 1 FROM artifact a
    JOIN artifact_execution_producer p ON p.artifact_id = a.id AND p.task_id = a.task_id
    JOIN execution e ON e.id = p.execution_id AND e.task_id = p.task_id
    JOIN work_unit_review_range w ON w.review_work_unit_id = e.work_unit_id
    JOIN review_checkpoint c ON c.id = w.checkpoint_id
    JOIN review_range r ON r.id = w.range_id
    WHERE a.id = NEW.report_artifact_id AND a.task_id = NEW.task_id AND a.kind = 'review_report'
      AND a.digest = NEW.report_digest AND e.id = NEW.execution_id
      AND e.actor_kind = NEW.actor_kind AND e.actor_id = NEW.actor_id
      AND w.review_work_unit_id = NEW.review_work_unit_id
      AND w.checkpoint_id = NEW.checkpoint_id AND w.range_id = NEW.range_id
      AND w.repo_id = NEW.repo_id AND c.state IN ('pending', 'in_review')
      AND json_extract(a.content, '$.verdict') = NEW.verdict
      AND json_extract(a.content, '$.subject.range.range_digest') = r.range_digest)
BEGIN SELECT RAISE(ABORT, 'Range report must bind its exact active checkpoint, range and producer'); END;

CREATE TRIGGER review_report_range_subject_insert
BEFORE INSERT ON artifact
WHEN NEW.kind = 'review_report' AND (
    EXISTS (
        SELECT 1 FROM execution_artifact_output o
        JOIN execution e ON e.id = o.execution_id AND e.task_id = o.task_id
        JOIN work_unit_review_range w ON w.review_work_unit_id = e.work_unit_id
        JOIN review_range r ON r.id = w.range_id
        WHERE o.artifact_id = NEW.id AND o.task_id = NEW.task_id AND (
              json_extract(NEW.content, '$.subject.kind') IS NOT 'commit_range'
           OR json_extract(NEW.content, '$.subject.checkpoint_id') IS NOT w.checkpoint_id
           OR json_extract(NEW.content, '$.subject.range.id') IS NOT r.id
           OR json_extract(NEW.content, '$.subject.range.range_digest') IS NOT r.range_digest))
    OR (json_extract(NEW.content, '$.subject.kind') = 'commit_range' AND NOT EXISTS (
        SELECT 1 FROM execution_artifact_output o
        JOIN execution e ON e.id = o.execution_id AND e.task_id = o.task_id
        JOIN work_unit_review_range w ON w.review_work_unit_id = e.work_unit_id
        WHERE o.artifact_id = NEW.id AND o.task_id = NEW.task_id)))
BEGIN SELECT RAISE(ABORT, 'ReviewReport commit_range subject must match its frozen range'); END;

CREATE TABLE review_finding (
    id                  TEXT PRIMARY KEY,
    task_id             TEXT NOT NULL REFERENCES task(id) ON DELETE RESTRICT,
    repo_id             TEXT NOT NULL REFERENCES repo(id) ON DELETE RESTRICT,
    range_id            TEXT NOT NULL REFERENCES review_range(id) ON DELETE RESTRICT,
    report_artifact_id  TEXT NOT NULL
                            REFERENCES review_range_report(report_artifact_id) ON DELETE RESTRICT,
    ordinal             INTEGER NOT NULL CHECK (ordinal BETWEEN 0 AND 99),
    label               TEXT NOT NULL CHECK (length(trim(label)) BETWEEN 1 AND 32),
    severity            TEXT NOT NULL CHECK (severity IN ('P0', 'P1', 'P2', 'P3')),
    title               TEXT NOT NULL CHECK (length(CAST(title AS BLOB)) BETWEEN 1 AND 256),
    evidence            TEXT NOT NULL CHECK (length(CAST(evidence AS BLOB)) BETWEEN 1 AND 4096),
    invariant_text      TEXT CHECK (invariant_text IS NULL
                            OR length(CAST(invariant_text AS BLOB)) BETWEEN 1 AND 512),
    anchor_commit       TEXT NOT NULL,
    digest              TEXT NOT NULL CHECK (length(digest) = 64),
    created_at          TEXT NOT NULL,
    UNIQUE (report_artifact_id, ordinal),
    UNIQUE (report_artifact_id, label)
);
CREATE INDEX idx_review_finding_task ON review_finding(task_id, repo_id, severity, id);
CREATE TRIGGER review_finding_insert_guard
BEFORE INSERT ON review_finding
WHEN NOT EXISTS (SELECT 1 FROM review_range_report p
    JOIN review_range r ON r.id = p.range_id
    WHERE p.report_artifact_id = NEW.report_artifact_id AND p.task_id = NEW.task_id
      AND p.repo_id = NEW.repo_id AND p.range_id = NEW.range_id
      AND p.verdict != 'questions' AND r.head_commit = NEW.anchor_commit
      AND NOT (p.verdict = 'pass' AND NEW.severity IN ('P0', 'P1')))
BEGIN SELECT RAISE(ABORT, 'Finding must belong to its exact range report'); END;

CREATE TABLE review_finding_location (
    finding_id               TEXT NOT NULL REFERENCES review_finding(id) ON DELETE RESTRICT,
    task_id                  TEXT NOT NULL REFERENCES task(id) ON DELETE RESTRICT,
    ordinal                  INTEGER NOT NULL CHECK (ordinal BETWEEN 0 AND 15),
    path                     TEXT NOT NULL CHECK (length(CAST(path AS BLOB)) BETWEEN 1 AND 1024),
    line_start               INTEGER CHECK (line_start IS NULL OR line_start >= 1),
    line_end                 INTEGER,
    attribution_kind         TEXT NOT NULL CHECK (attribution_kind IN
                                 ('work_unit', 'preexisting', 'upstream', 'unattributed')),
    attributed_work_unit_id  TEXT REFERENCES work_unit(id) ON DELETE RESTRICT,
    attributed_execution_id  TEXT REFERENCES execution(id) ON DELETE RESTRICT,
    attributed_actor_kind    TEXT CHECK (attributed_actor_kind IS NULL
                                 OR attributed_actor_kind IN ('human', 'agent')),
    attributed_actor_id      TEXT,
    attributed_commit_sha    TEXT,
    attributed_integration_id TEXT
                                 REFERENCES work_unit_repository_integration(id) ON DELETE RESTRICT,
    attributed_line_count    INTEGER NOT NULL DEFAULT 0 CHECK (attributed_line_count >= 0),
    PRIMARY KEY (finding_id, ordinal),
    CHECK ((line_start IS NULL) = (line_end IS NULL)),
    CHECK (line_end IS NULL OR line_end >= line_start),
    CHECK ((attributed_actor_kind IS NULL) = (attributed_actor_id IS NULL)),
    CHECK ((attribution_kind = 'work_unit') = (attributed_work_unit_id IS NOT NULL))
);

CREATE TABLE review_finding_resolution (
    id                  TEXT PRIMARY KEY,
    finding_id          TEXT NOT NULL REFERENCES review_finding(id) ON DELETE RESTRICT,
    task_id             TEXT NOT NULL REFERENCES task(id) ON DELETE RESTRICT,
    repo_id             TEXT NOT NULL REFERENCES repo(id) ON DELETE RESTRICT,
    outcome             TEXT NOT NULL CHECK (outcome IN ('fixed', 'not_reproduced', 'waived')),
    anchor_commit       TEXT NOT NULL CHECK (length(trim(anchor_commit)) > 0),
    report_artifact_id  TEXT REFERENCES review_range_report(report_artifact_id) ON DELETE RESTRICT,
    decision_id         TEXT REFERENCES decision(id) ON DELETE RESTRICT,
    waiver_reference    TEXT CHECK (waiver_reference IS NULL
                            OR length(CAST(waiver_reference AS BLOB)) BETWEEN 1 AND 512),
    evidence            TEXT NOT NULL CHECK (length(CAST(evidence AS BLOB)) BETWEEN 1 AND 4096),
    actor_kind          TEXT NOT NULL CHECK (actor_kind IN ('human', 'agent')),
    actor_id            TEXT NOT NULL,
    created_at          TEXT NOT NULL,
    UNIQUE (finding_id, outcome, anchor_commit),
    CHECK ((outcome = 'waived' AND decision_id IS NOT NULL AND waiver_reference IS NOT NULL
              AND report_artifact_id IS NULL AND actor_kind = 'human')
        OR (outcome != 'waived' AND decision_id IS NULL AND waiver_reference IS NULL
              AND report_artifact_id IS NOT NULL))
);
CREATE INDEX idx_review_finding_resolution_task
    ON review_finding_resolution(task_id, repo_id, finding_id);
CREATE TRIGGER review_finding_resolution_insert_guard
BEFORE INSERT ON review_finding_resolution
WHEN NOT EXISTS (SELECT 1 FROM review_finding f
        WHERE f.id = NEW.finding_id AND f.task_id = NEW.task_id AND f.repo_id = NEW.repo_id)
  OR (NEW.outcome != 'waived' AND NOT EXISTS (
        SELECT 1 FROM review_range_report p
        JOIN review_range r ON r.id = p.range_id
        JOIN review_checkpoint c ON c.id = p.checkpoint_id
        JOIN work_unit_review_range w ON w.review_work_unit_id = p.review_work_unit_id
        WHERE p.report_artifact_id = NEW.report_artifact_id AND p.task_id = NEW.task_id
          AND p.repo_id = NEW.repo_id AND p.verdict != 'questions'
          AND r.head_commit = NEW.anchor_commit
          AND p.actor_kind = NEW.actor_kind AND p.actor_id = NEW.actor_id
          AND EXISTS (SELECT 1 FROM json_each(w.input_findings_json) i
                      WHERE json_extract(i.value, '$.finding_id') = NEW.finding_id)
          AND (NEW.outcome != 'not_reproduced' OR EXISTS (
                SELECT 1 FROM json_each(c.reasons_json) reason WHERE reason.value = 'recheck'))))
  OR (NEW.outcome = 'waived' AND NOT EXISTS (
        SELECT 1 FROM review_finding f
        JOIN decision d ON d.id = NEW.decision_id AND d.task_id = f.task_id AND d.outcome = 'approve'
        JOIN proposal p ON p.id = d.proposal_id AND p.task_id = d.task_id
        JOIN decision_actor a ON a.decision_id = d.id AND a.actor_kind = 'human'
          AND a.actor_id = NEW.actor_id
        JOIN task t ON t.id = f.task_id
        JOIN project_member pm ON pm.project_id = t.project_id AND pm.user_id = a.actor_id
          AND pm.role IN ('owner', 'admin')
        WHERE f.id = NEW.finding_id AND f.severity != 'P0'
          AND p.target_kind = 'review_finding' AND p.target_id = f.id
          AND p.target_digest = f.digest))
BEGIN SELECT RAISE(ABORT, 'FORGE_REVIEW_FINDING_RESOLUTION_NOT_AUTHORIZED'); END;

CREATE TABLE review_finding_triage (
    finding_id             TEXT NOT NULL REFERENCES review_finding(id) ON DELETE RESTRICT,
    request_id             TEXT NOT NULL
                               REFERENCES task_coordination_request(id) ON DELETE RESTRICT,
    task_id                TEXT NOT NULL REFERENCES task(id) ON DELETE RESTRICT,
    report_artifact_id     TEXT NOT NULL
                               REFERENCES review_range_report(report_artifact_id) ON DELETE RESTRICT,
    execution_id           TEXT NOT NULL REFERENCES execution(id) ON DELETE RESTRICT,
    authorization_version  INTEGER NOT NULL CHECK (authorization_version >= 1),
    disposition            TEXT NOT NULL CHECK (disposition IN ('correct', 'recheck', 'human', 'defer')),
    rationale              TEXT NOT NULL CHECK (length(CAST(rationale AS BLOB)) BETWEEN 1 AND 2048),
    instructions           TEXT NOT NULL CHECK (length(CAST(instructions AS BLOB)) <= 8192),
    created_at             TEXT NOT NULL,
    PRIMARY KEY (finding_id, request_id)
);
CREATE TRIGGER review_finding_triage_insert_guard
BEFORE INSERT ON review_finding_triage
WHEN NOT EXISTS (
        SELECT 1 FROM task_coordination_request q
        WHERE q.id = NEW.request_id AND q.task_id = NEW.task_id AND q.kind = 'review_findings'
          AND q.review_report_artifact_id = NEW.report_artifact_id
          AND q.execution_id = NEW.execution_id
          AND q.authorization_version = NEW.authorization_version)
  OR NOT EXISTS (SELECT 1 FROM review_finding f
        WHERE f.id = NEW.finding_id AND f.task_id = NEW.task_id
          AND (NEW.disposition != 'defer' OR f.severity IN ('P2', 'P3')))
BEGIN SELECT RAISE(ABORT, 'FORGE_COORDINATION_REVIEW_DECISION_STALE'); END;

CREATE TABLE review_rework_round (
    report_artifact_id  TEXT PRIMARY KEY
                            REFERENCES review_range_report(report_artifact_id) ON DELETE RESTRICT,
    task_id             TEXT NOT NULL REFERENCES task(id) ON DELETE RESTRICT,
    round               INTEGER NOT NULL CHECK (round BETWEEN 1 AND 8),
    request_id          TEXT REFERENCES task_coordination_request(id) ON DELETE RESTRICT,
    created_by_kind     TEXT NOT NULL CHECK (created_by_kind IN ('human', 'agent')),
    created_by_id       TEXT NOT NULL,
    idempotency_key     TEXT NOT NULL CHECK (length(trim(idempotency_key)) BETWEEN 1 AND 256),
    created_at          TEXT NOT NULL,
    UNIQUE (task_id, idempotency_key)
);

CREATE TABLE review_correction_link (
    correction_work_unit_id  TEXT NOT NULL,
    finding_id               TEXT NOT NULL REFERENCES review_finding(id) ON DELETE RESTRICT,
    task_id                  TEXT NOT NULL,
    repo_id                  TEXT NOT NULL REFERENCES repo(id) ON DELETE RESTRICT,
    report_artifact_id       TEXT NOT NULL
                                 REFERENCES review_rework_round(report_artifact_id) ON DELETE RESTRICT,
    base_commit              TEXT NOT NULL CHECK (length(trim(base_commit)) > 0),
    created_at               TEXT NOT NULL,
    PRIMARY KEY (correction_work_unit_id, finding_id),
    FOREIGN KEY (correction_work_unit_id, task_id)
        REFERENCES work_unit(id, task_id) ON DELETE RESTRICT
);
CREATE INDEX idx_review_correction_link_finding ON review_correction_link(finding_id);

CREATE TABLE task_review_coverage_state (
    task_id     TEXT NOT NULL REFERENCES task(id) ON DELETE RESTRICT,
    repo_id     TEXT NOT NULL REFERENCES repo(id) ON DELETE RESTRICT,
    revision    INTEGER NOT NULL CHECK (revision >= 1),
    updated_at  TEXT NOT NULL,
    PRIMARY KEY (task_id, repo_id)
);
CREATE TRIGGER task_review_coverage_state_monotonic
BEFORE UPDATE ON task_review_coverage_state
WHEN NEW.task_id IS NOT OLD.task_id OR NEW.repo_id IS NOT OLD.repo_id
  OR NEW.revision != OLD.revision + 1
BEGIN SELECT RAISE(ABORT, 'Review coverage revision advances exactly once'); END;
```

Incrementos de cobertura. Todos usan este cuerpo (forma **U**), cambiando origen
de `task_id`/`repo_id`; todos exigen política vigente:

```sql
CREATE TRIGGER review_coverage_bump_policy_insert
AFTER INSERT ON task_review_policy_revision
BEGIN
    INSERT INTO task_review_coverage_state(task_id, repo_id, revision, updated_at)
    SELECT v.task_id, m.repo_id, 1, strftime('%Y-%m-%dT%H:%M:%fZ','now')
    FROM task_repository_set_revision v
    JOIN task_repository_set_revision_repo m ON m.task_id = v.task_id AND m.revision = v.revision
    WHERE v.task_id = NEW.task_id AND v.state = 'current' AND m.role IN ('primary', 'affected')
    ON CONFLICT(task_id, repo_id) DO UPDATE
        SET revision = revision + 1, updated_at = excluded.updated_at;
END;

CREATE TRIGGER review_coverage_bump_integration_insert
AFTER INSERT ON work_unit_repository_integration
BEGIN
    INSERT INTO task_review_coverage_state(task_id, repo_id, revision, updated_at)
    SELECT NEW.task_id, NEW.repo_id, 1, strftime('%Y-%m-%dT%H:%M:%fZ','now')
    WHERE EXISTS (SELECT 1 FROM task_review_policy_revision p
                  WHERE p.task_id = NEW.task_id AND p.state = 'current')
    ON CONFLICT(task_id, repo_id) DO UPDATE
        SET revision = revision + 1, updated_at = excluded.updated_at;
END;
```

| Trigger (forma U, `NEW.task_id`, `NEW.repo_id`) | Evento |
|---|---|
| `review_coverage_bump_integration_update` | `AFTER UPDATE OF outcome, target_after_sha ON work_unit_repository_integration` |
| `review_coverage_bump_checkpoint_insert` / `_update` | `AFTER INSERT` / `AFTER UPDATE ON review_checkpoint` |
| `review_coverage_bump_report_insert` | `AFTER INSERT ON review_range_report` |
| `review_coverage_bump_resolution_insert` | `AFTER INSERT ON review_finding_resolution` |
| `review_coverage_bump_base_insert` | `AFTER INSERT ON task_review_base` |
| `review_coverage_bump_attribution_insert` | `AFTER INSERT ON review_commit_attribution` |

`review_coverage_bump_result_line` (`AFTER UPDATE ON work_unit_review_line`) usa
la forma de `..._policy_insert` con `NEW.task_id` y añade
`AND EXISTS (política vigente)` (todos los repos escribibles).

Inmutabilidad. Para cada tabla `T` de las listas se crea
`T_immutable_delete` y, si es inmutable, `T_immutable_update`:

```sql
CREATE TRIGGER T_immutable_update BEFORE UPDATE ON T
BEGIN SELECT RAISE(ABORT, 'T is immutable'); END;
CREATE TRIGGER T_immutable_delete BEFORE DELETE ON T
WHEN NOT EXISTS (SELECT 1 FROM task t JOIN project_deletion_guard g ON g.project_id = t.project_id
                 WHERE t.id = OLD.task_id)
BEGIN SELECT RAISE(ABORT, 'T is preserved outside Project teardown'); END;
```

- update + delete: `task_review_base`, `review_commit_attribution`,
  `review_range`, `review_checkpoint_request`, `review_range_contributor`,
  `work_unit_review_range`, `review_range_report`, `review_finding`,
  `review_finding_location`, `review_finding_resolution`,
  `review_finding_triage`, `review_rework_round`, `review_correction_link`;
- solo delete (update ya guardado): `task_review_policy_revision`,
  `review_checkpoint`, `review_plan_checkpoint`, `task_review_coverage_state`;
- `project_review_policy_immutable_delete` usa
  `WHEN NOT EXISTS (SELECT 1 FROM project_deletion_guard g WHERE g.project_id = OLD.project_id)`.

Teardown de Project (`sqlite/project.rs`): anteponer a la primera sentencia de la
lista actual, en este orden, `DELETE ... WHERE task_id IN (SELECT id FROM task
WHERE project_id = ?)` sobre: `review_correction_link`, `review_rework_round`,
`review_finding_triage`, `review_finding_resolution`, `review_finding_location`,
`review_finding`, `review_range_report`, `work_unit_review_range`,
`review_range_contributor`, `review_checkpoint_request`, `review_checkpoint`,
`review_plan_checkpoint`, `review_range`, `review_commit_attribution`,
`task_review_base`, `task_review_coverage_state`, `task_review_policy_revision`;
después `DELETE FROM project_review_policy WHERE project_id = ?`. Los borrados
posteriores sobre `work_unit_repository_integration` no deben disparar
incrementos (ya no hay política).

### C0.4 `V185__review_range_authority.sql`

Migración directa sobre una conexión (patrón V168/V183):

```sql
PRAGMA foreign_keys = OFF;
PRAGMA legacy_alter_table = ON;
BEGIN IMMEDIATE;
CREATE TEMP TABLE _v185_check(ok INTEGER NOT NULL CHECK(ok = 1));
-- secciones 1 a 8
INSERT INTO _v185_check SELECT NOT EXISTS(SELECT 1 FROM pragma_foreign_key_check);
INSERT INTO _v185_check SELECT (SELECT integrity_check FROM pragma_integrity_check LIMIT 1) = 'ok';
DROP TABLE _v185_check;
INSERT INTO _migration(version, name, applied_at)
VALUES (185, 'review_range_authority', strftime('%Y-%m-%dT%H:%M:%fZ','now'));
COMMIT;
PRAGMA legacy_alter_table = OFF;
PRAGMA foreign_keys = ON;
```

**1. Independencia.** `DROP TRIGGER work_unit_review_independent_execution;` y
recrearlo copiando **literalmente** V162 líneas 191–269, con un único añadido:
tras `WHEN NEW.work_unit_id IS NOT NULL` insertar

```sql
 AND NOT EXISTS (SELECT 1 FROM work_unit_review_range r
                 WHERE r.review_work_unit_id = NEW.work_unit_id)
```

Triggers nuevos:

```sql
CREATE TRIGGER work_unit_range_review_independent_execution
BEFORE INSERT ON execution
WHEN EXISTS (SELECT 1 FROM work_unit_review_range r WHERE r.review_work_unit_id = NEW.work_unit_id)
 AND (
     NOT EXISTS (
         SELECT 1 FROM work_unit_review_range r
         JOIN review_checkpoint c ON c.id = r.checkpoint_id AND c.state IN ('pending', 'in_review')
         JOIN work_unit w ON w.id = r.review_work_unit_id AND w.task_id = NEW.task_id
         JOIN task_role role ON role.task_id = w.task_id AND role.role = w.role
         JOIN task_role_capability_configuration cfg ON cfg.task_role_id = role.id
         WHERE r.review_work_unit_id = NEW.work_unit_id
           AND EXISTS (SELECT 1 FROM json_each(cfg.configuration_json, '$.capabilities')
                       WHERE type = 'text' AND value = 'formal_review'))
     OR EXISTS (
         SELECT 1 FROM work_unit_review_range r
         JOIN review_range_contributor x ON x.range_id = r.range_id
         WHERE r.review_work_unit_id = NEW.work_unit_id
           AND x.actor_kind = NEW.actor_kind AND x.actor_id = NEW.actor_id
           AND (NEW.actor_kind = 'human' OR NEW.harness_session_id IS NULL)))
BEGIN SELECT RAISE(ABORT, 'WorkUnit Execution violates exact range review independence or readiness'); END;

CREATE TRIGGER range_review_session_isolation_insert
BEFORE INSERT ON execution
WHEN NEW.actor_kind = 'agent' AND NEW.harness_session_id IS NOT NULL AND EXISTS (
    WITH RECURSIVE ancestors(id) AS (
        SELECT NEW.harness_session_id
        UNION
        SELECT s.predecessor_session_id FROM harness_session s JOIN ancestors a ON a.id = s.id
        WHERE s.predecessor_session_id IS NOT NULL)
    SELECT 1 FROM work_unit_review_range r
    JOIN review_range_contributor x ON x.range_id = r.range_id
    JOIN execution produced ON produced.id = x.execution_id
    JOIN ancestors a ON a.id = produced.harness_session_id
    WHERE r.review_work_unit_id = NEW.work_unit_id)
BEGIN SELECT RAISE(ABORT, 'Range review requires a session without the producer context'); END;
```

`range_review_session_isolation_update`: idéntico, con
`BEFORE UPDATE OF harness_session_id ON execution` y la condición adicional
`NEW.harness_session_id IS NOT OLD.harness_session_id`.

```sql
CREATE TRIGGER work_unit_range_review_allocation
BEFORE UPDATE OF role, assigned_actor_kind, assigned_actor_id ON work_unit
WHEN EXISTS (SELECT 1 FROM work_unit_review_range r WHERE r.review_work_unit_id = NEW.id)
 AND (
     NOT EXISTS (
         SELECT 1 FROM task_role role
         JOIN task_role_capability_configuration cfg ON cfg.task_role_id = role.id
         WHERE role.task_id = NEW.task_id AND role.role = NEW.role
           AND EXISTS (SELECT 1 FROM json_each(cfg.configuration_json, '$.capabilities')
                       WHERE type = 'text' AND value = 'formal_review'))
     OR (NEW.assigned_actor_kind = 'human' AND EXISTS (
         SELECT 1 FROM work_unit_review_range r
         JOIN review_range_contributor x ON x.range_id = r.range_id
         WHERE r.review_work_unit_id = NEW.id
           AND x.actor_kind = 'human' AND x.actor_id = NEW.assigned_actor_id)))
BEGIN SELECT RAISE(ABORT, 'WorkUnit allocation violates exact range review independence'); END;
```

**2. Admisión del workspace revisor.**

```sql
CREATE TRIGGER work_unit_range_review_admission_guard
BEFORE INSERT ON execution
WHEN EXISTS (SELECT 1 FROM work_unit_review_range r WHERE r.review_work_unit_id = NEW.work_unit_id)
 AND (
     NEW.workspace_id IS NOT NULL
     OR NOT EXISTS (
         SELECT 1 FROM work_unit_review_range rr
         JOIN review_range r ON r.id = rr.range_id
         JOIN workspace_scope s ON s.work_unit_id = rr.review_work_unit_id
           AND s.task_id = rr.task_id AND s.scope_kind = 'work_unit' AND s.repo_id = rr.repo_id
         JOIN workspace_group g ON g.id = s.workspace_group_id
           AND g.work_unit_id = rr.review_work_unit_id
         JOIN workspace w ON w.id = s.workspace_id AND w.status = 'ready'
           AND w.before_sha IS r.head_commit
         WHERE rr.review_work_unit_id = NEW.work_unit_id)
     OR EXISTS (
         SELECT 1 FROM work_unit_review_range rr
         JOIN work_unit_repository_set_revision pin
           ON pin.work_unit_id = rr.review_work_unit_id AND pin.task_id = rr.task_id
         JOIN task_repository_set_revision_repo m
           ON m.task_id = pin.task_id AND m.revision = pin.revision AND m.role = 'context'
         WHERE rr.review_work_unit_id = NEW.work_unit_id AND NOT EXISTS (
             SELECT 1 FROM workspace_scope s
             JOIN workspace w ON w.id = s.workspace_id AND w.status = 'ready'
             JOIN task_repository_context_snapshot snap
               ON snap.task_id = rr.task_id AND snap.repo_id = m.repo_id
              AND snap.commit_sha IS w.before_sha
             WHERE s.work_unit_id = rr.review_work_unit_id AND s.scope_kind = 'work_unit'
               AND s.repo_id = m.repo_id))
     OR EXISTS (
         SELECT 1 FROM work_unit_review_range rr
         JOIN workspace_scope s ON s.work_unit_id = rr.review_work_unit_id
           AND s.scope_kind = 'work_unit'
         JOIN workspace w ON w.id = s.workspace_id
         JOIN work_unit_repository_set_revision pin
           ON pin.work_unit_id = rr.review_work_unit_id AND pin.task_id = rr.task_id
         LEFT JOIN task_repository_set_revision_repo m
           ON m.task_id = pin.task_id AND m.revision = pin.revision AND m.repo_id = s.repo_id
         WHERE rr.review_work_unit_id = NEW.work_unit_id
           AND (w.status != 'ready' OR m.repo_id IS NULL
                OR (s.repo_id != rr.repo_id AND m.role != 'context'))))
BEGIN SELECT RAISE(ABORT, 'FORGE_WU_ADMISSION_REVIEW_GROUP_UNAVAILABLE'); END;

CREATE TRIGGER work_unit_review_range_reviewer_role_immutable
BEFORE UPDATE OF role ON work_unit
WHEN EXISTS (SELECT 1 FROM work_unit_review_range r WHERE r.review_work_unit_id = NEW.id)
 AND NOT EXISTS (
     SELECT 1 FROM task_role role
     JOIN task_role_capability_configuration cfg ON cfg.task_role_id = role.id
     WHERE role.task_id = NEW.task_id AND role.role = NEW.role
       AND EXISTS (SELECT 1 FROM json_each(cfg.configuration_json, '$.capabilities')
                   WHERE type = 'text' AND value = 'formal_review'))
BEGIN SELECT RAISE(ABORT, 'review WorkUnit requires a TaskRole with formal_review capability'); END;
```

`DROP TRIGGER workspace_scope_insert_guard;` y recrearlo copiando
**literalmente** V183 líneas 1228–1303, con un único añadido: dentro del
paréntesis de ramas de `scope_kind = 'work_unit'`, después del
`OR EXISTS (... work_unit_review_subject_repository subject ...)`, insertar

```sql
                          OR EXISTS (
                              SELECT 1 FROM work_unit_review_range rr
                              JOIN review_range range_row ON range_row.id = rr.range_id
                              WHERE rr.review_work_unit_id = w.id AND rr.task_id = t.id
                                AND ((range_row.repo_id = ws.repo_id
                                      AND repository.role IN ('primary', 'affected')
                                      AND range_row.head_commit IS ws.before_sha)
                                  OR (repository.role = 'context' AND EXISTS (
                                      SELECT 1 FROM task_repository_context_snapshot snapshot
                                      WHERE snapshot.task_id = t.id AND snapshot.repo_id = ws.repo_id
                                        AND snapshot.commit_sha IS ws.before_sha)))
                          )
```

(La activación inserta `work_unit_review_range` antes de preparar el workspace.)

**3. Triage con `defer`.**

```sql
CREATE TABLE task_review_triage_v185 (
    review_report_artifact_id TEXT PRIMARY KEY REFERENCES artifact(id) ON DELETE RESTRICT,
    task_id TEXT NOT NULL REFERENCES task(id) ON DELETE CASCADE,
    report_digest TEXT NOT NULL,
    execution_id TEXT NOT NULL REFERENCES execution(id) ON DELETE RESTRICT,
    authorization_version INTEGER NOT NULL,
    disposition TEXT NOT NULL CHECK(disposition IN ('correct','recheck','human','defer')),
    rationale TEXT NOT NULL,
    instructions TEXT NOT NULL,
    decision_digest TEXT NOT NULL,
    review_work_unit_id TEXT REFERENCES work_unit(id) ON DELETE RESTRICT,
    created_at TEXT NOT NULL
);
INSERT INTO task_review_triage_v185 SELECT * FROM task_review_triage;
INSERT INTO _v185_check SELECT NOT EXISTS(
    SELECT * FROM task_review_triage EXCEPT SELECT * FROM task_review_triage_v185);
ALTER TABLE task_review_triage RENAME TO task_review_triage_v164;
ALTER TABLE task_review_triage_v185 RENAME TO task_review_triage;
DROP TABLE task_review_triage_v164;
```

Recrear sin cambios `task_review_triage_immutable`,
`task_review_triage_immutable_delete` (V164) y
`task_review_triage_current_request_guard` (V168 líneas 326–335). Añadir:

```sql
CREATE TRIGGER task_review_triage_defer_range_only
BEFORE INSERT ON task_review_triage
WHEN NEW.disposition = 'defer' AND NOT EXISTS (
    SELECT 1 FROM review_range_report p WHERE p.report_artifact_id = NEW.review_report_artifact_id)
BEGIN SELECT RAISE(ABORT, 'defer is only valid for range review findings'); END;

CREATE TRIGGER micro_fix_receipt_range_review_guard
BEFORE INSERT ON micro_fix_receipt
WHEN NEW.source_kind = 'review_findings' AND EXISTS (
    SELECT 1 FROM task_coordination_request q
    JOIN review_range_report p ON p.report_artifact_id = q.review_report_artifact_id
    WHERE q.id = NEW.request_id)
BEGIN SELECT RAISE(ABORT, 'FORGE_MICRO_FIX_NOT_AUTHORIZED'); END;
```

**4. Vista de fuente.**
`DROP VIEW task_review_coordination_source_current;` y
`CREATE VIEW task_review_coordination_source_current AS` con el `SELECT` de V168
líneas 102–123 literal (rama de resultado, sin el `;`), seguido de:

```sql
UNION ALL
SELECT report.id, report.task_id, report.digest, producer.execution_id, c.authorization_version
 FROM artifact report
 JOIN artifact_execution_producer producer
   ON producer.artifact_id = report.id AND producer.task_id = report.task_id
 JOIN execution reviewer
   ON reviewer.id = producer.execution_id AND reviewer.task_id = report.task_id
 JOIN review_range_report rr ON rr.report_artifact_id = report.id AND rr.task_id = report.task_id
  AND rr.execution_id = reviewer.id AND rr.report_digest = report.digest
  AND rr.verdict = 'request_changes'
 JOIN review_checkpoint cp ON cp.id = rr.checkpoint_id AND cp.state = 'reviewed'
 JOIN task_coordination c ON c.task_id = report.task_id
 JOIN task t ON t.id = c.task_id AND t.deleted_at IS NULL
 JOIN task_lifecycle lifecycle ON lifecycle.task_id = t.id
  AND lifecycle.state NOT IN ('done','cancelled','merging')
 JOIN task_dispatch dispatch ON dispatch.task_id = t.id AND dispatch.state = 'running'
 WHERE report.kind = 'review_report' AND reviewer.status = 'completed'
  AND c.mode != 'manual' AND c.revoked_at IS NULL AND c.team_stale = 0
  AND COALESCE(json_extract(c.envelope_json,'$.orchestrator_reviews_findings'),0) = 1
  AND NOT EXISTS(SELECT 1 FROM task_review_triage x WHERE x.review_report_artifact_id = report.id)
  AND NOT EXISTS(SELECT 1 FROM review_rework_round x WHERE x.report_artifact_id = report.id)
  AND NOT EXISTS(SELECT 1 FROM review_checkpoint newer WHERE newer.range_id = cp.range_id
       AND newer.attempt > cp.attempt AND newer.state = 'reviewed');
```

Orden dentro de la transacción de triage: fila agregada `task_review_triage`
(su guard necesita la fuente vigente), filas por hallazgo, ronda, WorkUnits y
enlaces, estado `triaged`.

**5. Proposals.** `DROP TRIGGER proposal_actor_target_guard_insert;` y recrear la
definición V177 con: la lista inicial ampliada con `'review_finding'`,
`'review_base'`, `'review_commit'`, y estas ramas:

```sql
  OR (NEW.target_kind = 'review_finding' AND NOT EXISTS (
          SELECT 1 FROM review_finding f
          WHERE f.id = NEW.target_id AND f.task_id = NEW.task_id AND f.severity != 'P0'))
  OR (NEW.target_kind IN ('review_base', 'review_commit') AND NOT EXISTS (
          SELECT 1 FROM task_review_coverage_state s
          WHERE s.task_id = NEW.task_id AND s.repo_id = NEW.target_id))
```

**6. `gate_evaluation_input`** (patrón V155): `DROP VIEW
gate_evaluation_stale_input;` `DROP TRIGGER` de `gate_evaluation_input_same_task_insert`,
`..._immutable_update`, `..._immutable_delete`, `..._review_line_guard`;
`DROP INDEX idx_gate_evaluation_input_ref;`. Crear
`gate_evaluation_input_v185` con las columnas, PK, UNIQUE y FK de V155 líneas
10–30 y como único cambio `'review_coverage'` añadido al CHECK de `input_kind`.
Copiar con lista explícita de columnas, comprobar con `EXCEPT` en `_v185_check`,
renombrar, borrar la anterior, recrear el índice y los triggers de inmutabilidad
(V155 126–142) y `..._review_line_guard` (V155 616–631) literales. Recrear
`gate_evaluation_input_same_task_insert` literal de V155 48–124 añadiendo antes
de `BEGIN`:

```sql
  OR (NEW.input_kind = 'review_coverage' AND NOT EXISTS (
      SELECT 1 FROM task_review_coverage_state s
      JOIN task_review_policy_revision p ON p.task_id = s.task_id AND p.state = 'current'
      WHERE s.task_id = NEW.task_id AND s.repo_id = NEW.input_id
        AND s.revision = NEW.input_version
        AND p.revision = json_extract(NEW.subject_json, '$.review_policy_revision')
        AND p.digest = json_extract(NEW.subject_json, '$.review_policy_digest')
        AND NEW.input_digest = json_extract(NEW.subject_json, '$.snapshot_digest')))
```

**7. Vigencia.** `CREATE VIEW gate_evaluation_stale_input` literal de V169 líneas
5–122, sin el `;` final, más:

```sql
 OR (i.input_kind = 'review_coverage' AND NOT EXISTS (
        SELECT 1 FROM task_review_coverage_state s
        JOIN task_review_policy_revision p ON p.task_id = s.task_id AND p.state = 'current'
        WHERE s.task_id = i.task_id AND s.repo_id = i.input_id AND s.revision = i.input_version
          AND p.revision = json_extract(i.subject_json, '$.review_policy_revision')
          AND p.digest = json_extract(i.subject_json, '$.review_policy_digest')
    ));
```

**8. Fail closed.**

```sql
CREATE VIEW gate_evaluation_review_coverage_missing AS
SELECT e.id AS evaluation_id
FROM gate_evaluation e
JOIN gate g ON g.id = e.gate_id AND g.task_id = e.task_id AND g.gate_kind = 'merge_readiness'
JOIN task_review_policy_revision p ON p.task_id = e.task_id AND p.state = 'current'
JOIN gate_policy_revision r ON r.gate_id = e.gate_id AND r.revision = e.policy_revision
WHERE r.schema_version != 4
   OR json_extract(r.policy_json, '$.review_coverage.review_policy_revision') IS NOT p.revision
   OR json_extract(r.policy_json, '$.review_coverage.review_policy_digest') IS NOT p.digest
   OR EXISTS (SELECT 1 FROM gate_evaluation_input i
        WHERE i.evaluation_id = e.id AND i.input_kind = 'review_coverage'
          AND i.status NOT IN ('satisfied', 'not_required', 'unchanged'))
   OR EXISTS (SELECT 1 FROM work_unit_repository_integration w
        JOIN task_repository_set_revision v ON v.task_id = w.task_id AND v.state = 'current'
        JOIN task_repository_set_revision_repo m ON m.task_id = v.task_id
          AND m.revision = v.revision AND m.repo_id = w.repo_id
          AND m.role IN ('primary', 'affected')
        WHERE w.task_id = e.task_id AND w.outcome = 'success' AND NOT EXISTS (
            SELECT 1 FROM gate_evaluation_input i WHERE i.evaluation_id = e.id
              AND i.input_kind = 'review_coverage' AND i.input_id = w.repo_id));

CREATE TRIGGER review_coverage_lifecycle_guard
BEFORE INSERT ON task_lifecycle_transition
WHEN NEW.to_state = 'ready_to_merge' AND EXISTS (
    SELECT 1 FROM gate_evaluation_review_coverage_missing x
    WHERE x.evaluation_id = NEW.gate_evaluation_id)
BEGIN SELECT RAISE(ABORT, 'gate.review_coverage_missing'); END;

CREATE TRIGGER review_coverage_integration_admission_guard
BEFORE INSERT ON task_integration_operation
WHEN NEW.kind IN ('task_merge', 'publish_pr') AND EXISTS (
    SELECT 1 FROM gate_evaluation_review_coverage_missing x
    WHERE x.evaluation_id = NEW.gate_evaluation_id)
BEGIN SELECT RAISE(ABORT, 'gate.review_coverage_missing'); END;

CREATE TRIGGER review_coverage_push_update_guard
BEFORE INSERT ON pr_external_action
WHEN NEW.kind = 'push_update' AND EXISTS (
    SELECT 1 FROM gate_evaluation_review_coverage_missing x
    WHERE x.evaluation_id = NEW.gate_evaluation_id)
BEGIN SELECT RAISE(ABORT, 'gate.review_coverage_missing'); END;
```

Un push de una acción ya admitida se revalida en Rust (vigencia + digest) antes
de llamar al proveedor.

### C0.5 `crates/db` — modelos y repositorios (lane A)

En `models.rs`. Todos `#[derive(Debug, Clone, PartialEq, Eq)]`; enums además
`Copy` y `enum_strings!` con el valor SQL (`Severity`: `"P0"`…`"P3"`).
Marcas de tiempo e IDs: `String`.

```rust
pub enum ReviewRiskClass { R0, R1, R2 }
pub enum ReviewPolicyRevisionState { Current, Superseded }
pub enum ReviewPolicySource { ProjectDefault, Owner }
pub enum ReviewBaseSource { FirstIntegration, WorkspacePrepare, HumanRebaseline }
pub enum ReviewCheckpointState { Queued, Pending, InReview, Reviewed, Blocked, Superseded }
pub enum ReviewCheckpointReason { Baseline, Delta, AfterWorkUnit, SizeThreshold, Requested, Final, Recheck }
pub enum ReviewRequestSourceKind { Human, Coordinator, Scheduler }
pub enum ReviewVerdict { Pass, RequestChanges, Questions }
pub enum ReviewFindingSeverity { P0, P1, P2, P3 }          // derive Ord; is_blocking() = P0|P1
pub enum ReviewResolutionOutcome { Fixed, NotReproduced, Waived }
pub enum ReviewAttributionKind { WorkUnit, Preexisting, Upstream, Unattributed }
pub enum ReviewFindingDisposition { Correct, Recheck, Human, Defer }
// ProposalTargetKind += ReviewFinding => "review_finding", ReviewBase => "review_base",
//                       ReviewCommit => "review_commit"

pub struct ReviewPolicyDocument {            // Serialize, Deserialize, deny_unknown_fields
    pub schema_version: u32,                 // 1
    pub risk_class: ReviewRiskClass,
    pub coverage_required: bool,
    pub trigger_mode: ReviewTriggerMode,     // Auto | Manual (snake_case)
    pub reviewer_role: String,
    pub required_reviewers: u32,
    pub require_human: bool,
    pub size_threshold: ReviewSizeThreshold, // { changed_lines: u64, changed_files: u64 }
    pub coordinator_may_request: bool,
    pub max_checkpoints_per_repo: u32,       // 1..=64, defecto 12
}
pub struct ProjectReviewPolicy { pub project_id: String, pub version: i64,
    pub policy: Option<ReviewPolicyDocument>, pub digest: Option<String>,
    pub updated_by_user_id: String, pub created_at: String, pub updated_at: String }
pub struct TaskReviewPolicyRevision { pub task_id: String, pub revision: i64,
    pub state: ReviewPolicyRevisionState, pub policy: ReviewPolicyDocument, pub digest: String,
    pub source: ReviewPolicySource, pub created_by_user_id: String,
    pub created_at: String, pub superseded_at: Option<String> }
pub struct ReviewPolicyRef { pub revision: i64, pub digest: String }
pub struct TaskReviewBase { pub task_id: String, pub repo_id: String, pub revision: i64,
    pub base_commit: String, pub base_tree: String, pub source: ReviewBaseSource,
    pub decision_id: Option<String>, pub pinned_at: String }
pub struct ReviewCommitAttribution { pub id: String, pub task_id: String, pub repo_id: String,
    pub commit_sha: String, pub actor: ActorRef, pub decision_id: String, pub created_at: String }
pub struct ReviewRange { pub id: String, pub task_id: String, pub repo_id: String,
    pub repository_set_revision: i64, pub base_commit: String, pub head_commit: String,
    pub base_tree: String, pub head_tree: String, pub range_digest: String,
    pub integration_workspace_id: String, pub head_integration_id: String, pub created_at: String }
pub struct ReviewCheckpoint { pub id: String, pub task_id: String, pub repo_id: String,
    pub state: ReviewCheckpointState, pub reasons: Vec<ReviewCheckpointReason>, pub attempt: i64,
    pub base_commit: Option<String>, pub head_commit: String, pub range_id: Option<String>,
    pub policy_revision: i64, pub blocked_code: Option<String>,
    pub blocked_detail_json: Option<String>, pub version: i64,
    pub created_at: String, pub updated_at: String }
pub struct ReviewPlanCheckpoint { pub task_id: String, pub plan_artifact_id: String,
    pub plan_key: String, pub work_unit_ids: Vec<String>, pub repository_ids: Vec<String>,
    pub reason: String, pub active: bool, pub created_at: String }
pub struct ReviewRangeContributor { pub id: String, pub task_id: String, pub range_id: String,
    pub work_unit_id: Option<String>, pub execution_id: Option<String>,
    pub attribution_id: Option<String>, pub actor: ActorRef, pub created_at: String }
pub struct ReviewInputFinding { pub finding_id: String, pub severity: ReviewFindingSeverity,
    pub digest: String }                     // Serialize; orden por finding_id
pub struct WorkUnitReviewRange { pub review_work_unit_id: String, pub task_id: String,
    pub checkpoint_id: String, pub range_id: String, pub repo_id: String,
    pub input_findings: Vec<ReviewInputFinding>, pub input_findings_digest: String,
    pub created_at: String }
pub struct ReviewRangeReport { pub report_artifact_id: String, pub task_id: String,
    pub repo_id: String, pub range_id: String, pub checkpoint_id: String,
    pub review_work_unit_id: String, pub execution_id: String, pub actor: ActorRef,
    pub verdict: ReviewVerdict, pub report_digest: String, pub created_at: String }
pub struct ReviewFinding { pub id: String, pub task_id: String, pub repo_id: String,
    pub range_id: String, pub report_artifact_id: String, pub ordinal: i64, pub label: String,
    pub severity: ReviewFindingSeverity, pub title: String, pub evidence: String,
    pub invariant: Option<String>, pub anchor_commit: String, pub digest: String,
    pub created_at: String }
pub struct ReviewFindingLocation { pub finding_id: String, pub task_id: String, pub ordinal: i64,
    pub path: String, pub line_start: Option<i64>, pub line_end: Option<i64>,
    pub attribution: ReviewLocationAttribution }
pub struct ReviewLocationAttribution { pub kind: ReviewAttributionKind,
    pub work_unit_id: Option<String>, pub execution_id: Option<String>,
    pub actor: Option<ActorRef>, pub commit_sha: Option<String>,
    pub integration_id: Option<String>, pub line_count: i64 }
pub struct ReviewFindingResolution { pub id: String, pub finding_id: String, pub task_id: String,
    pub repo_id: String, pub outcome: ReviewResolutionOutcome, pub anchor_commit: String,
    pub report_artifact_id: Option<String>, pub decision_id: Option<String>,
    pub waiver_reference: Option<String>, pub evidence: String, pub actor: ActorRef,
    pub created_at: String }
pub struct ReviewFindingTriage { pub finding_id: String, pub request_id: String,
    pub task_id: String, pub report_artifact_id: String, pub execution_id: String,
    pub authorization_version: i64, pub disposition: ReviewFindingDisposition,
    pub rationale: String, pub instructions: String, pub created_at: String }
pub struct ReviewReworkRound { pub report_artifact_id: String, pub task_id: String,
    pub round: i64, pub request_id: Option<String>, pub created_by: ActorRef,
    pub idempotency_key: String, pub created_at: String }
pub struct ReviewCorrectionLink { pub correction_work_unit_id: String, pub finding_id: String,
    pub task_id: String, pub repo_id: String, pub report_artifact_id: String,
    pub base_commit: String, pub created_at: String }
pub struct TaskReviewCoverageState { pub task_id: String, pub repo_id: String,
    pub revision: i64, pub updated_at: String }
/// Filas que necesita el evaluador puro; leídas en una sola transacción.
pub struct ReviewCoverageRows {
    pub policy: Option<TaskReviewPolicyRevision>, pub base: Option<TaskReviewBase>,
    pub state_revision: i64,
    pub integrations: Vec<WorkUnitRepositoryIntegration>,
    pub ranges: Vec<ReviewRange>, pub checkpoints: Vec<ReviewCheckpoint>,
    pub reports: Vec<ReviewRangeReport>, pub findings: Vec<ReviewFinding>,
    pub resolutions: Vec<ReviewFindingResolution>,
    pub attributions: Vec<ReviewCommitAttribution>,
    pub failing_result_lines: Vec<String>,   // line_id con último request_changes en el repo
}
```

En `repository.rs` (`#[async_trait]`, `Result<T> = db::Result<T>`; impl en
`sqlite/review_range.rs`). Cada writer es `BEGIN IMMEDIATE`, revalida versiones y
escribe sus `events` en la misma transacción. CAS fallido →
`DbError::VersionConflict`; el replay idempotente devuelve la fila existente.

```rust
pub struct WriteTaskReviewPolicy { pub task_id: String, pub expected_revision: Option<i64>,
    pub policy: ReviewPolicyDocument, pub digest: String, pub source: ReviewPolicySource,
    pub created_by_user_id: String,
    pub gate_revisions: Vec<ReviseGateWithPolicy>, // { gate_id, expected_active_revision: Option<i64>, revision: CreateGatePolicyRevision }
    pub initial_bases: Vec<TaskReviewBase>, pub events: Vec<CreateDomainEvent>, pub created_at: String }
pub struct WriteProjectReviewPolicy { pub project_id: String, pub expected_version: Option<i64>,
    pub policy: Option<(ReviewPolicyDocument, String)>, pub updated_by_user_id: String,
    pub updated_at: String }

#[async_trait] pub trait ReviewPolicyRepo {
    async fn get_current_task_review_policy(&self, task_id: &str) -> Result<Option<TaskReviewPolicyRevision>>;
    async fn write_task_review_policy(&self, input: WriteTaskReviewPolicy) -> Result<TaskReviewPolicyRevision>;
    async fn get_project_review_policy(&self, project_id: &str) -> Result<Option<ProjectReviewPolicy>>;
    async fn write_project_review_policy(&self, input: WriteProjectReviewPolicy) -> Result<ProjectReviewPolicy>;
}
// CreateTask gana `review_policy: Option<WriteTaskReviewPolicy>` solo si el writer de Task
// puede aplicarlo en su misma transacción (copia del default del Project).

pub struct ReviewCheckpointRequestRecord { pub idempotency_key: String,
    pub source_kind: ReviewRequestSourceKind, pub source_ref: String, pub reason: String }
pub struct UpsertQueuedReviewCheckpoint { pub checkpoint: ReviewCheckpoint,
    pub request: Option<ReviewCheckpointRequestRecord>,
    pub supersede: Vec<(String /*id*/, i64 /*expected_version*/)>, pub events: Vec<CreateDomainEvent> }
pub struct ActivateReviewCheckpoint { pub checkpoint_id: String, pub expected_version: i64,
    pub expected_coverage_revision: i64, pub range: ReviewRange,
    pub contributors: Vec<ReviewRangeContributor>,   // ignorados si el rango ya los tiene congelados
    pub work_unit: CreateWorkUnit, pub review_range: WorkUnitReviewRange,
    pub updated_at: String, pub events: Vec<CreateDomainEvent> }
pub struct TransitionReviewCheckpoint { pub checkpoint_id: String, pub expected_version: i64,
    pub state: ReviewCheckpointState, pub add_reasons: Vec<ReviewCheckpointReason>,
    pub blocked_code: Option<String>, pub blocked_detail_json: Option<String>,
    pub cancel_review_work_units: bool, pub updated_at: String, pub events: Vec<CreateDomainEvent> }

#[async_trait] pub trait ReviewCheckpointRepo {
    async fn get_review_checkpoint(&self, id: &str) -> Result<Option<ReviewCheckpoint>>;
    async fn list_review_checkpoints(&self, task_id: &str, repo_id: Option<&str>, page: PageRequest)
        -> Result<Page<ReviewCheckpoint>>;
    async fn upsert_queued_review_checkpoint(&self, input: UpsertQueuedReviewCheckpoint) -> Result<ReviewCheckpoint>;
    async fn activate_review_checkpoint(&self, input: ActivateReviewCheckpoint) -> Result<ReviewCheckpoint>;
    async fn transition_review_checkpoint(&self, input: TransitionReviewCheckpoint) -> Result<ReviewCheckpoint>;
    async fn get_review_range(&self, id: &str) -> Result<Option<ReviewRange>>;
    async fn get_work_unit_review_range(&self, work_unit_id: &str) -> Result<Option<WorkUnitReviewRange>>;
    async fn list_review_range_contributors(&self, range_id: &str) -> Result<Vec<ReviewRangeContributor>>;
    async fn list_tasks_with_review_policy(&self, after_task_id: Option<&str>, limit: u32) -> Result<Vec<String>>;
    async fn list_active_review_plan_checkpoints(&self, task_id: &str) -> Result<Vec<ReviewPlanCheckpoint>>;
}

pub struct IngestRangeReport { pub report: ReviewRangeReport,
    pub findings: Vec<(ReviewFinding, Vec<ReviewFindingLocation>)>,
    pub resolutions: Vec<ReviewFindingResolution>,
    pub expected_checkpoint_version: i64,
    pub next_checkpoint_state: ReviewCheckpointState,      // InReview | Reviewed | Blocked
    pub blocked_code: Option<String>, pub events: Vec<CreateDomainEvent> }

#[async_trait] pub trait ReviewRangeReportRepo {
    /// Artifact, inputs, ledger, resoluciones y checkpoint en una transacción.
    async fn ingest_range_review_report(&self, artifact: CreateArtifact, evidence_input_ids: Vec<String>,
        artifact_input_ids: Vec<String>, artifact_event: CreateDomainEvent, range: IngestRangeReport)
        -> Result<ExecutionArtifactOutputWrite>;
    async fn get_review_range_report(&self, artifact_id: &str) -> Result<Option<ReviewRangeReport>>;
}

pub struct ReviewHumanDecision { pub proposal: CreateProposal, pub decision: CreateDecision }
// tipos existentes de CollaborationRepo
pub struct ReviewFindingFilter { pub repo_id: Option<String>,
    pub severity: Option<ReviewFindingSeverity>, pub finding_ids: Option<Vec<String>> }
pub struct ApplyReviewTriage { pub aggregate: /* fila task_review_triage */ CreateTaskReviewTriage,
    pub findings: Vec<ReviewFindingTriage>, pub corrections: Option<ApplyReviewCorrections>,
    pub recheck: Option<UpsertQueuedReviewCheckpoint>,
    pub expected_request_version: i64, pub events: Vec<CreateDomainEvent> }
pub struct ApplyReviewCorrections { pub round: ReviewReworkRound,
    pub work_units: Vec<CreateWorkUnit>, pub dependencies: Vec<AddWorkUnitDependency>,
    pub links: Vec<ReviewCorrectionLink>, pub max_rework_rounds: i64,
    pub events: Vec<CreateDomainEvent> }

#[async_trait] pub trait ReviewLedgerRepo {
    async fn load_review_coverage_rows(&self, task_id: &str, repo_id: &str) -> Result<ReviewCoverageRows>;
    async fn load_review_coverage_rows_in_tx(&self, conn: &mut SqliteConnection, task_id: &str, repo_id: &str)
        -> Result<ReviewCoverageRows>;
    async fn list_review_findings(&self, task_id: &str, filter: ReviewFindingFilter, page: PageRequest)
        -> Result<Page<ReviewFinding>>;
    async fn get_review_finding(&self, id: &str) -> Result<Option<ReviewFinding>>;
    async fn list_review_finding_locations(&self, finding_id: &str) -> Result<Vec<ReviewFindingLocation>>;
    async fn list_review_finding_resolutions(&self, finding_id: &str) -> Result<Vec<ReviewFindingResolution>>;
    async fn list_review_finding_triage(&self, finding_id: &str) -> Result<Vec<ReviewFindingTriage>>;
    async fn list_review_correction_links(&self, finding_id: &str) -> Result<Vec<ReviewCorrectionLink>>;
    async fn waive_review_finding(&self, expected_coverage_revision: i64, decision: ReviewHumanDecision,
        resolution: ReviewFindingResolution, events: Vec<CreateDomainEvent>) -> Result<ReviewFindingResolution>;
    async fn rebaseline_review(&self, expected_coverage_revision: i64, decision: ReviewHumanDecision,
        base: TaskReviewBase, events: Vec<CreateDomainEvent>) -> Result<TaskReviewBase>;
    async fn attribute_review_commits(&self, checkpoint_id: &str, expected_version: i64,
        decision: ReviewHumanDecision, attributions: Vec<ReviewCommitAttribution>,
        events: Vec<CreateDomainEvent>) -> Result<ReviewCheckpoint>;
    async fn apply_review_triage(&self, input: ApplyReviewTriage) -> Result<()>;
    async fn apply_review_corrections(&self, input: ApplyReviewCorrections) -> Result<ReviewReworkRound>;
    async fn count_review_rework_rounds(&self, task_id: &str) -> Result<i64>;
}
```

`PageRequest`/`Page`, `CreateProposal`, `CreateDecision` y
`CreateTaskReviewTriage` son los tipos existentes equivalentes; la lane A usa los
nombres reales (o introduce `CreateTaskReviewTriage` con las columnas de la tabla)
y los publica en su primer commit. `apply_review_corrections` cuenta
`task_rework_receipt + review_rework_round < max_rework_rounds` dentro de la
transacción; los contadores existentes de rondas deben sumar ambas tablas
(lane D).

### C0.6 Servicios (`crates/services`)

Errores: sin variantes nuevas de `ServiceError`; se usa
`ServiceError::Coordination { code, message }`.

```rust
// review_range/policy.rs  (lane B)
impl ReviewPolicyService {
    pub fn validate(doc: &ReviewPolicyDocument) -> Result<()>;          // review.policy.invalid
    pub fn digest(doc: &ReviewPolicyDocument) -> Result<String>;
    pub async fn current(&self, task_id: &str) -> Result<Option<TaskReviewPolicyRevision>>;
    pub async fn set(&self, user_id: &str, task_id: &str, expected_revision: Option<i64>,
        policy: ReviewPolicyDocument) -> Result<TaskReviewPolicyRevision>;
        // review.policy.version_conflict | review.policy.not_authorized | review.policy.invalid
    pub async fn project_default(&self, project_id: &str) -> Result<Option<ProjectReviewPolicy>>;
    pub async fn set_project_default(&self, user_id: &str, project_id: &str,
        expected_version: Option<i64>, policy: Option<ReviewPolicyDocument>) -> Result<ProjectReviewPolicy>;
    pub async fn pin_base(&self, task_id: &str, repo_id: &str) -> Result<Option<TaskReviewBase>>;
}

// review_range/coverage.rs  (lane B)
pub struct RepoCandidate { pub repo_id: String, pub commit_sha: String }
pub enum ReviewCoverageStatus { NotRequired, Unchanged, Uncovered, Partial, InReview,
    FindingsOpen, HistoryRewritten, Blocked, Satisfied }   // as_str() snake_case
pub struct CoveredRange { pub range_id: String, pub base_commit: String, pub head_commit: String,
    pub valid_edge: bool, pub report_artifact_ids: Vec<String> }   // ordenados por range_id / id
pub struct SeverityCounts { pub p0: u32, pub p1: u32, pub p2: u32, pub p3: u32 }
pub struct RepoCoverage { pub repo_id: String, pub status: ReviewCoverageStatus,
    pub reason_code: Option<String>, pub revision: i64, pub base_commit: Option<String>,
    pub candidate_commit: Option<String>, pub frontier_commit: Option<String>,
    pub unreviewed_commits: u32, pub ranges: Vec<CoveredRange>, pub open_findings: SeverityCounts,
    pub open_finding_ids: Vec<String>, pub resolution_ids: Vec<String>,
    pub active_checkpoint_id: Option<String>, pub snapshot_digest: String }
/// Hechos git precalculados fuera de transacción.
pub struct CommitFacts { pub candidate: String, pub ancestors_of: BTreeMap<String, BTreeSet<String>>,
    pub distance_to_candidate: BTreeMap<String, u32>, pub base_tree: String, pub candidate_tree: String }
pub struct CoverageFacts { pub task_id: String, pub repos: BTreeMap<String, (i64 /*revision*/, CommitFacts)> }
pub struct CoverageSnapshot { pub policy: ReviewPolicyRef, pub repos: Vec<RepoCoverage> } // por repo_id
pub fn evaluate_coverage(rows: &ReviewCoverageRows, facts: &CommitFacts) -> RepoCoverage;  // pura
impl ReviewCoverageService {
    pub async fn compute(&self, task_id: &str, repo_id: &str, candidate: Option<&str>) -> Result<RepoCoverage>;
    pub async fn compute_task(&self, task_id: &str) -> Result<Vec<RepoCoverage>>;
    pub async fn prepare(&self, task_id: &str, candidates: &[RepoCandidate]) -> Result<CoverageFacts>;
    pub async fn snapshot_in_tx(&self, conn: &mut SqliteConnection, facts: &CoverageFacts)
        -> Result<CoverageSnapshot>;                        // review.coverage.facts_stale
}
```

Precedencia de estado (el primero que aplique): `blocked` (base desconocida o no
ancestro, o checkpoint `blocked`), `unchanged`, `in_review` (checkpoint
`pending|in_review`), `findings_open` (P0/P1 abiertos o línea de resultado en
`request_changes`), `not_required` (`coverage_required=false`), `satisfied`,
`history_rewritten` (alguna arista revisada con head no ancestro de C),
`partial` (frontier ≠ B), `uncovered`. `snapshot_digest` cubre en orden:
`repo_id, review_policy_digest, base_commit, candidate_commit, ranges,
open_finding_ids, resolution_ids, active_checkpoint_id, status`.

```rust
// review_range/scheduler.rs  (lane B)
pub enum ReviewRequestSource { Human { user_id: String },
    Coordinator { execution_id: String, request_id: String } }
impl ReviewScheduler {
    pub async fn process_domain_event(&self, event: &DomainEvent) -> Result<usize>;
    pub async fn recover(&self, limit: u32) -> Result<usize>;
    pub async fn reconcile_task(&self, task_id: &str) -> Result<usize>;
    pub async fn request(&self, source: ReviewRequestSource, task_id: &str, repo_id: &str,
        reason: String, idempotency_key: &str) -> Result<ReviewCheckpoint>;
    pub async fn retry(&self, user_id: &str, checkpoint_id: &str, expected_version: i64)
        -> Result<ReviewCheckpoint>;      // blocked → superseded + queued attempt+1
    pub fn recheck_input(&self, report: &db::ReviewRangeReport, now: &str)
        -> UpsertQueuedReviewCheckpoint;  // consumido por triage (D)
}

// review_range/attribution.rs  (lane B)
pub struct FindingLocation { pub path: String, pub line_start: Option<i64>, pub line_end: Option<i64> }
pub struct RangeContributors { pub contributors: Vec<ReviewRangeContributor>,
    pub unrecorded_commits: Vec<String>, pub upstream_commits: Vec<String> }
impl ReviewAttribution {
    pub async fn attribute(&self, range: &db::ReviewRange, locations: &[FindingLocation])
        -> Result<Vec<ReviewLocationAttribution>>;   // 1:1 con locations
    pub async fn contributors(&self, task_id: &str, repo_id: &str, base: &str, head: &str)
        -> Result<RangeContributors>;
    pub fn primary_author(locations: &[ReviewLocationAttribution]) -> Option<ReviewLocationAttribution>;
}

// review_range/resolution.rs  (lane B)
impl ReviewFindingService {
    pub async fn open_findings(&self, task_id: &str, repo_id: &str, candidate: &str)
        -> Result<Vec<ReviewFinding>>;
    pub async fn waive(&self, user_id: &str, task_id: &str, finding_id: &str,
        expected_coverage_revision: i64, rationale: String, reference: String)
        -> Result<ReviewFindingResolution>;
    pub async fn rebaseline(&self, user_id: &str, task_id: &str, repo_id: &str,
        expected_coverage_revision: i64, rationale: String) -> Result<TaskReviewBase>;
        // base nueva = merge-base(rama por defecto, candidato); la resuelve Forge
    pub async fn attribute_commits(&self, user_id: &str, task_id: &str, checkpoint_id: &str,
        expected_version: i64, attributions: Vec<(String, ActorRef)>, rationale: String)
        -> Result<ReviewCheckpoint>;
}
```

`crates/git` (lane B), funciones libres con `repo_path: &Path`; reutilizar
`is_ancestor` y `commit_parents`:

```rust
pub async fn resolve_tree(repo_path: &Path, commit: &str) -> Result<String>;
pub async fn rev_list(repo_path: &Path, head: &str, exclude: &str) -> Result<Vec<String>>;
pub struct NumstatSummary { pub changed_files: u64, pub changed_lines: u64 }
pub async fn numstat(repo_path: &Path, base: &str, head: &str) -> Result<NumstatSummary>;
pub struct BlameLine { pub line: u32, pub commit: String }
pub async fn blame_lines(repo_path: &Path, commit: &str, path: &str, start: u32, end: u32) -> Result<Vec<BlameLine>>;
pub async fn path_exists_or_deleted(repo_path: &Path, base: &str, head: &str, path: &str) -> Result<bool>;
pub async fn merge_base(repo_path: &Path, left: &str, right: &str) -> Result<String>;
```

Gate v4 (lane E), en `gate_engine.rs` (submódulo `gate_engine/`):

```rust
#[serde(deny_unknown_fields)]
pub struct GateReviewCoverageRequirement { pub review_policy_revision: i64, pub review_policy_digest: String }
pub struct GatePolicyDocumentV4 { /* campos de V3 */ pub review_coverage: GateReviewCoverageRequirement }
/// Todo generador de Gates merge_readiness pasa por aquí.
pub fn merge_policy_preserving_coverage(next: serde_json::Value, policy: Option<&db::ReviewPolicyRef>,
    v2_check_repo_id: Option<&str>) -> Result<serde_json::Value>;
```

`policy=None`: devuelve `next` sin cambios y rechaza un documento v4.
`policy=Some`: convierte v1/v2/v3 a v4 con ese ref (un `required_checks` v2 exige
`v2_check_repo_id`); un v4 de entrada se reescribe con el ref vigente. v4 acepta
`review` opcional. Solo para `gate_kind=merge_readiness`; otros no cambian.
Input v4: `input_kind=review_coverage`, `input_id=repo_id`,
`input_version=coverage revision`, `input_digest=snapshot_digest`,
`producer_ref=null`, `status=ReviewCoverageStatus`,
`subject={review_policy_revision, review_policy_digest, snapshot_digest,
base_commit, candidate_commit}`. Resultado `satisfied` solo si todos los estados
están en `satisfied|not_required|unchanged`; un requisito `review` v1–v3 nunca
acepta un report con `subject.kind=commit_range`.

Códigos y HTTP (lane F en `api/src/errors.rs`):

| Código | HTTP |
|---|---|
| `review.policy.version_conflict` | 409 |
| `review.policy.invalid` | 422 |
| `review.policy.not_authorized`, `review.finding.waiver_not_authorized` | 403 |
| `review.range.nothing_to_review`, `review.range.checkpoint_active`, `review.range.head_not_integrated`, `review.range.unrecorded_commits` | 409 |
| `review.range.not_permitted` (coordinador sin permiso de política) | 403 → incluir en la lista explícita |
| `review.checkpoint.limit_reached`, `review.checkpoint.reviewer_unavailable`, `review.checkpoint.version_conflict` | 409 |
| `review.report.subject_mismatch`, `review.report.questions` | 409 |
| `review.report.invalid_findings` | 422 |
| `review.coverage.base_unknown`, `review.coverage.base_not_ancestor`, `review.coverage.facts_stale`, `review.coverage.version_conflict` | 409 |
| `review.finding.not_waivable`, `review.attribution.commit_not_pending` | 409 |
| `gate.review_coverage_missing` | 409 |
| `coordination.plan_review_checkpoint_required`, `coordination.plan_result_review_redundant` | 422 |
| `coordination.plan_review_policy_stale` | 409 |

Un `request` repetido con la misma clave devuelve el mismo checkpoint; otra clave
sobre el mismo `(base, head)` activo → `checkpoint_active`; head ya cubierto →
`nothing_to_review`; Task pausada, cancelada o sin política → conflicto existente
de lifecycle o `review.range.nothing_to_review`.

### C0.7 Severidad, hallazgos y cierre (contrato del report)

Agent: última línea `FORGE_RESULT: <json>`. `schema_version=2` obligatorio para un
WorkUnit de rango; `1` sigue siendo el único válido para resultado de WorkUnit.

```json
{
  "$schema": "https://json-schema.org/draft/2020-12/schema",
  "type": "object",
  "additionalProperties": false,
  "required": ["schema_version","kind","verdict","summary","findings","questions","resolutions"],
  "properties": {
    "schema_version": {"const": 2},
    "kind": {"const": "review"},
    "verdict": {"enum": ["pass","fail","needs_human"]},
    "summary": {"type":"string","minLength":1,"maxLength":8192},
    "criteria": {"type":"array"},
    "questions": {"type":"array","maxItems":32,"items":{"type":"string","minLength":1,"maxLength":2048}},
    "evidence_considered": {"type":"array"},
    "findings": {"type":"array","maxItems":100,"items":{
      "type":"object","additionalProperties":false,
      "required":["label","severity","title","evidence","locations"],
      "properties":{
        "label":{"type":"string","pattern":"^[A-Za-z0-9._-]{1,32}$"},
        "severity":{"enum":["P0","P1","P2","P3"]},
        "title":{"type":"string","minLength":1,"maxLength":256},
        "evidence":{"type":"string","minLength":1,"maxLength":4096},
        "invariant":{"type":["string","null"],"maxLength":512},
        "locations":{"type":"array","maxItems":16,"items":{
          "type":"object","additionalProperties":false,"required":["path"],
          "properties":{
            "path":{"type":"string","minLength":1,"maxLength":1024},
            "line_start":{"type":["integer","null"],"minimum":1},
            "line_end":{"type":["integer","null"],"minimum":1}}}}}}},
    "resolutions": {"type":"array","maxItems":500,"items":{
      "type":"object","additionalProperties":false,
      "required":["finding_id","outcome","evidence"],
      "properties":{
        "finding_id":{"type":"string","minLength":1,"maxLength":64},
        "outcome":{"enum":["fixed","not_reproduced","still_open"]},
        "evidence":{"type":"string","minLength":1,"maxLength":4096}}}}
  }
}
```

Los `maxLength` se aplican en **bytes UTF-8**. Validación Rust, todo o nada
(`collaboration_service/range_report.rs`, lane C):

```rust
pub(crate) struct RangeReportContext { pub range: db::ReviewRange,
    pub input_findings: Vec<db::ReviewFinding>, pub recheck: bool }
pub(crate) fn validate_range_report(input: &RangeReportInput, ctx: &RangeReportContext)
    -> Result<ValidatedRangeReport>;  // puro: todo lo siguiente salvo rutas/ancestry
```

1. verdict: `pass→pass`, `fail→request_changes`, `needs_human→questions`.
2. `needs_human`: `questions` no vacío; `findings` y `resolutions` vacíos →
   checkpoint `blocked`, `blocked_code=review.report.questions`.
3. En `pass`/`fail`, `questions` vacío.
4. `label` único; `line_end` exige `line_start` y `>=`; solo `line_start` ⇒ `line_end=line_start`.
5. `path` relativo, sin `..`, NUL ni `\`; existe en head o está borrado en
   `base..head`; líneas dentro del fichero en head (borrado: sin líneas).
6. `resolutions`: IDs únicos del conjunto congelado; cada P0/P1 de entrada
   aparece; `not_reproduced` solo en recheck; `fixed` exige head descendiente
   estricto del ancla.
7. `pass`: sin P0/P1 nuevos ni de entrada `still_open`.
8. `fail`: al menos un hallazgo nuevo o un `still_open`.
9. Sujeto, independencia, sesión y checkpoint vigentes, revalidados en la
   transacción → `review.report.subject_mismatch`.

`still_open` no crea fila de resolución. Un report inválido sigue el fallo técnico
de revisión existente; no escribe Artifact ni ledger. El checkpoint pasa a
`reviewed` cuando `required_reviewers` Actors distintos entregaron `pass`
o `request_changes`.

Human: `SubmitReviewReportRequest` (C0.8) se convierte a la misma estructura;
`pass|request_changes|questions` corresponden a `pass|fail|needs_human`.

Contenido del Artifact (`kind=review_report`):
`{kind, verdict, summary, criteria, questions, evidence_considered,
findings:[{finding_id, ...campos}], resolutions, subject:{task_id,
review_execution_id, workspace_id:null, base_commit_sha:null,
head_commit_sha:null, workspace_snapshot_digest:null, kind:"commit_range",
checkpoint_id, range:{id, repo_id, repository_set_revision, base_commit,
head_commit, base_tree, head_tree, range_digest}, checkout:[{repo_id,
workspace_id, commit_sha, snapshot_digest}], input_findings_digest}}`.

Canónicos: `range_digest` = `{task_id, repo_id, repository_set_revision,
base_commit, head_commit, base_tree, head_tree, integration_workspace_id,
head_integration_id}`; digest de hallazgo = `{id, report_artifact_id, severity,
title, evidence, invariant, anchor_commit, locations:[{path,line_start,line_end}]}`;
`input_findings_digest` = array ordenado por `finding_id`.

Abierto para candidato C: ninguna resolución cuyo `anchor_commit` sea ancestro o
igual de C. P0 y P1 bloquean.

### C0.8 `api-types` (lane F)

Nuevo `crates/api-types/src/review_ranges.rs`. Todos
`Debug, Clone, Serialize, Deserialize, TS, PartialEq, Eq`, `#[ts(export)]`;
requests con `#[serde(deny_unknown_fields)]`; enums
`#[serde(rename_all = "snake_case")]` salvo severidad (`P0`…`P3`); `i64/u64`
con `#[ts(type = "number")]` (`"number | null"` si `Option`); `Option` de request
con `#[serde(default)] #[ts(optional = nullable)]`.

| Tipo | Campos |
|---|---|
| `ReviewRiskClass`, `ReviewTriggerMode`, `ReviewSizeThreshold`, `ReviewPolicyDocument` | espejo de C0.5 |
| `ReviewFindingSeverity`, `ReviewCheckpointState`, `ReviewCheckpointReason`, `ReviewCoverageStatus`, `ReviewResolutionOutcome`, `ReviewAttributionKind` | espejo |
| `ReviewResolutionClaim` | enum `fixed \| not_reproduced \| still_open` |
| `TaskReviewPolicyResponse` | `task_id`, `policy: Option<TaskReviewPolicyRevisionResponse>` |
| `TaskReviewPolicyRevisionResponse` | `revision`, `digest`, `source`, `policy`, `created_by_user_id`, `created_at` |
| `SetTaskReviewPolicyRequest` | `expected_revision: Option<i64>`, `policy` |
| `ProjectReviewPolicyResponse` | `version`, `policy`, `digest` |
| `ReviewRangeResponse` | `id`, `task_id`, `repo_id`, `repository_set_revision`, `base_commit`, `head_commit`, `base_tree`, `head_tree`, `range_digest` |
| `ReviewSeverityCounts` | `p0`, `p1`, `p2`, `p3` |
| `ReviewCoverageRangeResponse` | `range_id`, `base_commit`, `head_commit`, `valid_edge`, `reports: Vec<ReviewCoverageReportRef>` (`report_artifact_id`, `actor: ActorRef`, `verdict`) |
| `RepoReviewCoverageResponse` | campos de `RepoCoverage` salvo `resolution_ids`; `ranges: Vec<ReviewCoverageRangeResponse>` |
| `TaskReviewCoverageResponse` | `task_id`, `review_policy_revision: Option<i64>`, `repositories` |
| `ReviewCheckpointResponse` | `id`, `task_id`, `repo_id`, `state`, `reasons`, `attempt`, `base_commit?`, `head_commit`, `range_id?`, `review_work_unit_ids`, `blocked_code?`, `blocked_commits: Vec<String>`, `version`, `created_at`, `updated_at` |
| `ListReviewCheckpointsResponse` | `items`, `next_cursor` |
| `CreateReviewCheckpointRequest` | `repo_id`, `reason`, `idempotency_key` |
| `RetryReviewCheckpointRequest` | `expected_version` |
| `ReviewFindingLocationResponse` | `path`, `line_start?`, `line_end?`, `attribution_kind`, `work_unit_id?`, `execution_id?`, `actor?` |
| `ReviewFindingResolutionResponse` | `id`, `outcome`, `anchor_commit`, `report_artifact_id?`, `decision_id?`, `reference?`, `actor`, `created_at` |
| `ReviewFindingResponse` | `id`, `task_id`, `repo_id`, `range_id`, `report_artifact_id`, `label`, `severity`, `title`, `evidence`, `invariant?`, `anchor_commit`, `open`, `locations`, `disposition?: TaskReviewDisposition`, `correction_work_unit_ids`, `resolutions`, `created_at` |
| `ListReviewFindingsResponse` | `items`, `next_cursor` |
| `WaiveReviewFindingRequest` | `expected_coverage_revision`, `rationale`, `reference` |
| `RebaselineReviewRequest` | `repo_id`, `expected_coverage_revision`, `rationale` |
| `ReviewBaseResponse` | `task_id`, `repo_id`, `revision`, `base_commit`, `source`, `decision_id?`, `pinned_at` |
| `ReviewCommitAttributionInput` | `commit_sha`, `actor: ActorRef` |
| `AttributeReviewCommitsRequest` | `expected_version`, `attributions`, `rationale` |
| `StructuredReviewLocation` / `StructuredReviewFinding` / `ReviewResolutionInput` | campos del schema C0.7 |
| `PlanReviewCheckpoint` | `key`, `after_work_unit_keys: Vec<String>`, `repository_ids: Vec<String>` (default, omitido si vacío), `reason` |

Extensiones:

- `SubmitReviewReportRequest`: `structured_findings: Option<Vec<StructuredReviewFinding>>`,
  `resolutions: Option<Vec<ReviewResolutionInput>>` (ambos
  `default`, `skip_serializing_if = "Option::is_none"`). Rango: ambos presentes y
  `findings` vacío; resultado de WorkUnit: ambos ausentes. Si no,
  `review.report.invalid_findings`.
- Respuesta de WorkUnit (donde hoy está `review_subject`):
  `review_range: Option<WorkUnitReviewRangeResponse>` =
  `{review_work_unit_id, checkpoint_id, range: ReviewRangeResponse,
  input_finding_ids}` omitido si ausente.
- `UpdateProjectRequest.default_review_policy: Option<Option<ReviewPolicyDocument>>`
  (mismo patrón que `primary_repo_id`); `ProjectResponse.default_review_policy:
  Option<ProjectReviewPolicyResponse>`.
- `TaskReviewDisposition` añade `Defer` (**cambio de enum público**, CHANGELOG).
- `ProposalTargetKind` añade `ReviewFinding`, `ReviewBase`, `ReviewCommit`; la
  ruta genérica de proposals/decisions rechaza crearlos o decidirlos (403).
- `TaskCoordinationPlanResponse.review_checkpoints: Vec<PlanReviewCheckpoint>`
  (omitido si vacío).

### C0.9 REST, MCP, eventos

REST bajo `/api/v1`, `Authorization: Bearer`, listas con `cursor`/`limit` e
`items`:

| Método y ruta | Request → Response | Autoridad / errores |
|---|---|---|
| `GET /tasks/{id}/review-policy` | → `TaskReviewPolicyResponse` | lectura de Task |
| `PUT /tasks/{id}/review-policy` | `SetTaskReviewPolicyRequest` → ídem | owner/admin; `version_conflict`, `invalid`, `not_authorized` |
| `PATCH /projects/{id}` | `default_review_policy` | owner/admin |
| `GET /tasks/{id}/review-coverage` | → `TaskReviewCoverageResponse` | lectura |
| `GET /tasks/{id}/review-checkpoints?repo_id` | → `ListReviewCheckpointsResponse` | lectura |
| `POST /tasks/{id}/review-checkpoints` | `CreateReviewCheckpointRequest` → `ReviewCheckpointResponse` | Human con escritura de Task; `review.range.*`, `limit_reached` |
| `POST /tasks/{id}/review-checkpoints/{checkpoint_id}/retry` | `RetryReviewCheckpointRequest` → `ReviewCheckpointResponse` | owner/admin |
| `POST /tasks/{id}/review-checkpoints/{checkpoint_id}/attributions` | `AttributeReviewCommitsRequest` → `ReviewCheckpointResponse` | owner/admin; `commit_not_pending` |
| `GET /tasks/{id}/review-findings?repo_id&severity&open` | → `ListReviewFindingsResponse` | lectura |
| `POST /tasks/{id}/review-findings/{finding_id}/waiver` | `WaiveReviewFindingRequest` → `ReviewFindingResponse` | owner/admin; `version_conflict`, `not_waivable`, `waiver_not_authorized` |
| `POST /tasks/{id}/review-base` | `RebaselineReviewRequest` → `ReviewBaseResponse` | owner/admin; `review.coverage.*` |

`open` se evalúa contra el candidato vigente. Sin forge-ctl (DR12).

MCP (`additionalProperties:false`, todos los listados obligatorios salvo `?`):

| Tool | Schema | Acceso |
|---|---|---|
| `forge_get_review_coverage` | `{task_id: string}` | lectura de Task |
| `forge_set_task_review_policy` | `{task_id: string, expected_revision?: integer\|null, policy: object}` | usuario owner/admin; nunca Agent ni Project Copilot |
| `forge_request_range_review` | `{task_id: string, repo_id: string, reason: string(≤1024), idempotency_key: string}` | usuario Human con escritura de Task |
| `forge_waive_review_finding` | `{task_id: string, finding_id: string, expected_coverage_revision: integer, rationale: string, reference: string}` | usuario owner/admin; nunca Agent ni Project Copilot |
| `forge_submit_work_unit_review` | `report` añade `structured_findings?: array`, `resolutions?: array` con el schema C0.7 | sin cambio |

Eventos de dominio (`scope_type='task'`) y SSE (`PublicEventType`, variantes
`TaskReviewPolicyUpdated`, `ReviewCheckpointCreated`,
`ReviewCheckpointStateChanged`, `ReviewRangeReported`, `ReviewFindingsTriaged`,
`ReviewFindingResolved`, `ReviewCoverageChanged`). Payload cerrado: sin texto,
rutas, SHAs ni diffs.

| `event_type` | `entity_type` / id | payload |
|---|---|---|
| `task.review_policy_updated` | `task` / task | `task_id, revision, risk_class, coverage_required` |
| `review.checkpoint_created` | `review_checkpoint` / id | `task_id, repo_id, checkpoint_id, state, reasons, attempt` |
| `review.checkpoint_state_changed` | `review_checkpoint` / id | `task_id, repo_id, checkpoint_id, from_state, state, version, blocked_code` |
| `review.range_reported` | `artifact` / report | `task_id, repo_id, checkpoint_id, range_id, report_artifact_id, verdict, p0, p1, p2, p3` |
| `review.findings_triaged` | `artifact` / report | `task_id, report_artifact_id, request_id, correct, recheck, human, defer` |
| `review.finding_resolved` | `review_finding` / id | `task_id, repo_id, finding_id, severity, outcome, resolution_id` |
| `review.coverage_changed` | `task` / task | `task_id, repo_id, revision, status` |

`review.coverage_changed` lo emite el scheduler tras reconciliar
(`dedupe_key=review-coverage:{task}:{repo}:{revision}`); los demás, el writer que
produce el hecho.

### C0.10 Coordinador (lane D)

Plan (`WorkUnitPlan`, `task_coordination.rs`), ambos campos omitidos si vacíos:

```rust
#[serde(default, skip_serializing_if = "Vec::is_empty")]
pub review_checkpoints: Vec<api_types::PlanReviewCheckpoint>,   // máx. 8
#[serde(default, skip_serializing_if = "Option::is_none")]
pub review_policy_digest: Option<String>,                        // 64 hex
```

`key` slug 1–64 `[a-z0-9_-]` único; `after_work_unit_keys` 1–32, existentes, con
`requires_integration=true`; `repository_ids` ⊆ repos escritos por ellos (vacío =
todos); `reason` 1–1024 bytes. `review_policy_digest` obligatorio e igual al
vigente al materializar si la Task tiene política; prohibidos ambos campos sin
política (`coordination.invalid_plan`). Con `coverage_required=true`,
`review_subject_key` hacia un WorkUnit `requires_integration=true` →
`coordination.plan_result_review_redundant` (DR11). R2 con al menos una arista
`depends_on` entre dos WorkUnits con integración y sin checkpoints →
`coordination.plan_review_checkpoint_required`. Los checkpoints R2 no añaden
dependencias (DR6). Replan: quitar o cambiar un checkpoint activo no cumplido es
`scope_or_authority_change`; añadir no.

Salida nueva, en `coordinator_outputs/range_review.rs`:

```rust
pub struct RequestRangeReviewOutput { pub repo_id: String, pub reason: String } // deny_unknown_fields; reason 1..=1024 bytes
CoordinatorOutput::RequestRangeReview(RequestRangeReviewOutput)   // "type":"request_range_review"
CoordinatorOutputKind::RequestRangeReview                         // "request_range_review"
```

Solo en turnos tipados, solo si figura en `allowed_actions`, cuenta para el máximo
de 4; además exige `coordinator_may_request` (`review.range.not_permitted`).
Idempotency key: `coordinator:{request_id}:{ordinal}`.

Triage de rango, `schema_version: 3` (1 y 2 intactas; una fuente de rango exige 3
y una de resultado lo rechaza):

```rust
struct RangeReviewDecisionV3 { schema_version: u32, request_id: String, task_id: String,
    authorization_version: i64, range_review: RangeReviewTriage,
    #[serde(default)] outputs: Vec<serde_json::Value> }
struct RangeReviewTriage { review_report_artifact_id: String, report_digest: String,
    findings: Vec<RangeFindingDecision> }
struct RangeFindingDecision { finding_id: String, disposition: api_types::TaskReviewDisposition,
    rationale: String /*1..=2048*/, #[serde(default)] instructions: String /*<=8192*/ }
```

Todas `deny_unknown_fields`; máximo 64 KiB; sin `micro_fix` (DR10,
`coordination.output_not_permitted`). Conjunto exigido = hallazgos nuevos del
report más entradas `still_open`: cada P0/P1 exactamente una vez con
`correct|recheck|human`; P2/P3 opcionales y admiten `defer`; `correct` exige
`instructions`. Disposición agregada: `human` > `correct` > `recheck` > `defer`;
si no hay entradas, `defer`. `rationale` agregada = la del primer hallazgo (por
ordinal) con la disposición ganadora; `instructions` agregada vacía;
`decision_digest` sobre `RangeReviewTriage` canónico. Contexto del turno
(`exact_review_findings_context`): rango, hallazgos, ubicaciones con atribución,
cobertura por repo. Prompt: lo de «Cambios al coordinador».

Correcciones: un WorkUnit nuevo por `(repo, WorkUnit autor)`, encadenados por
dependencia dentro del repo en orden de integración; mismo Actor/TaskRole si
elegibles; si no, TaskRole implementador sin Actor o `human`. Base = HEAD de
integración vigente; sin `work_unit_result_requirement`. Preparar su workspace
desde `review_correction_link.base_commit` o el HEAD vigente si avanzó (lane C).

### C0.11 Lanes: checklist, filtros y orden

Regla: una variante o campo nuevo que rompa compilación en un fichero ajeno se
resuelve en el PR que la introduce con el brazo mínimo que falle cerrado, avisando
a la lane dueña. `services/src/lib.rs` es de B; E y C declaran submódulos desde
sus propios ficheros. Todo test nuevo empieza por `review_range_`.

**Lane A — persistencia** (C0.3–C0.5)
- [ ] V184, V185, comentario en `migration.rs`.
- [ ] Modelos, traits, `sqlite/review_range.rs`, enums nuevos, `ProposalTargetKind`, `defer`.
- [ ] `review_report` writer atómico; teardown de Project; lectura de `review_coverage` en el repo de Gate.
- [ ] Copia del default de Project en la transacción de creación de Task.
- [ ] Tests: migración desde V183 preservando Gate/triage/coordinación, índices, inmutabilidad, scope, exclusividad, independencia y sesión, resultado sin regresión, P0, bumps, fail closed, teardown, integraciones registradas en repo único.
- CI: `cargo test --locked -p db --lib review_range_` (registrar `tests/review_ranges.rs` en `tests.rs`) y `cargo test --locked -p db --test task_coordination_schema review_range_`.

**Lane B — núcleo** (C0.6)
- [ ] B0: módulos y firmas compilables + funciones git. Después policy, coverage (evaluador puro), scheduler + escáner, attribution, resolution; enganche del scheduler al bucle de eventos desde ficheros propios (si requiere `api`, petición a F).
- [ ] `set` de política usa el helper de E y revisa el Gate atómicamente.
- CI: `cargo test --locked -p git --lib review_range_`; `cargo test --locked -p services --lib review_range_`.

**Lane C — ejecución y report** (C0.7)
- [ ] Parser v2, `validate_range_report`, ingestión Agent y Human, atribución al ingerir.
- [ ] Workspace group de solo lectura, sin lease, verificación de no escritura; admisión/dispatch; asignación `auto`/`manual`; prompt (base, head, diffstat, contribuyentes, hallazgos, R3).
- [ ] Sin intents de rework para rango; Proposals de revisión rechazadas por la vía genérica.
- CI: `cargo test --locked -p services --lib review_range_`; `cargo test --locked -p api --test work_unit_reviews review_range_`.

**Lane D — coordinación y rework** (C0.10)
- [ ] Plan, materialización de `review_plan_checkpoint`, replan, prompt.
- [ ] `request_range_review`, triage v3, correcciones, `request_rework` manual con report de rango, conteo de rondas combinado, sin micro-fix.
- [ ] `micro_fix.rs` crea Gates solo vía helper de E.
- CI: `cargo test --locked -p services --test task_coordination review_range_`.

**Lane E — Gate y merge**
- [ ] E0: tipos v4 + helper. Después evaluador, inputs, fail closed en Rust, todos los generadores de Gates, recálculo bajo claim de integración, pre-push (publicación y rework), batch multirepo.
- [ ] Igualdad report=candidato solo para `review_report`; head remoto distinto visible.
- [ ] Test: un merge observado externamente se sigue registrando; si la vigencia existente lo impide, escalar (no debilitar guards).
- CI: `cargo test --locked -p api --test gate_engine_api review_range_`; `cargo test --locked -p api --test coordinator_verification_pr_loop review_range_`.

**Lane F — superficies** (C0.8, C0.9)
- [ ] F1: `api-types`, tipos TS, `PublicEventType` (sin rutas ni tools de escritura).
- [ ] F2: rutas, `errors.rs`, MCP, web, SSE; docs de CLAUDE.md afectadas, `api.md`, CHANGELOG **Added**/**Changed**; filtros en `build-local-linux.yml` y `-p git` en clippy.
- CI: `cargo test --locked -p api --test happy_path review_range_`; `cargo test --locked -p mcp-server --lib review_range_`.

Todos los pasos de CI llevan `if: ${{ !cancelled() }}`.

**Orden de merge** en `feat/review-ranges`:

1. C0 (este documento).
2. A.
3. F1.
4. B0.
5. E0.
6. B, C y E en paralelo; entran en el orden B → E → C.
7. D.
8. F2 (primera exposición de escritura de políticas).
9. C3: CI completa y E2E real multirepo. Entrega 0.9.3 (DR14).

### C0.12 Cuestiones abiertas

1. **Decisiones Human por checkpoint bloqueado (R4).** Se representa como estado
   `blocked` con rutas dedicadas, sin entrada en la bandeja
   `human_decision_request`. Incluirlo allí exige reconstruir esa tabla; confirmar
   que no es necesario en v1.
2. **Merge externo con cobertura obsoleta.** Los guards de vigencia existentes
   podrían impedir registrar el cierre de una PR ya mergeada en GitHub si la
   cobertura cambia después de publicar. La lane E lo prueba; relajar un guard
   requiere decisión.
3. **Waiver: autoridad.** DR8 dice «Human con referencia»; se congela owner/admin,
   como en el diseño. Confirmar si un miembro sin ese rol debe poder.
