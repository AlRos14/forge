# Plan de implementación — Coordinación de tareas y experiencia guiada

Fecha: 2026-11-07. Baseline inicial: `fd599647`. Baseline de PR0: `ae3b2287`;
migración última verificada: V157.
Estado: PR0 (contrato transversal) completado; PR1–PR3 implementados, PR4 con
el circuito de calidad y rework exacto implementado y límites por ruta/ping-pong
pendientes, y el circuito de intervención de PR5 disponible; PR6 pendiente. No
hay despliegue, aceptación funcional ni autorización para lanzar FirePOS. El
propietario ha aprobado las responsabilidades y el recorrido del
[modelo operativo objetivo](../concepts/task-operating-model.md). Las
decisiones de esquema y endpoints están fijadas en el
[contrato PR0](pr0-task-coordination-contract.md) y el
[ADR 0016](../adr/0016-task-coordination-turn.md); los PR dependientes deben
implementarlas sin reintroducir fuentes de verdad paralelas.

## Revisión del sistema actual

La revisión cubre las fronteras del recorrido completo. Es análisis de contrato
y código, no una nueva aceptación funcional o visual en navegador.

| Frontera actual | Evidencia / entrada de código | Fricción o hueco que resolver |
|---|---|---|
| Equipo/defaults | ProjectWorkersTab, project_task_role_defaults, task_role_configuration | Se configura desde primitives; plantillas/alta al final; selección con IDs |
| Alta de tarea | Board/listado, TaskService y copia de defaults | Objetivo no activa claramente preparación coordinada |
| Lanzamiento | TaskLifecycleService::launch_preflight, TaskDetailPage | no_work_units bloquea; UI conduce a edición manual antes de lanzar |
| Orquestación | orchestrator_runtime, task_service/orchestrator, wakes e intentos | Hay cognición por eventos y creación acotada de WU; falta contrato de arranque orientado a objetivo y ciclo operativo completo |
| Worker/dispatch | WorkUnitService, task_dispatcher, execution/runner | Autoridad exacta útil; preparación/asignación técnica expuestas como carga humana |
| Copilot | project_copilot, copilot_mcp, ProjectCopilotDock | Gestión de proyecto y preparación WU ya existen; acceso a tareas activas necesita circuito único de intervención |
| Colaboración | collaboration_service, Message/Handoff/Proposal/Decision | Mensaje y handoff no materializan automáticamente trabajo para el destinatario |
| Calidad/rework | task_rework, review lines y V149–V156 | Corrección exacta de entrega ya existe; falta distinguir defecto del plan, especialista y decisión humana |
| Entrega/evidencia | Artifacts, manifests, ValidationRun, GateEngine | Reutilizar exactitud; finalización técnica no debe convertirse en aprobación tácita |
| Lifecycle/recuperación | task_lifecycle, recovery, shutdown, provider receipts | Ampliar los mismos controles a preparación y coordinación; no duplicar schedulers |
| UI/contratos públicos | api-types, REST, MCP, CLI, hooks y SSE | Explicar acciones y bloqueos reales en el mismo modelo, no crear una UX que prometa acciones ausentes |

El Orchestrator actual no debe describirse como inexistente ni como supervisor
completo. Se reutilizan sus hechos durables y acciones existentes, ampliándolos
con contratos explícitos. Tampoco se presenta el análisis como «solo un ajuste
visual»: hay cambios de admisión, delegación, colaboración y recuperación.

## Alcance y decisiones

Un único programa integra backend y UX. No repetir la separación A/B sin un
recorrido vertical compartido. Primero cerrar un circuito mínimo usable; después
extender especializaciones. Mantener roles/capacidades y defaults existentes;
no inventar un motor de workflow general ni un catálogo nuevo de clases de Agent.

La dirección preferida para romper el arranque circular es un turno de gestión
de Task admitido desde el equipo autorizado, basado en wakes/intentos actuales.
Separarlo de las Executions de trabajadores. Evaluar cómo reutilizar el harness
plumbing de Copilot sin copiar su autoridad de Proyecto. El turno Task queda
limitado a esa Task y a sus acciones permitidas. El modelo de datos concreto y
la transición desde las actuales Executions de orquestación se fijan en PR0.

La alternativa de un WorkUnit de coordinación solo sería válida si representa
trabajo real y su resultado, nunca una fila ficticia para satisfacer un guard.
PR0 debe comparar ambas opciones con admisión, recuperación, provenance y coste
de migración; justificar cualquier cambio a la dirección preferida antes de
implementar. No mantener ambos mecanismos activos como dos autoridades.

