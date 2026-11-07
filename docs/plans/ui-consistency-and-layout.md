# Plan B — Coherencia visual, márgenes y usabilidad de Forge

Estado: plan independiente para un agente de frontend.
Fecha: 2026-11-06. Baseline inspeccionado: `3d7621b6`.
Este documento propone trabajo; no afirma una auditoría visual terminada.

## Encargo y resultado

Corregir los problemas de espaciado, alineación, desbordamiento, jerarquía y
consistencia de Forge en conjunto. Mantener identidad, funcionalidades y datos.
El resultado debe facilitar trabajar con proyectos, tareas, WorkUnits, ejecuciones
y configuración sin controles cortados, anchos arbitrarios ni estilos dispares.
No limitar la mejora a una pantalla ni iniciar un rediseño de marca.

No modificar contratos backend, autoridad, roles, dispatch, Gates ni política de
rework. La iniciativa de roles del plan A es independiente y posee sus
formularios funcionales. No requiere nuevas dependencias ni herramientas de
sitios externos; se trabaja en el frontend React existente con pnpm.

## Evidencia inicial y límites

La inspección de código revela focos, todavía no defectos visuales reproducidos:

- `TaskDetailPage` combina bloques y tarjetas con padding 12/16/24, select nativo
  y componentes propios. Su navegación horizontal necesita comprobar ancho.
- `task-work-management` contiene formularios y filas densas, grids y controles
  nativos. Su implementación funcional pertenece a A; reportar incidencias y
  corregir las que se resuelvan en componentes compartidos, sin editar ese archivo.
- `ProjectSettingsPage` combina alturas calculadas fijas, overflow anidado,
  navegación horizontal y anchos distintos de los de TaskDetailPage.
- CardHeader/CardContent usan padding 24; otros paneles construyen a mano la
  misma estructura con valores diferentes. Input no fija una altura uniforme;
  coexisten Select propio, ComboSelect y select nativo.
- El repo ya tiene tokens en index.css, primitives, pruebas y Playwright.
  Extender ese sistema; no crear otro framework o una segunda biblioteca UI.

Primero reproducir cada incidencia en navegador. No afirmar «verificado» con
solo leer className. Guardar capturas antes/después de la misma ruta y estado.

## Auditoría y orden de trabajo

1. Levantar una instancia aislada mediante Makefile y datos desechables propios
   bajo `./test/`; usar un puerto libre distinto del servidor compartido. Si el
   target no permite elegir subdirectorio de datos, añadir/proponer un target
   acotado o usar un worktree aislado cuyo `./test/` sea distinto. No ejecutar
   clean-test sobre datos ajenos. No cambiar el servidor que usa el usuario.
2. Inventariar rutas visibles y estados, incluyendo autenticación, shell,
   sidebar, board/listado de tareas, detalle de tarea/ejecución, proyectos y sus
   settings/releases, agentes/hosts, operaciones y cuenta/settings globales.
3. Crear `docs/ui/consistency-audit.md`: ID, ruta/componente, viewport, estado,
   reproducción, captura, severidad, causa comprobada y propietario A/B.
   Inventariar en todos; implementar primero P0 (controles inaccesibles), P1
   (overflow, formularios, navegación), P2 (jerarquía/densidad), P3 (acabados).
4. Acordar tokens y patrones mediante una página de evidencia local o fixtures
   de componentes. No agregar una galería de desarrollo a la navegación pública.
5. Arreglar primitives y layout; después recorrer páginas por familias.
6. Repetir flujos reales en navegador y cerrar la matriz de cobertura. Los
   problemas reservados a A permanecen señalados hasta la integración conjunta.

## Contrato visual compartido con A

Conservar tipografía, paleta, dark/light y marca Forge existentes. Documentar los
valores en `docs/ui/visual-contract.md`; no cambiar el nombre del producto.

