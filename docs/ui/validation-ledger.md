# Ledger de validación

| Comprobación | Resultado | Evidencia/límite |
| --- | --- | --- |
| Baseline de 17 rutas × cinco anchos × dos temas | FAIL esperado | 4 casos móviles fallan por overflow en settings; 6 pasan. `baseline/` |
| pnpm lint | PASS | tras cambios de implementación |
| pnpm typecheck | PASS | tsc -b |
| pnpm test | PASS | 29 archivos, 145 tests |
| pnpm build | PASS | Vite; aviso de chunks >500 kB y Browserslist antiguo |
| Smoke y perfil 409 → recuperación → guardado → navegación por teclado | PASS | `verified-after/`, instancia local real; error 409 inyectado por Playwright |
| Login/register | PASS | 10 casos; `route-states/` |
| Pestañas de Task de A | PASS de carga | 5 casos, 30 capturas light; no aceptación funcional de roles |
| Layout posterior y primitives | PASS | 22 E2E (10 layouts, 10 primitives, smoke y perfil); `verified-after/` |
| Alturas exactas de escritorio | PASS, 4 casos | `desktop-height/`: 1280×800 y 1440×900 |
| git diff --check | PASS | Sin errores de whitespace |
| Zoom real 200%, contraste AA global | NOT RUN | 320 CSS px cubre reflow, no sustituye zoom real |
| Execution/terminal real y Release poblada | NOT RUN | sin iniciar harness/proveedores para generar datos |
| Cierre conjunto con A | NOT RUN | A01 y estados funcionales de roles pendientes |
| Despliegue, merge y servidor compartido | NOT RUN | fuera de este cambio |

Evidencia local: `test/ui-consistency-artifacts/`. Los directorios exploratorios
`before`, `after`, `primitives-before` no certifican aceptación. Ver motivos en
la auditoría. La evidencia de browser es local, no validación de producción.

Select abierto a 320 px: PASS, 2 casos adicionales en light/dark; capturas en
`dropdown/`. Las 37 pruebas E2E principales y 6 comprobaciones adicionales
pasan. Las limitaciones NOT RUN anteriores siguen abiertas: esto no constituye
aceptación global del plan ni cierre de la integración con A.