## PR0 — Contrato transversal y conservación

- Leer Architecture y V2 completos; enumerar invariantes afectados, en especial
  Actor/Role, Execution, WorkUnit, HarnessSession, revisión, Gate, autoridad y
  ausencia de fuentes de verdad paralelas. Declarar cómo se cumple cada uno.
- Completar inventario de callers, DTOs, comandos MCP/CLI, SSE, snapshots,
  JSONL, triggers, cleanup/teardown y recuperación de los mecanismos tocados.
- Fijar esquema de autorización de tarea y vínculo a versión del equipo; acciones
  que puede preparar/ejecutar cada modo. No conceder administración de roles al
  agente ni autoridad de worker desde un turno de gestión.
- ADR del arranque de coordinación: reutilización de wakes, identidad de turno,
  harness, receipt de Start, delegación y scopes; continuidad y proveedor incierto.
- Diseñar transición conservadora para wakes/Executions actuales, incluidos
  intentos activos y pendientes. No borrar ni reetiquetar evidencia histórica;
  no arrancar automáticamente trabajo previo al cambio.
- Definir acciones/proyecciones de UX y ejemplos de request/response por estado.
  No asignar números de migración hasta verificar el último instalado.

Salida: ADR, matriz de autoridad, contratos versionados y bocetos de recorrido.
Gate de diseño: una tarea sin WorkUnits puede solicitar preparación sin conceder
escritura; todo worker sigue exigiendo WU/asignación/lease explícitos.

### Registro PR0 — 2026-11-07

Completado sobre `ae3b2287` (V157 verificada). Entregables:

- [ADR 0016 — Task-scoped coordination turns](../adr/0016-task-coordination-turn.md):
  decisión de reutilizar wake/intento/Execution/receipts existentes, no un
  scheduler nuevo; comparación con la WorkUnit de coordinación y con Copilot.
- [Contrato PR0 — coordinación de tareas](pr0-task-coordination-contract.md):
  matriz de autoridad, modos `manual|proposal|prepare_and_execute`, envelope,
  modelo durable propuesto, contrato del `work_unit_plan`, materialización,
  carreras, REST/MCP/CLI, proyecciones UX y transición conservadora.
- Inventario verificado: `orchestrator_wake`/`orchestrator_wake_execution` y
  guards V152; `task_dispatch` V141/V145; TaskRole/capabilities V149–V152;
  Project Copilot V139/V140; colaboración V090 y planes V098; rework V143/V153/
  V157; TaskLifecycle, GateEngine, WorkUnitService y dispatcher.
- Transición fijada: todas las Tasks existentes pasan a `manual`; no se
  reetiqueta ni se reproduce ningún wake/Execution histórico; no se arranca
  trabajo al migrar.
- Gate de diseño: el turno no crea worker, Workspace ni lease; sólo la
  materialización determinista crea WorkUnits desde un plan exacto.

No se ejecutaron pruebas de runtime porque PR0 es documental. El siguiente
paso es PR2 (contrato mínimo backend) y, en el mismo corte vertical, la
superficie PR1 que no prometa capacidades ausentes. PR3 extiende la supervisión;
PR4–PR6 conservan revisión, intervención y aceptación.

### Registro de implementación backend — 2026-11-07

El backend de PR1–PR6 se implementa sobre el contrato PR0 con migraciones
aditivas V158–V160: autorización de coordinación versionada y equipo resuelto,
turno read-only sobre wakes existentes, plan `work_unit_plan` inspeccionable,
aprobación por digest, materialización atómica idempotente, solicitudes de
calidad con presupuesto global y consumo atómico, y un único circuito de
intervención para Humans y Project Copilot. TaskLifecycle, WorkUnit, review,
Gate y WorkspaceLease conservan su autoridad.

### Alcance real del corte — 2026-11-09

