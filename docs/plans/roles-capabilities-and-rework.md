# Plan A — Roles configurables, capacidades y devolución de correcciones

Estado: diseño propuesto para implementación por un agente independiente.
Fecha: 2026-11-06. Baseline inspeccionado: `3d7621b6`.
Este documento no cambia el contrato vigente ni autoriza lanzar tareas reales.

## Encargo y resultado

Eliminar la dependencia de nombres especiales como `reviewer` y `orchestrator`.
La UI ofrecerá plantillas y roles personalizados con capacidades visibles. Un
rol llamado «Revisor técnico» podrá producir revisión formal y, si está
configurado, devolver hallazgos a un rol de implementación. Todo ello deberá
conservar autoridad exacta, independencia, historial y recuperación.

Leer AGENTS.md, docs/architecture.md, docs/migration/architecture-v2.md,
docs/concepts/roles.md, work-units.md, review-and-validation.md y orchestration.md.
Invariantes afectados: INV-004–006, INV-008–010, INV-016–018, INV-022,
INV-028–030, INV-033–036 e INV-038. No reintroducir Execution purpose.

## Problema comprobado

- El formulario de TaskRole acepta un nombre libre y coordinación. No explica
  que ciertas cadenas activan funciones especiales.
- Los servicios, Gates, MCP, UI y triggers SQLite comparan nombres literales.
- `revisor` y `orquestador` son roles personalizados sin las capacidades de
  `reviewer` y `orchestrator`.
- Terminar una revisión que pide cambios satisface una dependencia de
  finalización. Esa dependencia no representa aprobación.
- Hay una operación explícita de rework; emitir un informe no la ejecuta solo.

## Decisiones de producto

1. Conservar los identificadores y nombres persistidos actuales. Añadir una
   etiqueta de presentación opcional; no renombrar ni traducir IDs históricos.
2. Separar nombre, capacidades, coordinación y política de devolución. No
   introducir una clase exclusiva de rol: un rol puede combinar capacidades.
3. Ofrecer plantillas Planificador, Implementador, Revisor y Orquestador, y
   «Personalizado». Son valores iniciales editables, no autoridad por nombre.
   Personalizado comienza sin capacidades privilegiadas; mostrar sus límites.
4. `RoleMembership` sigue definiendo elegibilidad; WorkUnit define alcance;
   Execution, HarnessSession y WorkspaceLease conservan sus controles propios.
   Una capacidad nunca concede acceso general al repositorio ni a otros Actors.
5. Toda devolución usa un informe exacto `request_changes`; crea corrección y
   nueva revisión. No reabre ni reescribe WorkUnits anteriores.
6. Modo predeterminado: devolución manual. Automatización requiere una política
   explícita y persistida, visible antes de lanzar la Task.
7. La entrega final debe poder exigir aprobación de revisión, no solo que el
   revisor termine. Esa exigencia debe sobrevivir a nuevas rondas de corrección.

## Contrato de capacidades

Añadir configuración tipada y versionada al TaskRole. Catálogo inicial cerrado:

| Capacidad propuesta | Efecto permitido |
|---|---|
| `formal_review` | Actuar como productor de ReviewReport sobre un sujeto exacto |
| `orchestration` | Participar en las operaciones tipadas actuales del Orchestrator |
| `request_rework` | Solicitar corrección a destinos expresamente autorizados |

El trabajo ordinario y planificar son WorkUnits cognitivos normales: no necesitan
un permiso ficticio `can_work`. La plantilla Planificador sugiere instrucciones;
Implementador permite trabajo ordinario; Revisor activa revisión formal y puede
solicitar rework solo con destinos configurados; Orquestador activa orquestación
con las restricciones actuales de acciones. No conceder poderes nuevos por usar
una plantilla. Exponer el catálogo por API; la web no mantiene otro catálogo
independiente. La política de acciones de orquestación sigue restringiendo las
acciones ya soportadas; esta iniciativa no agrega merge ni asignación autónoma
ilimitada al Orchestrator.

La configuración guarda también destinos permitidos por ID de TaskRole, modo
manual/automático y selección explícita del rol revisor de la siguiente ronda.
Las referencias de defaults de Proyecto se resuelven a los nuevos TaskRole al
crear la Task; nunca copiar IDs de roles de otra Task.

Solo la autoridad humana vigente que administra responsabilidades de la Task o
los defaults del Proyecto puede ampliar capacidades o activar automatización.
Un agente puede proponer cambios; no puede concedérselos ni cambiar destinos
para ampliar su alcance. Esta restricción se comprueba en servicios y MCP.

