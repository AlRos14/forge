# Segunda revisión de A+B — 2026-11-06

Estado inspeccionado: diff local sobre `3d7621b6`, todavía sin commit y con
cambios/pruebas de otra sesión en curso. No implica revisión de un HEAD final.
No se modificó implementación ni datos compartidos.

## Hallazgos anteriores comprobados de nuevo

- A-01, migración: RESUELTO en la fuente inspeccionada. V149 inicializa el
  documento completo y capacidades vacías. V149–V155 PASS sobre una nueva copia
  consistente de la base instalada V148: `/tmp/forge-audit-round2.sqlite`.
- A-02, starvation de cola: RESUELTO. La consulta ahora excluye paused antes de
  LIMIT, conservando cancelación. Repetida con la constante SQL actual, 100
  pendientes pausados y uno running: se selecciona el running.
- A-03, pausa: la materialización ahora revalida dispatch dentro de la
  transacción. Cierra la carrera de creación, pero introduce el problema R2-02.
- B-01, Escape: PERSISTE, reproducido con Chromium y fixture temporal usando
  componentes actuales. Select dentro de Dialog + Escape deja cero Dialog y
  cero listbox; se esperaba conservar el Dialog. Vite aislado y fixture retirados.
- AB-01, cabecera móvil y cierre visual conjunto: pendiente. La disposición de
  título/acción/badge y el ledger de B siguen dejando ese cierre abierto.

## R2-01 — P1: conciliar intervención manual rompe la cola automática

El servicio contempla una corrección manual previa para el mismo informe:
`process_pending_automatic_rework` busca un receipt por Task e informe y llama a
`finish_automation_intent(...completed...)` con ese par. Sin embargo, el trigger
`task_rework_automation_intent_pair_guard` exige además igualdad entre la clave
manual y la clave automática derivada. Son dos solicitudes con claves distintas.

Reproducida la actualización con el trigger SQL literal de V153 en una base en
memoria: todas las referencias coinciden salvo las claves de las dos solicitudes.
SQLite rechaza: `completed automatic rework intent must reference its exact
atomic rework receipt`. La intención queda pendiente; cada tick vuelve al mismo
error y la propagación con `?` interrumpe el lote. No se solicita crear otro par
ni debilitar el vínculo de sujeto: hace falta representar explícitamente la
conciliación con el receipt manual exacto, con identidad auditable.

Reproducido también en API con un fixture desechable: capturar intención,
pausar/reanudar, crear manualmente el par con otra clave y ejecutar el worker.
La solicitud manual tiene éxito; el worker falla con el mismo SqliteError 1811.
Log: `/tmp/forge-audit-round2-reconcile.log`. Fuente temporal conservada en
`/tmp/forge-audit-round2-api-regression.rs`; retirada del árbol de tests al acabar.
Es regresión local con fixture shell, no aceptación de un proveedor cognitivo.

## R2-02 — P2: una pausa concurrente convierte la intención en bloqueo terminal

Hallazgo por inspección: si el worker leyó running y la Task se pausa antes de
la transacción, request_rework devuelve InvalidOperation por pausa. El manejador
clasifica toda InvalidOperation como blocker y cambia la intención de pending a
blocked. V153 hace esa transición terminal; la consulta posterior solo selecciona
pending. Resume no vuelve a intentar la devolución, aunque el mensaje diga que
reanudar basta.

La comprobación transaccional de pausa debe conservarse; el resultado de pausa
es diferimiento reintentable, no bloqueo definitivo. También debe probarse
cancelación concurrente y distinguirla de una decisión humana pendiente.
No se ha reproducido el interleaving con un harness real ni se afirma arranque
posterior a pausa.

## Validación independiente

- PASS: aplicación de V149–V155 sobre copia instalada, sin tocar el original.
- PASS: selección actual del backlog en SQLite con 101 intenciones.
- PASS: pnpm typecheck y diez tests en cuatro suites frontend focalizadas.
- FAIL: conciliación manual/automática en V153, reproducida también en API.
- FAIL: Escape en Select portalled dentro de Dialog, Chromium.
- PASS: git diff --check en el estado al que corresponde esta segunda revisión.
- NOT RUN: CI completo y aceptación con proveedores/harness reales; cierre
  visual conjunto, zoom real y contraste AA global continúan sin demostrar.

No se reejecutaron las suites de la otra sesión ni se apropiaron sus resultados.
La prueba API esperó al target compartido y terminó con el fallo descrito.
No se interrumpió la validación ajena ni se limpió target para forzar su ejecución.

## Revisión 3 — resolución de pendientes

- R2-01, IMPLEMENTADO: V156 conserva la clave automática original y agrega
  `reconciled_manual_receipt_id`. El worker compara informe, WorkUnit y versión
  exactos, y completa la intención con el ID del recibo humano. El trigger exige
  que ese ID señale el mismo Task, informe, sujeto, par y ronda y que la clave
  del recibo sea distinta; el enlace solo se puede añadir al completar una
  intención pendiente. PASS: `cargo test -p api --test task_rework
  automatic_rework_replays_once_and_waits_for_task_resume`; el test crea el
  recibo manual con otra clave, reanuda el worker y verifica el vínculo al ID
  exacto sin crear un segundo par.
- R2-02, IMPLEMENTADO: pausa y cancelación transaccionales devuelven errores
  tipados distintos. Una pausa concurrente deja la intención `pending`; una
  cancelación la marca `cancelled`. Una decisión humana inválida sigue siendo
  `blocked`. PASS: prueba unitaria de clasificación y prueba API existente de
  rechazo mientras dispatch está pausado. No se simula un interleaving real de
  dos transacciones en esta ronda.
- R2-03, RESUELTO: ambos handlers Escape del Select previenen propagación al
  listener de Dialog. PASS: 3 tests de `select.test.tsx`, incluyendo opción
  portal y disparador dentro del diálogo.

Validación de esta ronda: PASS `cargo fmt --all -- --check`, `git diff --check`,
`FORGE_SKIP_WEB_BUILD=1 cargo clippy --workspace --all-targets -- -D warnings`,
`pnpm lint`, `pnpm typecheck`, `pnpm build`, Select (3/3), suite frontend (148
tests en 29 archivos), tests de `services` (2/2), regresión API/manual (1/1,
con migraciones hasta V156), y fixture de preservación V148→V156.

`cargo test -p db` quedó parcial: 71/71 tests unitarios pasaron, incluida la
fixture V148→V156. Se detuvo al entrar en los 17 tests de migración file-backed:
la parte unitaria había tardado 1162.80 s y los primeros tests file-backed
superaron 60 s sin completar. No se observó fallo en esa fase, pero no se cuenta
como PASS completo. NOT RUN: suite MCP completa, CI completo, E2E Chromium tras
la corrección, y aceptación con proveedores reales. La reproducción Chromium
del fallo previa a la corrección se conserva en esta revisión.