La auditoría independiente del PR 23 identificó huecos de durabilidad y de
alcance. Este corte corrige la revalidación transaccional, el vínculo atómico
wake/solicitud, la reanudación de intervenciones —incluida la cancelación entre
TaskLifecycle y dispatch—, el rol revisor literal, el falso ciclo del DAG y el
replay de objetivos distintos. También incorpora el consumidor durable de
solicitudes de calidad: sigue la cadena exacta informe→rework→nueva revisión,
no cruza correcciones de informes distintos, solo enlaza correcciones de rework
exactas y no resuelve un defecto por finalización técnica. La finalización
técnica automática exige autorización de coordinación vigente y no revocada, sin
equipo stale, y solo cubre entregas del plan, correcciones enlazadas a una
solicitud de calidad exacta o intentos de rework automático completados. La
versión exacta de autorización, el modo, la revocación y la staleness del equipo
se revalidan dentro de la misma transacción `BEGIN IMMEDIATE` que completa y
congela el resultado; una revocación o cambio a manual que gane la carrera deja
cero efecto automático. Las Tasks históricas/manuales no se automatizan
retroactivamente. La materialización crea la línea de revisión y el Gate de
aprobación que consumen las WorkUnits dependientes de la revisión.

No se declara PR1–PR6 completo. Siguen pendientes, con acuerdo de producto:

- límites por ruta de calidad y detección de ping-pong más allá del presupuesto
  global de la Task;
- aceptación E2E conjunta, recorrido con host/proveedor real y evidencia visual
  independiente (PR6).

El PR 23 no debe mergearse presentando el programa completo. La validación de
runtime y la aceptación conjunta con la UI se registran en el informe de cierre;
las pruebas de Rust se ejecutan en la fase de integración solicitada.

## PR1 — Equipo reutilizable y entrada comprensible

- Presentar equipo y flujo del Proyecto sobre los defaults existentes. Alta de
  roles visible arriba; plantillas con responsabilidades y capacidades; nombres
  de Actors resueltos con harness/host e IDs solo en detalles.
- Seleccionar coordinador y límites, resolver membresías elegibles y mostrar los
  roles que faltan. No imponer un formulario de cada WorkUnit para crear tarea.
- Vista de tarea vacía con objetivo/restricciones, equipo, preparar propuesta y
  preparar/ejecutar. Preflight distingue preparación de dispatch de workers.
- Mientras preparación no esté implementada, no habilitar un botón que finja
  ejecutar; se puede integrar este PR junto con PR2 mediante el mismo cambio
  vertical o mantener la superficie nueva fuera del flujo público.

## PR2 — Preparación desde objetivo sin WU manual

- Admitir el turno del coordinador mediante autorización durable, versión,
  Actor y harness exactos. No arrancar trabajador por este camino.
- Producir propuesta de desglose con WU, asignaciones elegibles, dependencias,
  entregables, criterios, revisión y condiciones de avance.
- Validar propuesta y materializar atómicamente sus referencias, con identidades
  estables para replay. Las acciones del modelo no son permisos.
- Preparar propuesta termina esperando aprobación. Preparar/ejecutar puede
  autorizar el plan compatible con la política; un exceso se vuelve excepción.
- Demostrar desde UI y API una tarea nueva sin roles/WU editados manualmente,
  usando equipo aprobado del Proyecto. Permitir comparar y modificar propuesta.

## PR3 — Coordinar ejecución y entrega técnica

- Supervisor despierta por hechos relevantes, no por tokens/logs. Explicar y
  registrar qué WorkUnit admite el dispatcher y qué espera cada participante.
- Acciones tipadas para preparar/asignar trabajo permitido y solicitar trabajo
  adicional dentro del plan autorizado, sin acceso directo a Git/SQL.
- Presentar entrega con artifacts/commits/manifiestos y comprobaciones técnicas.
  El flujo autorizado puede completar técnicamente la WU al cumplir el contrato,
  sin requerir un clic humano por cada unidad. No equivale a review pass.
- Distinguir fallo de intento, bloqueo de requisito y defecto de entrega.
  Conservar el modo manual y el acceso al historial técnico.

## PR4 — Solicitudes de calidad y rutas de retorno

- Introducir una solicitud durable de trabajo/decisión con categoría y efecto:
  corrección de entrega, replanteamiento, especialista o decisión humana.
- Reutilizar rework para corregir resultados; otras categorías tienen contracts
  propios y no falsifican un ReviewReport de código ni un retry.
- Enlazar informe, hallazgos, subject exacto, destinatario, política, solicitud,
  WorkUnits producidos, respuesta y verificación de resolución.
- Mensaje/handoff informativo sigue siendo informativo. UI solo muestra trabajo
  asignado/arrancado cuando existan esos hechos. No mutar semántica histórica.
- El supervisor puede recibir un defecto de planificación y coordinar ajuste;
  el revisor puede dirigirse a otros roles permitidos. Sin destino válido, pedir
  decisión humana. Nunca elegir aleatoriamente un Actor ni evadir independencia.