## Admisión, revocación e historial

- Usar una fuente tipada de capacidades en todos los servicios y en SQLite;
  inventariar cada comparación literal antes de sustituirla. Nada de aliases
  `revisor -> reviewer` ni fallback al nombre una vez completada la migración.
- Guardar en la admisión de Execution la referencia al TaskRole, su versión y
  las capacidades relevantes efectivas. Será evidencia inmutable de autoridad
  en ese intento, no un nuevo Purpose ni autoridad reutilizable.
- Admisión y Provider Start revalidan configuración, membership y WorkUnit
  vigentes transaccionalmente. Una revocación antes del Start impide arrancar.
- Revocar no borra evidencia válida previa. El Gate comprueba la procedencia
  exacta admitida al producir el informe, no infiere autoridad histórica de una
  configuración actual. La política vigente del Gate aún decide qué acepta.
- Un intento en curso no puede adquirir capacidades nuevas por una edición del
  rol. Nuevas acciones mutantes requieren tanto autoridad admitida como actual.
  Un informe de un intento admitido antes de revocar puede conservarse como
  evidencia; no puede provocar nueva devolución sin autoridad actual válida.
- Rechazar configuraciones/capacidades desconocidas y devolver conflictos 409
  con versiones esperadas; REST y MCP comparten el mismo servicio.

## Devolución determinista y acotada

La política requiere rol destino, rol de nueva revisión y selección de Actor:
Actor explícito, o el contribuidor original únicamente si sigue siendo miembro
activo elegible del destino. Si hay varios contribuidores y no se indicó uno,
no seleccionar al azar. Revisor distinto de todos los contribuidores relevantes.
Si falta Actor, membership, capacidad o presupuesto: registrar bloqueo durable
explicable y pedir decisión humana, sin consumir una ronda ni crear medio par.

La corrección consume el resultado y hallazgos exactos del informe; no inventa
requisitos. Su alcance se prepara en la solicitud manual o mediante una regla
acotada que referencia todos los hallazgos del informe y el alcance original.
Si exige un cambio de alcance o decisión de negocio, se detiene para decisión
humana. No tratar `questions`, fallo técnico o texto libre como request_changes.

Persistir cada solicitud automática, versión de política, sujeto, informe y
resultado. Clave única derivada de política + informe + sujeto; procesar eventos
con replay seguro. Concurrencia, reintentos SSE/eventos y reinicio no duplican
C1/R2 ni presupuesto. La misma identidad con distinto contenido da conflicto.
Conservar el límite actual de ocho rondas por Task; esta iniciativa no lo eleva.

Creación de intención y del par de WorkUnits es atómica. Dispatcher conserva
arranques y controles de pausa/capacidad. Mientras esté pausada una Task se puede
registrar la intención, pero la materialización automática queda pendiente hasta
resume. En cancelación no se crea trabajo nuevo. Al reanudar, volver a validar
política y autoridad; una edición no reproduce un informe ya consumido.
Cambiar manual a automático no procesa retrospectivamente informes antiguos sin
una solicitud humana explícita.

## Aprobación y múltiples entregables

No confundir una dependencia de trabajo completado con aprobación. Incorporar
un requisito tipado de aprobación anclado a una línea durable de revisión:
fuente original, revisiones exactas y correcciones sucesivas. Debe bloquear al
consumidor hasta un informe favorable válido sobre el último resultado de esa
línea. Un pass antiguo no aprueba una corrección posterior; un Gate satisfecho
sobre entradas antiguas no se reutiliza para una nueva entrega.

Reutilizar Gate y sus entradas inmutables. Definir el puente desde el requisito
pendiente hasta una evaluación exacta vigente; no convertir Gate en ejecutor ni
hacer de un campo booleano otra autoridad. Su comprobación se repite dentro de
la admisión del WorkUnit consumidor para cerrar la carrera con un nuevo rework.

Cada sujeto formal sigue apuntando a un WorkUnit exacto. Para revisar D1–D5:
crear un WorkUnit de paquete de revisión antes del revisor, que consuma los
Artifacts exactos de D1–D5 y produzca el paquete inmutable; la entrega al PO es
posterior y exige aprobación de esa línea. No usar como sujeto solo D5 ni ampliar
silenciosamente el esquema actual a una revisión multifuente.

