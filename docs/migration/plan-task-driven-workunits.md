# Plan: lanzar Tasks y coordinar WorkUnits sin gestión manual de Executions

Estado: propuesta de implementación para revisión humana. Este documento no
describe funcionalidad ya disponible ni autoriza un merge o un despliegue.
El estado implementado en esta rama, sin merge, se registra en
[task-driven-workunits-implementation.md](task-driven-workunits-implementation.md).

Fecha: 2026-11-05. Baseline inspeccionado:
`155ba434c11f601af7a7b61f782943439bb72526`, rama
`fix/post-v2-security-hardening`. Antes de implementar, comprobar de nuevo HEAD,
árbol de trabajo, migraciones y contratos; el inventario no congela el repositorio.

## 1. Resultado de producto

El flujo habitual será:

```text
Describir Task → preparar WorkUnits → Lanzar tarea → supervisar resultados
```

El Human no crea Executions, selecciona un Execution purpose, elige sesiones ni
conduce manualmente cada ronda de corrección. Puede preparar el trabajo
manualmente o delegarlo en el Project Copilot. Ambas vías usan los mismos
servicios y controles de dominio.

La Task ofrece lanzar, pausar, continuar y cancelar, además de resolver bloqueos.
Los WorkUnits muestran alcance, asignación, dependencias, progreso, entregables,
revisión y problemas. Los detalles técnicos quedan accesibles desde Activity y
diagnóstico, sin un formulario o una pestaña de gestión de Executions.

Se elimina `ExecutionPurpose` de contratos públicos y decisiones del runtime.
No se sustituye por otro enum de clasificación obligatorio, ni por un
clasificador LLM previo al arranque. Un WorkUnit puede exigir investigación,
planificación, implementación y pruebas sin cambiar de modo.

## 2. Qué desaparece y qué debe sobrevivir

Desaparecen la creación/control manual de Executions como flujo público y el
campo `purpose` activo en UI, REST, MCP, CLI, DTOs, eventos y persistencia actual.
Los clientes afectados reciben una retirada explícita y documentada; no habrá
aliases silenciosos ni dos contratos de lanzamiento permanentes.

Cada intento de trabajo conserva identidad técnica durable, Actor, WorkUnit y
versión admitida, configuración efectiva, sesión exacta, workspace/lease cuando
corresponda, recibos de Provider Start, resultado, uso y procedencia de outputs.
Puede reutilizarse la infraestructura interna de Execution para estos hechos.
Cambiar todos sus nombres no es un objetivo ni una condición de aceptación.

Estos registros se crean automáticamente. No son otra unidad que el usuario
deba configurar. Tampoco se colapsan en el estado mutable del WorkUnit: los
reintentos, cancelaciones, reasignaciones y cambios de sesión conservan historia.

Se preservan TaskLifecycle como única autoridad agregada, TaskRoles y
RoleMembership, asignación explícita, autoridad actual de daemon/Agent,
aislamiento de workspaces, integración serializada, Gates, revisión independiente,
ValidationRun y decisiones humanas. Lanzar una Task no concede merge, despliegue,
credenciales nuevas ni ampliación de alcance.

## 3. Lanzamiento y despacho

Reutilizar TaskLifecycleService, TaskDispatcher, admisión transaccional y recibos
actuales. No crear otro WorkflowEngine, otro polling loop o un estado de Task
paralelo. La intención durable de lanzar/pausar y su versión deben vivir bajo
esa autoridad; su representación exacta se decidirá con un inventario de datos.

El lanzamiento autenticado identifica la Task, su versión y una clave de
idempotencia. Comprueba alcance autorizado, WorkUnits preparados, asignaciones
y dependencias. La respuesta distingue admisión del lanzamiento, trabajo pendiente
y trabajo realmente iniciado; un HTTP exitoso no prueba un arranque externo.

El dispatcher:

1. Calcula readiness desde registros actuales y selecciona WorkUnits elegibles.
2. Resuelve un Actor concreto con membership activa. Un rol con varios miembros
   no implica fanout ni autoriza elegir un miembro arbitrario.
3. Admite el intento, sus snapshots y lease/recibos en la transacción vigente.
4. Revalida autoridad al reclamar Provider Start antes del efecto externo.
5. Despacha con límites de concurrencia y aislamiento; registra el resultado.
6. Desbloquea dependientes solo cuando sus requisitos exactos están satisfechos.