- Límites globales y por ruta, detección de ping-pong e idempotencia entre rutas.
  Cambiar categoría o rol no reinicia el presupuesto. Preservar ocho rondas de
  rework existentes salvo una decisión de producto posterior explícita.

## PR5 — Intervención humana y Copilot coherentes

- Una única bandeja durable por tarea para decisiones y respuestas versionadas.
  Afecta al mismo estado que consume el supervisor, sin una cola Copilot aparte.
- Copilot contextual explica el bloqueo y prepara opciones; mutaciones sobre
  tarea activa entran por intervención/propuesta, no por edición lateral.
- El humano puede pausar, cambiar instrucciones futuras, pedir replanteamiento
  o revocar autorización. Mostrar qué sigue activo y qué dejará de arrancar.
- Alcance mayor, desacuerdo, permisos, presupuesto y aprobación reservada tienen
  salida humana; ausencia de respuesta no equivale a permiso.

## PR6 — UX completa, conservación y aceptación

- Principal: resumen, equipo, plan/avance, revisión, resultado y próxima acción.
  Advanced: edición manual, roles/membresías, WU, intentos, logs y referencias.
- Integrar accesibilidad, responsive, nombres largos, estados de red y errores.
  Corregir los pendientes visuales conocidos de TaskDetail y diálogos.
- Actualizar getting-started/README para enseñar primero objetivo/equipo/modo;
  documentar también el modo manual. APIs, CLI y MCP exponen claramente las
  operaciones soportadas; no inventar aliases para ocultar retiradas.
- Revisar todos los gates de conservación y la suite relevante antes de cerrar.
  El nuevo flujo no cambia ni lanza la Task FirePOS real sin autorización.

## Matriz mínima de aceptación

| Escenario | Resultado obligatorio |
|---|---|
| Task vacía + equipo válido | Coordinador prepara desglose sin WU manual y sin worker Start prematuro |
| Equipo incompleto | Pregunta concreta; no editor técnico obligatorio ni selección de cuenta ajena |
| Preparar propuesta | Propuesta inspeccionable; cero escritura/worker Start antes de aprobar |
| Preparar y ejecutar | Avance completo dentro de límites, con actores y evidencia exactos |
| Planificación solo lectura | D1–D5 y paquete/revisión en Artifacts; cero cambios Git/proveedores |
| Defecto de entrega | Corrección al responsable, revisión independiente y Gate sobre resultado nuevo |
| Defecto del plan | Orquestador recibe solicitud, conserva historia y replantea futuro afectado |
| Decisión de negocio | Humano ve opciones; Copilot ayuda sin conceder permisos por sí mismo |
| Dos respuestas/consumidores | Un solo efecto y recibo; conflicto tipado si contenido cambia |
| Pausa/resume/cancel concurrentes | Sin nuevos Starts tras pausa; diferimientos recuperables; cancelación sin trabajo nuevo |
| Revocación antes de Start/acción | Autoridad vigente revalidada, sin efecto protegido |
| Reinicio/resultado incierto | Sin duplicar proveedor, WU, corrección ni presupuesto |
| Ciclo entre roles | Límite global y escalado humano, sin bucle indefinido |
| Migración de instalación real | IDs, permisos, evidencia y tareas preservados; automatización no retroactiva |
| UI móvil/dark/teclado | Acción principal accesible, nombres legibles, sin recortes ni cierre accidental |

Pruebas: DB upgrade en copia consistente más fixtures; servicios y API de
admisión/acciones/recuperación; MCP y CLI; E2E de los dos modos y de intervención;
cuatro viewports, light/dark y teclado. Inyectar carreras con barreras deterministas,
no solo sleeps. Fixtures no equivalen a aceptación de harness/proveedor real.
Registrar PASS/FAIL/BLOCKED/NOT RUN y versión exacta por etapa.

## Organización y definición de terminado

PR0 es secuencial. Después se pueden separar dominio/orquestación y frontend,
con DTOs y estados acordados y propietarios por archivo, en worktrees distintos.
Cada corte posterior se integra verticalmente; frontend no promete capacidades
pendientes y backend no se considera entregado sin el recorrido visible.
Revisar cada PR de forma independiente y reservar cierre conjunto de producto.

Terminado significa: un usuario nuevo configura equipo una vez, describe una
tarea, autoriza el modo y observa ejecución/revisión/corrección/entrega sin
montar manualmente el grafo. El sistema conserva control humano para excepciones,
trazabilidad y autoridad exacta. No basta que un endpoint cree WorkUnits ni que
los tests aislados de roles estén verdes.
