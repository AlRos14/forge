# Auditoría de coherencia visual

Instancia propia: `make dev-ui-audit`, `127.0.0.1:18081`, datos
`test/ui-consistency`. Vite: puerto 15173 con `VITE_API_PROXY_TARGET` a 18081.
Sin daemon, MCP, proveedores, limpieza ni reinicio del servidor compartido.

Evidencia local (no versionada): `test/ui-consistency-artifacts/`.
Los subdirectorios de Playwright incluyen viewport/tema y los PNG incluyen ruta.
`baseline` contiene capturas previas; `verified-after` las posteriores esperando contenido renderizado.
`after` fue interrumpida al detectar capturas de Loading y no se acepta.
`before` es una primera pasada exploratoria cuyo dark no estaba configurado
correctamente: no se usa como evidencia de dark. `primitives-before` falló por
la inicialización del fixture React y no constituye evidencia de producto.

## Incidencias reproducidas

| ID | Ruta/componente | Viewport/estado | Reproducción y captura | Prioridad | Causa comprobada / propietario |
| --- | --- | --- | --- | --- | --- |
| B01 | /account, /account/tokens | 320 y 390, sesión autenticada | Abrir Profile; baseline/*320-light*/-account.png | P0 | Sidebar fija 224 px y contenido sin min-w-0 recortan formulario; B |
| B02 | /settings, /settings/agent, /settings/paths | 320, light/dark | Abrir sección; baseline/*320-light*/-settings.png, error-context.md | P1 | Sidebar fija y grid interno 220px+contenido; overflow documento de 201 px medido; B |
| B03 | Select | 320, etiqueta de 300 caracteres | Fixture Role; primitives-baseline, error-context.md | P1 | Portal sin ancho máximo: borde derecho 1917.4 px; B |
| B04 | Cuenta/settings/hosts | móvil | Comparar familia en capturas baseline | P2 | Alturas calculadas y scroll interno, padding dispar; B |

## Inventario y cobertura

La especificación `ui-consistency.spec.ts` recorre 17 rutas en 320, 390, 768,
1280 y 1440 CSS px, en light y dark. 320 es prueba de reflow, no una prueba
real del zoom de navegador al 200%. Proyecto con nombre largo y Task real,
creados únicamente en la instancia desechable. Sin fallback silencioso de
backend: el spec exige que la lectura de Projects tenga éxito.

| Familia | Rutas/estado | Cobertura |
| --- | --- | --- |
| Shell y navegación | autenticado, sidebar móvil, títulos largos | capturas y medición de overflow |
| Cuenta | /account, /account/tokens | profile, tokens vacíos |
| Global settings | /settings, /settings/agent, /settings/paths | configuración real local |
| Agentes/hosts | /agents, /daemons | vacíos, sin activar hosts |
| Operaciones | /operations | estado local |
| Proyecto | /projects/:id/board, /tasks | una tarea de título largo |
| Project settings | general/repos/members/workers/mcp/hooks/danger | sin repos ni proveedores |
| Primitives | Input, Button, Select, Card, Dialog | fixture, texto largo, teclado, Escape, restauración de foco |
| Autenticación | /login, /register, /oauth/authorize/consent | pendiente de flujo completo |
| TaskDetail (A) | /tasks/:id y overview/work/executions/review/terminal/history | 30 capturas light; A01 y cierre conjunto pendientes |
| Execution | /tasks/:id/executions/:id | pendiente: no iniciar harness para generar evidencia |
| Releases | /projects/:id/releases/:id | pendiente de fixture estable de snapshot |

## Límites de aceptación

No se declara aceptación visual global. Pendientes: zoom real al 200%, contraste
AA medido, datos extensos, permisos restringidos, errores 409 y cargas lentas,
Markdown y terminales, flujo completo login/proyecto/tarea/actividad y edición
con error/guardado. Los estados de dominio de A y su integración final requieren
cierre conjunto; no se modifican sus archivos. No se inventan defectos en A por
lectura de clases: deben reproducirse antes de asignarlos.

## Reproducir

```bash
FORGE_SKIP_WEB_BUILD=1 make dev-ui-audit
FORGE_E2E_WEB_PORT=15173 VITE_API_PROXY_TARGET=http://127.0.0.1:18081 pnpm --dir web dev
CI=1 FORGE_E2E_WEB_BASE_URL=http://127.0.0.1:15173 FORGE_E2E_BACKEND_BASE_URL=http://127.0.0.1:18081 pnpm --dir web exec playwright test e2e/ui-consistency.spec.ts e2e/ui-primitives.spec.ts --retries=0 --output=../test/ui-consistency-artifacts/verified-after
```

CI=1 selecciona el Chromium instalado en vez de exigir Google Chrome. Mantener
la instancia separada: los fixtures de auth crean un usuario de prueba y el
spec crea un proyecto y una tarea si no hay ninguno. No apuntar al servidor
compartido. Evidencia y ledger se complementan; una medición de overflow no
prueba ausencia de clipping ni accesibilidad completa.

## Superficie reservada a A

A01 (P2): `/tasks/:id/work`, 320 px, título de 200 caracteres. Captura
`route-states/ui-route-states-route-states-320-chromium/task-work.png`.
El título comparte fila con Edit task y el badge de lifecycle: queda una columna
estrecha y la navegación termina muy por debajo del primer viewport. Propuesta
para A: permitir wrap en la cabecera y situar acciones debajo cuando no quede
ancho de lectura. Cierre: título legible con acciones y tabs accesibles en 320 y
390, mismos datos, ambos temas. B no modifica TaskDetailPage.

`ui-auth-layout.spec.ts`: PASS de login/register en cinco anchos y ambos temas
(10 casos). `ui-route-states.spec.ts`: PASS de carga de seis pestañas de Task en
cinco anchos (5 casos, light). Esto prueba carga y aporta capturas; no certifica
la usabilidad de cada panel ni los estados de roles todavía no integrados.

## Comparación inspeccionada

- [Cuenta antes, 320 light](../../test/ui-consistency-artifacts/baseline/ui-consistency-layout-320-light-chromium/-account.png).
- [Cuenta después, 320 light](../../test/ui-consistency-artifacts/verified-after/ui-consistency-layout-320-light-chromium/-account.png).
- [Select abierto, 320 dark](../../test/ui-consistency-artifacts/dropdown/ui-primitives-controls-320-dark-chromium/select-open.png).

Cuenta mantiene ruta, viewport y estado de edición; el nombre visible cambia al
ejecutar el flujo de guardado (`E2E Default User` → `Layout audit user`). La
comparación demuestra el cambio de distribución, no igualdad de píxeles/datos.
Los formularios ya no quedan cortados; las capturas han sido inspeccionadas.