Un grafo ya preparado puede ejecutarse sin un Agent orquestador. El core decide
readiness y exclusión; los agentes deciden cómo trabajar y pueden proponer o
solicitar nuevas unidades dentro de la delegación autorizada.

Pausa impide nuevas admisiones/arranques al cruzar sus fronteras transaccionales;
no afirma haber detenido trabajo ya iniciado. Cancelación registra la solicitud
y confirma por separado los resultados observados. Continuar reutiliza los
recibos y las sesiones exactas que sigan siendo válidos. Un resultado externo
incierto nunca permite un segundo Start automático.

Un WorkUnit asignado a un Human genera trabajo pendiente para esa persona, sin
CLI o HarnessSession ficticios. Un Agent no disponible deja un motivo visible y
una vía de recuperación. El scheduler no cambia de cuenta o identidad en silencio.

Los WorkUnits añadidos dinámicamente deben estar preparados y dentro del alcance
delegado para entrar en el despacho. Una ampliación de requisitos se presenta
como decisión; no se ejecuta bajo la autorización de una corrección.

## 4. WorkUnits y revisión dinámica

Separar WorkUnits cuando cambia el responsable, el entregable o la aceptación.
No crear unidades para cada paso mental o comando del harness.

```text
Implementación I1 → Revisión R1
                         │ informe solicita cambios
                         ▼
                    Corrección C1 → Revisión R2
                                         │
                                    aceptar / corregir
```

R1 termina al entregar un informe válido, aunque su veredicto solicite cambios.
Ese estado no acepta I1 ni satisface un Gate de aprobación. Un fallo técnico del
revisor tampoco equivale a un informe negativo válido.

La revisión fija el sujeto exacto: WorkUnit/versión y Artifact/digest o commit,
según corresponda. Las correcciones y revisiones posteriores conservan los
resultados anteriores. Un informe sobre una versión antigua no aprueba otra.

Por defecto se añaden C1 y R2; no se reabre ni reescribe I1/R1. El grafo permanece
acíclico. Los reintentos técnicos dentro de una unidad siguen siendo intentos,
sin necesidad de crear otro WorkUnit por cada fallo de transporte.

### Una operación para solicitar corrección y nueva revisión

Diseñar una operación de dominio accesible por MCP y REST, con control equivalente
en UI. Su nombre final se fijará al implementar; este plan no publica un tool
ya disponible. Recibe:

- Task y versiones esperadas de los registros afectados.
- Informe, sujeto exacto y hallazgos pendientes.
- Alcance y aceptación de la corrección, sin inventar requisitos.
- Rol destinatario y, opcionalmente, Actor propuesto.
- Contexto de nueva revisión y clave de idempotencia.

En una transacción, el servicio comprueba autoridad actual, alcance, informe,
sujeto vigente, presupuesto y elegibilidad; registra C1/R2, sus relaciones,
procedencia y recibo idempotente. Puede dejar la asignación pendiente si falta un
Actor elegible, con un bloqueo explícito. La respuesta incluye las unidades
creadas o reproducidas. El efecto externo posterior corresponde al dispatcher.

El Agent llama a Forge para delegar trabajo; no recibe autoridad para lanzar un
proceso arbitrario, usar una cuenta ajena o saltarse la asignación.

La independencia excluye como revisor a los Actors que contribuyeron al resultado
revisado cuando la política exige independencia. Usar otro rol o un modelo
distinto con la misma identidad no la demuestra. La autocorrección sigue siendo
válida, pero no sustituye esa revisión.

### Dependencias y acceso al resultado

La readiness de revisión debe expresar que el resultado fijado está disponible.
Hoy una dependencia de un WorkUnit de repositorio puede exigir su integración;
no asumir que un edge existente permite revisar antes de integrar.

Introducir la relación mínima de sujeto/consumo de resultado necesaria para
revisión, sin debilitar las dependencias que sí exigen integración. Un reviewer
debe poder inspeccionar el snapshot autorizado exacto sin escribir en el
workspace activo de un implementador. No añadir un DSL de workflows.

### Límites y parada

La Task dispone de presupuesto acotado para rondas y trabajo, con valores
documentados al implementar. La operación consume presupuesto atómicamente;
reproducir el mismo recibo no lo consume otra vez.

Agotamiento, imposibilidad de asignación, desacuerdo no resuelto o ampliación de
alcance generan un bloqueo explicable y una decisión humana. No hay bucles
infinitos, aprobaciones automáticas por agotamiento ni pérdida de hallazgos.