El paquete debe fijar un manifiesto inmutable de IDs/digests de todos sus
Artifacts fuente y del Artifact de entrega, con procedencia de sus Executions.
No basta una lista de enlaces editables en el prompt. Extender la selección del
Artifact resultado a entregables ordinarios (hoy no basta el tratamiento especial
de ReviewReport), comprobando producción, Task y digest en la transacción de
aceptación. Una corrección produce otro manifiesto; no reemplaza el anterior.
La revisión y el Gate fijan ese manifiesto y el intento exacto.

## Migración preservadora

Añadir migraciones después de la última existente al comenzar (V148 en este
baseline). No modificar V001–V148. Backfill:

- Los roles que ya tenían semántica literal `reviewer`/`orchestrator` reciben las
  capacidades equivalentes, con procedencia de migración comprobable.
- Los personalizados conservan nombres, asignaciones e historia; no se adivinan
  capacidades por idioma. UI explica qué falta y permite configuración humana.
- Recuperar autoridad histórica solo desde hechos que la demuestren. Los casos
  ambiguos se mantienen como historia sin inventar certificación de revisión.
- Defaults de Proyecto, TaskRoles, snapshots, triggers, mappers y teardown deben
  evolucionar juntos. Plantilla no implica configuración automática en Tasks
  existentes. Preservar IDs de Executions, Artifacts, sujetos y leases.

## Secuencia de implementación y superficies

1. Inventario de cadenas especiales en DB/triggers, services, REST, MCP, CLI,
   defaults, web y tests. Especificar DTOs y transacciones; actualizar ADR y
   contrato conceptual como diseño antes de tocar admisión.
2. Persistencia y migración; servicio de configuración y catálogo; snapshots y
   guards; sustituir lecturas por nombre en todas las fronteras.
3. Revisión formal genérica, solicitud manual de devolución y aprobación de la
   línea exacta. Mantener el recorrido manual completo funcional primero.
4. Consumidor durable de devolución automática, pausa, replay y recuperación.
5. UI de plantillas/capacidades/defaults, destinos y aprobación. Explicar por
   qué una acción no está disponible y mostrar la próxima intervención humana.
6. Pruebas integradas y documentación de API/CLI/MCP/SSE y CHANGELOG Unreleased.
   Generar TypeScript desde Rust; no editar tipos para simular capacidades.

REST debe ofrecer catálogo y lectura/escritura versionada de configuración,
además del requisito de aprobación. Revisar cuerpos de creación/asignación de
roles, defaults de Proyecto, RequestRework y respuestas de readiness. Retirar
parámetros basados en nombres mágicos de forma explícita en CHANGELOG; no
introducir compatibilidad silenciosa. Mantener CLI/MCP alineados y documentar
operaciones que la CLI todavía no pueda preparar.

## Propiedad de archivos y ejecución paralela con el agente B

Este agente posee Rust/SQL, DTOs y tipos generados, hooks/query keys/SSE y tests
de contratos; documentación de arquitectura/API/conceptos/migración/changelog;
TaskDetailPage y sus tests; task-work-management, task-rework-panel,
work-unit-review-panel y sus tests; formularios de defaults de roles en
components/settings. Extraer nuevos formularios a `web/src/features/task-roles/`.

El agente B posee componentes UI genéricos, tokens, shell/layout y el resto de
páginas. No editar ProjectSettingsPage como contenedor: integrar el formulario
mediante el componente de settings ya existente. Si hace falta un punto nuevo
de montaje, comunicar el contrato y reservar una integración serial al final.
No editar index.css, primitives, package.json, lockfile ni Playwright config.
Añadir tests de roles en un spec nuevo, sin reescribir la configuración global.

Ambos trabajan en ramas/worktrees separados desde el mismo baseline. Este plan
es autocontenido; no necesita que B termine antes. Usar las APIs actuales de los
componentes UI y el contrato visual de B: separación 16/24 px, campos con etiqueta,
acciones que envuelven, `min-w-0`, errores junto al campo y controles accesibles.
B puede mejorar los componentes compartidos preservando sus APIs.

## Aceptación obligatoria

- Rol personalizado «Revisor de planificación» emite revisión formal sin ningún
  nombre mágico; rol llamado `reviewer` sin capacidad no puede hacerlo.
- Plantillas y personalización funcionan para Humanos y Agentes; defaults se
  copian con referencias locales y no alteran Tasks previas.
- Mismo Actor puede pertenecer a varios roles pero no revisar su contribución.
- Replay tras reinicio y consumidores concurrentes crean exactamente un C1/R2;
  cambio de contenido da 409; ocho rondas siguen siendo el máximo.