| Patrón | Regla inicial |
|---|---|
| Espaciado | Escala existente basada en 4 px; 8 entre elementos relacionados, 16 dentro de secciones, 24 entre secciones |
| Página | Gutter 16 px en pequeño, 24 en escritorio; un único responsable del margen exterior |
| Ancho | Formularios de lectura/edición con ancho acotado; board y tablas usan el espacio disponible |
| Panel | Padding 16 en pequeño y 24 cuando hay espacio; header/body/footer alineados |
| Campo | Etiqueta persistente, ayuda/error asociados, control y mensaje sin saltos arbitrarios |
| Controles | Altura ordinaria coherente de 40 px; variante compacta explícita de 32 px, sin mezclar tamaños accidentalmente |
| Acciones | Acción principal identificable, secundarias consistentes, destructivas diferenciadas; wrap o menú accesible |
| Texto largo | `min-w-0`, wrap para contenido; truncado solo con acceso al valor completo |
| Foco | Visible en ambos temas; tabulación predecible; sin acciones solo en hover |
| Scroll | Página con scroll principal; paneles internos solo cuando la tarea lo necesita y con límites claros |

La escala es una base, no una orden de forzar paneles densos a una única medida.
Registrar excepciones útiles (terminal, editor, board) y probarlas. No imponer un
max-width global que recorte el board ni overflow-hidden para ocultar fallos.
Botones táctiles tienen área de interacción suficiente incluso en modo compacto.

A usa primitives con sus APIs actuales. B puede añadir props opcionales sin
romper callers, pero no renombrar ni eliminar props existentes. Cambios en
altura/padding deben comprobarse contra formularios reservados a A mediante
fixtures equivalentes. A puede aplicar este contrato dentro de sus componentes
sin esperar nuevas APIs de B.

## Patrones que deben quedar resueltos

### Shell, páginas y navegación

Un responsable para gutter y ancho por familia de página. Cabeceras con título,
descripción y acciones que respondan al ancho real. Sidebar y navegación por
teclado en pequeño. Tabs accesibles y sin cortar el último tab. No ocultar
bloqueos/estados ni quitar detalles funcionales para simplificar una captura.

### Formularios y controles

Alinear etiquetas, ayudas, errores, selects, inputs y botones. Reutilizar Input,
Textarea, Select/ComboSelect y primitives existentes. Mantener select nativo
cuando convenga; darle el mismo contrato visual y semántico. No reemplazarlo
por un dropdown sin navegación de teclado, selección y lectores de pantalla.

Separar grupos por significado y reservar el mensaje de error en su contexto.
Estado disabled debe explicar el bloqueo cuando afecte a una operación esencial.
Preservar aria-labels, labels y nombres usados por pruebas salvo una mejora
justificada que actualice callers. No usar placeholders como única etiqueta.

### Datos, estado y superficies complejas

Tablas: columnas importantes visibles, scroll horizontal local cuando sea
necesario, cabeceras y acciones accesibles. Tarjetas y badges: tamaños y colores
semánticos consistentes, sin usar solo color para comunicar estado. Loading,
empty, error, conflicto 409 y pending con un patrón común y recuperación visible.

Dialogs/sheets: título y foco correctos, cierre accesible, cuerpo desplazable
cuando haga falta, acciones visibles sin tapar contenido. Dropdowns/portals:
comprobar clipping, posición cerca de bordes y z-index en dialogs y scroll.
Terminales, logs, Markdown y bloques de código conservan legibilidad, copiado y
su propio scroll; una línea larga no ensancha la página completa.

## Propiedad de archivos y trabajo paralelo

B posee:

- `web/src/index.css`, configuración visual existente y componentes genéricos
  en `web/src/components/ui/`; tests correspondientes.
- Shell, navegación y contenedores globales que encuentre en el inventario.
- Páginas distintas de TaskDetailPage y sus componentes de dominio, incluyendo
  ProjectSettingsPage como contenedor; board/listados, detalles de Execution,
  cuenta/settings, agentes, hosts y operaciones. Mantener contratos de props.
- Nuevos helpers presentacionales bajo `web/src/components/layout/` si la
  auditoría demuestra repetición real; no extraer wrappers por deporte.
- `docs/ui/`, nuevos specs visuales y evidencias bajo la ruta de artifacts local
  que se indique en el ledger. No meter capturas generadas masivas en Git.

A posee Rust/SQL, DTOs, tipos generados, API/hooks/SSE y contratos funcionales;
TaskDetailPage y sus tests; task-work-management, task-rework-panel,
work-unit-review-panel y sus tests; formularios de defaults de roles en
components/settings; nuevos formularios `features/task-roles/`.
B no edita esos archivos. Puede inspeccionarlos y registrar defectos. Los cambios
visual-only dentro de ellos se describen con reproducción y criterio de cierre
para que A los aplique como parte de su trabajo. B sigue con el resto de UI.