## 5. Outputs y finalización

Artifacts: entregables de Agents, Humans o ValidationRuns con procedencia exacta.
Review: encargos, informes, sujetos, hallazgos y veredictos verificables.
Terminal: acceso operativo autorizado, no un entregable generado por el Agent.
History/Activity: proyección de hechos durables registrada por Forge.

El contrato de resultado del harness permite presentar entregables y solicitar
finalización. El core valida estructura, referencias, versión, dependencias y
evidencias antes de aceptarla. Una frase del modelo o un exit code cero no
completa un WorkUnit por sí solos. Las unidades de repositorio conservan el
resultado exacto e integración explícita cuando sea necesaria.

Todos los WorkUnits terminados no implican Task aceptada: los Gates, validaciones,
integraciones y aprobaciones humanas requeridos deben estar satisfechos. El
core evalúa hechos; no decide semánticamente si una solución es buena.

## 6. Inventario inicial de cambios

Verificar estos puntos de entrada antes de implementar:

| Área | Archivos/contratos iniciales | Cambio |
| --- | --- | --- |
| UI | `web/src/pages/TaskDetailPage.tsx`, vistas y clientes relacionados | Quitar selector/formulario; acciones de Task y progreso por WorkUnit |
| REST/MCP/CLI | `crates/api/src/routes/executions.rs`, `crates/mcp-server/src/{params,rpc,tools}`, clientes | Retirar lanzamiento manual y purpose; publicar operaciones de Task/rework |
| Despacho | `crates/services/src/task_dispatcher*`, `task_service/execution*` | Automatizar intentos conservando admisión y recuperación |
| Revisión/Gates | `task_service/execution/runner.rs`, rutas reviews, `db/src/sqlite/{execution,collaboration}.rs` | Usar sujeto/encargo/evidencia en lugar de purpose=review |
| Orquestación | `orchestrator_runtime.rs`, `db/src/sqlite/orchestrator_wake.rs` | Identificar delegación/wake explícitos; preservar dedupe y evitar auto-despertares |
| Planes | `plan_artifact.rs`, callers del runner | Producir Artifacts según entregables; eliminar ramas purpose=plan |
| Persistencia | models/repositories/migraciones, relaciones de productores | Preservar intentos e historia; retirar purpose activo al final |
| Operación | logs, SSE, terminal, daemon, uso, recuperación y cancelación | Mantener correlación y autoridad sin gestión pública manual |

El inventario completo debe clasificar cada uso antes de borrar: comportamiento,
contrato público, almacenamiento histórico o prueba. No eliminar indiscriminadamente
el vocabulario legítimo de ejecución de procesos o sesiones de harness.

## 7. Secuencia de implementación

Cada bloque entrega un cambio revisable, con tests y documentación del estado
real. No se mezclará con reparaciones pendientes del Project Copilot.

### P0 — Contrato e inventario

- Revisar este plan y registrar el contrato objetivo en un ADR.
- Mapear lectores/escritores y referencias de Execution y purpose, incluidos
  gates, productores, triggers, daemon, eventos y clientes externos.
- Identificar datos e intentos en curso y preparar fixtures de actualización.
- Revisar INV-008/009/014 y los consumidores de INV-016/018/019/028/029/033–036.
  Eliminar la exigencia de purpose sin invalidar historia ni autoridad técnica.

Salida: mapa completo, contratos nuevos y pruebas de preservación definidas.

### P1 — Lanzamiento durable de Task

- Implementar lanzar/pausar/continuar/cancelar y preflight comprensible.
- Extender dispatcher existente para WorkUnits preparados, incluidas tareas
  no-repositorio, Humans y disponibilidad de Agents.
- Probar concurrencia, idempotencia, permisos actuales, pausas y recuperación.

Salida: una Task preparada progresa sin creación manual de Executions.

### P2 — Entregables, revisión y corrección dinámica

- Desacoplar aceptación de resultados del purpose y del mero exit code.
- Implementar sujeto de revisión exacto y relación de disponibilidad de resultado.
- Implementar una llamada idempotente de corrección + nueva revisión y presupuesto.
- Integrar con Gates, rework y colaboración existentes, sin un segundo mecanismo
  de retries que consuma dos veces el mismo fallo.

Salida: I1 → R1 → C1 → R2 funciona y conserva independencia/procedencia.

### P3 — Retirada pública y desacoplamiento restante

