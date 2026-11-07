# Auditoría de las implementaciones A y B — 2026-11-06

Baseline Git: `3d7621b6`; revisión sobre el diff local sin commit y con otra
sesión todavía terminando pruebas. Estos resultados corresponden al estado
inspeccionado, no a un HEAD final ni a aceptación de producción.
No se modificó implementación, la Task FirePOS ni la base de datos compartida.

## Dictamen

NO-GO para cerrar A o aceptar globalmente A+B. B aporta mejoras verificables,
pero conserva una interacción defectuosa y pendientes de aceptación visual.

## Hallazgos

### A-01 — P1: V150 impide actualizar una base instalada con intentos ordinarios

V149 inicializa configuraciones no especiales con `{}`. V150 hace backfill de
`execution_task_role_authority.capabilities_json` mediante
`json_extract(configuration_json, '$.capabilities')` en una columna NOT NULL.
Para roles ordinarios la expresión devuelve NULL.

Reproducido sobre copia SQLite consistente de `test/forge.db` en
`/tmp/forge-independent-audit.sqlite`: V149 PASS; V150 FAIL con
`NOT NULL constraint failed: execution_task_role_authority.capabilities_json`.
La copia contiene diez filas de historial coincidentes para `planner` con `{}`.
La base original solo se leyó. No se ejecutó Forge sobre la copia ni se avanzó
su ledger de migraciones. Los fixtures de base vacía no demuestran este upgrade.

Corregir el backfill respetando la ausencia de capacidades privilegiadas y
probar también planner/implementer/personalizados con Executions históricas y
membership válido. La revisión no autoriza editar migraciones ya publicadas;
resolver cómo asegurar que una instalación V148 pueda atravesar V150 antes de
aplicar migraciones posteriores.

### A-02 — P2: la selección de pendientes puede bloquear globalmente rework

`process_pending_automatic_rework` toma los primeros 100 pending, incluidos los
pausados, y después los omite sin avanzar cursor ni estado. Si los 100 más
antiguos están pausados, una intención posterior de una Task running nunca se
selecciona en ticks sucesivos. El límite 100 es el usado por el runtime.

Reproducida la consulta exacta en SQLite aislado con 100 pendientes pausados y
uno running: dos selecciones consecutivas de 100 filas, cero procesables y el
running fuera de ambas. Esto comprueba el bloqueo del algoritmo de selección;
no es un E2E con 101 harnesses. Filtrar diferidos antes del límite o introducir
paginación/selección justa, preservando la cancelación y las intenciones durables.

### A-03 — P2: pausa no se revalida al materializar la corrección automática

Hallazgo por inspección, no reproducción concurrente: el estado de dispatch se
lee antes del procesamiento; `request_rework` abre BEGIN IMMEDIATE y revalida
Task/lifecycle/capacidades/membership, pero no task_dispatch para la solicitud
automática. Pausar después de la lectura y antes de esa transacción permite
crear C1/R2 y consumir presupuesto después de responder a la pausa.

La pausa modifica task_dispatch, no las versiones que aquí se comprueban. Los
guards de Execution aún impiden arrancar los nuevos intentos: no se afirma un
Provider Start posterior a la pausa. Es incumplimiento de la política propuesta
que difiere también la materialización automática hasta resume.

### B-01 — P2: Escape del desplegable cierra también el diálogo

Reproducido con Chromium usando los componentes actuales, Vite aislado en
15180 y un fixture temporal sin datos backend: abrir Dialog, abrir Select dentro
del Dialog y pulsar Escape. Resultado: `dialogRemaining=0,listboxRemaining=0`.
El Select consume Escape con preventDefault pero el listener de document del
Dialog también lo maneja y cierra el formulario. Esperado: cerrar solo el
Select, conservar el diálogo y devolver foco al trigger.

Evidencia: `/tmp/forge-audit-ui-escape.png`, script
`/tmp/forge-audit-ui.cjs`. El fixture de código temporal y Vite se retiraron.
Esto es un defecto pendiente del sistema de componentes; no se atribuye su
introducción a este diff. Agregar también ComboSelect/portal/foco al recorrido.

### AB-01 — P2: cierre visual conjunto pendiente en TaskDetail

La captura de B de la cabecera de TaskDetail a 320 px muestra el título largo
encajonado por Edit task y el badge, desplazando navegación y contenido más
allá del viewport. El código actual conserva esa fila y su flex-1. La propia
auditoría B lo registra como A01. No repetirlo como hallazgo nuevo ni declarar
cierre global mientras siga abierto. Hace falta captura posterior en ambos
temas y cierre por A sobre su superficie reservada.

## Cobertura y comprobaciones independientes

- PASS: pnpm typecheck.
- PASS: cuatro suites focalizadas de frontend, diez tests: task-work-management,
  task-rework-panel, work-unit-review-panel y TaskDetailPage.
- FAIL: upgrade SQL V148 -> V150 sobre copia de datos instalados.
- FAIL: interacción Select dentro de Dialog al pulsar Escape.
- FAIL: git diff --check, whitespace en bindings generados del diff actual.
- Inspeccionadas: capturas Cuenta móvil posterior y TaskDetail móvil de B.
  Cuenta presenta una mejora clara; no equivale a una nueva auditoría visual de
  todas las rutas ni a aceptación funcional de roles.
- NOT RUN: suites Rust completas, pruebas concurrentes del worker automático,
  E2E completo de roles con proveedores reales y cierre conjunto de A+B.

El ledger B declara correctamente NOT RUN para zoom real al 200%, contraste AA
global, Execution/terminal real, Release poblada y cierre con A. Esos pendientes
siguen fuera de la aceptación; fixtures no requieren activar proveedores para
cubrir todas las variantes visuales.

No se cambió implementación ni se crearon commits en esta revisión. Las
reproducciones usaron copias, SQLite en memoria y navegador aislado. El informe
no valida los resultados que otra sesión todavía pueda estar ejecutando.