No tocar package.json/pnpm-lock ni Playwright config sin coordinar un propietario
único. Reutilizar herramientas ya instaladas. Añadir specs nuevos sin modificar
los de A. No hacer formatting masivo, cambiar APIs de componentes, reorganizar
carpetas de dominio ni editar datos compartidos para conseguir capturas.

Ambos parten del mismo baseline en ramas/worktrees separados. Los cambios de B
en primitives llegan por integración Git; A no necesita importarlos para
implementar su dominio. No hacer cherry-picks cruzados durante el trabajo.

## Pruebas y aceptación visual

Viewport mínimo: 390×844, 768×1024, 1280×800, 1440×900. Añadir prueba a 200% de
zoom y 320 CSS px de ancho efectivo para formularios/lectura; board, tablas y
terminales pueden requerir scroll local. Comprobar light y dark.

Datos: títulos de 200 caracteres, nombres de roles largos, IDs/rutas sin espacios,
listas vacías y extensas, errores multilínea, Markdown/tablas/código, múltiples
acciones, cargas lentas, permisos restringidos y estados disabled.

Criterios observables:

- Sin overflow horizontal de página a los anchos objetivo; excepciones de
  contenido bidimensional tienen contenedor local y navegación utilizable.
- Sin superposición o recorte de etiquetas, acciones, dropdowns ni footer de
  dialogs. Ninguna corrección depende de esconder contenido.
- Cabeceras, paneles y campos de una familia comparten alineación/espaciado.
- Todas las acciones esenciales siguen accesibles por teclado y en pantalla
  pequeña; foco no se pierde al abrir/cerrar superficies.
- Contraste AA del texto/controles aplicables; estados no dependen solo del color.
- Loading/error/empty/pending son legibles y coherentes, con acción de recuperación
  donde ya exista. No añadir falsas acciones ni esconder limitaciones backend.
- Flujos login -> proyecto -> tarea -> actividad y settings -> edición -> error
  -> guardado mantienen comportamiento; comparar peticiones si surge una duda.
- Auditoría cubre todas las rutas inventariadas. Ningún P0/P1 propio queda sin
  resolver; los reservados a A tienen asignación y cierre conjunto explícitos.

Checks: pnpm lint, pnpm typecheck, pnpm test, pnpm build y E2E de smoke más specs
visuales focalizados. Usar Playwright y capturas con fixtures estables; si faltan
browser o auth, registrar BLOCKED y causa exacta, sin afirmar aceptación visual.
No usar snapshots de píxeles como única prueba ni agregar tests que solo reflejen
clases CSS. Capturar light/dark, diálogos abiertos, estados de error y viewport.

## Entrega, integración y prompt para el agente

Entregar auditoría priorizada, contrato visual, implementación, capturas
comparables y ledger de PASS/FAIL/BLOCKED/NOT RUN. Distinguir tests funcionales,
aceptación visual y estados pendientes del agente A.

Integración final serial por el responsable de juntar ambos trabajos:

1. Integrar A y B conservando contratos de primitives y datos.
2. A aplica y verifica los defectos de UI de su lista reservada.
3. Probar juntos plantillas, capacidades, revisión negativa, devolución,
   corrección y aprobación en los cuatro viewports y ambos temas.
4. Repetir checks afectados y revisar diff. No declarar UI global terminada con
   defectos P0/P1 pendientes en la nueva superficie de roles.

No mergear, publicar ni reiniciar el Forge compartido sin autorización. No
modificar FirePOS ni activar proveedores. Esto es mejora de producto existente.

> Implementa docs/plans/ui-consistency-and-layout.md como agente de UI.
> Empieza por una auditoría en navegador con evidencia y priorización. Mejora
> tokens, primitives, shell y páginas de tu propiedad conservando sus contratos.
> Respeta los archivos reservados al plan A; documenta sus defectos para cierre
> conjunto. Usa una instancia y datos aislados. Entrega capturas antes/después,
> pruebas y matriz de cobertura, sin cambios backend ni despliegues.