- Usar el nuevo flujo en UI, REST, MCP, CLI y Project Copilot.
- Retirar operaciones manuales y schemas antiguos; documentar la ruptura.
- Eliminar ramas de planes/orquestación basadas en purpose, preservando wakes,
  presupuestos, protección de acciones y ausencia de bucles de eventos.
- Mantener inspección autorizada de intentos a través de Activity/diagnóstico.

Salida: ningún consumidor activo necesita purpose ni crear una Execution.

### P4 — Migración y limpieza

- Usar nuevas migraciones numeradas; no modificar las históricas ni fijar un
  número hasta comprobar el repositorio real.
- Preservar IDs, relaciones de outputs, recibos, sesiones, logs y valores
  históricos de purpose en almacenamiento histórico explícito.
- Migrar relaciones antes de retirar columnas/triggers activos. No convertir
  purpose histórico en authority actual ni reconstruir sujetos por heurísticas.
- Drenar o reconciliar intentos en curso con procedimiento probado. No iniciar
  trabajo automáticamente durante la actualización.
- Eliminar DTOs, enums, imports y tests obsoletos; sustituir pruebas que cubrían
  autoridad por pruebas del contrato nuevo, sin ocultar fallos.

Salida: purpose no participa en runtime/persistencia activa; historia íntegra.

## 8. Validación y aceptación

Primero tests focalizados; después suites relevantes, API happy path, fmt,
Clippy y frontend. No cargo clean ni reconstrucciones generales repetidas.

Casos obligatorios:

- Lanzamiento de Task con un WorkUnit y con DAG paralelo, sin purpose.
- Doble clic, reintento de red, dispatchers concurrentes y reinicio: sin doble Start.
- Caída antes/después del claim y resultado incierto: recuperación sin repetir efectos.
- Revocación de ownership/membership/daemon/lease entre admisión y claim: denegada.
- Pausa/cancelación durante cola y durante arranque: sin promesas falsas de parada.
- Agent ausente, rol ambiguo y Human asignado: estado explicable, sin selección falsa.
- WorkUnits no-repositorio, workspaces aislados e integración serializada.
- Revisión de resultado antes de integrar cuando corresponda, sin alterar edges
  que exigen integración.
- Rework idempotente y concurrente, presupuesto atómico, grafo sin ciclos y alcance acotado.
- Informe requests-changes finaliza la revisión pero no aprueba la entrega.
- Corrección cambia el sujeto; revisión anterior no satisface el Gate nuevo.
- Independencia comprueba contribuyentes, no etiquetas de rol/modelo.
- Reintento técnico, reasignación y cambio de sesión preservan intentos anteriores.
- Artifact o exit code del harness no elude aceptación, Gates ni autoridad humana.
- Actualización de una DB histórica y con trabajo pendiente: referencias preservadas.
- Contratos retirados fallan explícitamente; operaciones nuevas coherentes en REST/MCP/UI.
- E2E: lanzar → entregar → solicitar corrección → corregir → revisar → revisión humana.

La aceptación requiere diff completo revisado, comprobaciones del HEAD definitivo
y revisión adversarial independiente. Tests focalizados o servidor saludable no
se presentarán como CI completo. No merge ni despliegue implícitos.

## 9. Documentación, recuperación y límites

Actualizar en cada bloque los contratos afectados: architecture y architecture-v2,
conceptos de WorkUnits/roles/revisión/orquestación/Artifacts/sesiones, API, CLI,
getting-started, documentación de logs/eventos, README y AGENTS/CLAUDE cuando
corresponda. Registrar rupturas en CHANGELOG Unreleased. Los documentos actuales
siguen describiendo el runtime actual hasta implementar su cambio.

Antes del cutover de DB: copia consistente y ensayo de restauración. Un binario
antiguo no se ejecutará sobre una DB migrada sin compatibilidad demostrada. La
recuperación usa forward fix o restauración coherente de DB y estado asociado;
no promete deshacer efectos externos ni borrar trabajo de usuarios.

Fuera de alcance: renombrar Forge, sustituir harnesses, retirar autoridad durable,
crear un nuevo sistema de permisos, auto-merge, auto-deploy, workflow DSL o
eliminar registros históricos para obtener cero coincidencias textuales.

Revisión independiente del plan: pendiente. Los subagentes Luna solicitados
alcanzaron su límite de uso durante la sesión; no se atribuye una aprobación
independiente que no se ha realizado.