- Revocación y reasignación entre preflight/admisión/Start cierran las carreras.
- Pausa, cancelación, falta de Actor e independencia producen estados visibles.
- request_changes no desbloquea entrega; pass sobre resultado viejo tampoco.
- Migración con fixtures V148 preserva conteos/IDs/relaciones, informes válidos y
  datos ambiguos sin promoverlos a autoridad. Gate conserva evidencia histórica.
- Escenario FirePOS en datos desechables: D1–D5 -> paquete -> revisión negativa
  -> corrección -> revisión favorable -> entrega al PO, solo Artifacts.

Checks: cargo test -p db; suites focalizadas de services, MCP y API (incluido
happy_path); cargo fmt --all -- --check; clippy proporcional a crates tocados;
pnpm lint/typecheck/test/build; E2E de roles y devolución manual/automática.
Finalmente ejecutar CI de repositorio y registrar PASS/FAIL/BLOCKED/NOT RUN.
No sustituir aceptación con proveedores reales por fixtures locales.

## Entrega y prompt para el agente

Entregar implementación, migraciones, contratos, pruebas y ledger reproducible;
identificar conflictos con B. No modificar la Task FirePOS real, lanzar agentes,
mergear, publicar ni reiniciar la instancia compartida sin autorización.

> Implementa docs/plans/roles-capabilities-and-rework.md sobre su baseline,
> preservando datos e invariantes. Eres propietario exclusivo del dominio y de
> los formularios funcionales de roles/review/rework. Respeta el reparto con el
> plan B. Completa el recorrido manual, luego el automático; documenta contratos
> y verifica carreras, migración e independencia. No hagas cambios reales en
> FirePOS ni despliegues. Entrega diff y evidencia de aceptación.

## Registro de implementación — 2026-11-06

Implementación local sobre la base V148: capacidades tipadas y snapshots de
autoridad por Execution; configuración y defaults de TaskRole; revisión formal,
rework manual/automático durable, línea de revisión con Gate sobre el resultado
exacto y manifiestos de Artifact; superficies REST/MCP/CLI/web, tipos generados,
documentación y CHANGELOG. Migraciones V149–V156 son forward-only.

La revisión round 3 también resuelve los tres pendientes registrados en
`docs/audits/2026-11-06-plans-a-b-review-round2.md`:

- V156 enlaza la intención automática al ID del recibo manual exacto. El worker
  reconcilia primero un recibo ya durable y compara Task, informe, WorkUnit,
  versión, par y ronda; la clave manual distinta se representa como tal sin
  relajar los guards del sujeto.
- La comprobación de pause/cancel permanece en `BEGIN IMMEDIATE`. Pause
  concurrente queda `pending` y se reintenta al resumir; cancelación tiene error
  y estado terminal propios; una decisión humana inválida sigue siendo
  `blocked`.
- Escape de opción portal o disparador cierra el Select y no propaga al Dialog.

### Validación local

- PASS: `cargo test -p services task_rework::tests` (2 tests).
- PASS: `cargo test -p api --test task_rework
  automatic_rework_replays_once_and_waits_for_task_resume` (1 test; incluye la
  intervención manual con otra clave y migración V156).
- PASS: `cargo test -p db --lib
  v148_facts_and_legacy_task_roles_survive_role_capability_migrations` (1 test,
  preservación V148→V156).
- PASS: `pnpm test` (148 tests, 29 archivos), `pnpm lint`, `pnpm typecheck`,
  `pnpm build`, `cargo fmt --all -- --check`, `git diff --check` y
  `FORGE_SKIP_WEB_BUILD=1 cargo clippy --workspace --all-targets -- -D warnings`.
- PARCIAL: `cargo test -p db` completó 71/71 tests unitarios y se detuvo en la
  fase de migración file-backed por su duración: 1162.80 s para los unit tests;
  no se observó un fallo antes de detenerlo. La prueba focalizada de preservación
  V148→V156 sí pasó independientemente.
- NOT RUN: suite MCP completa, CI completa, E2E Chromium después de la corrección
  y aceptación con proveedores/harness reales. Un intento previo de Playwright
  no pudo arrancar Vite por `Failed to create stream fd: Operation not permitted`.

No se modificó la Task FirePOS, ni se desplegó, publicó, integró o reinició el
servidor compartido. El cierre visual conjunto AB-01 del audit de UI sigue
pendiente y no forma parte de estos tres arreglos.
