# Contrato visual

Se conservan Forge, las familias tipográficas y la paleta light/dark de index.css.
Este contrato no cambia autoridad, APIs, permisos ni estado de dominio.

| Patrón | Valor y responsable |
| --- | --- |
| Espaciado | Escala de 4 px; 8 entre controles, 16 dentro de sección, 24 entre secciones |
| Página | AppShell aplica 16 px, 24 desde escritorio; las páginas no duplican ese gutter |
| Settings/cuenta | Navegación superior en pequeño, lateral de 224 px desde lg; scroll de página; edición acotada a 760 px |
| Sección de settings | Una columna hasta xl; descripción de 220 px y contenido flexible desde xl |
| Panel | Card y DialogContent: 16 px, 24 desde sm; header y body alineados |
| Controles | Input, Select, ComboSelect y Button ordinarios: 40 px; Button sm: 32 px; overrides de altura existentes conservados |
| Acciones | Footer de dialog con gap 8, apilado en móvil y wrap en escritorio |
| Texto | min-w-0 en cadenas flexibles; identificadores completos con wrap; truncado requiere acceso al valor completo |
| Navegación | Tabs compartidos permiten wrap; navegación de settings conserva enlaces nativos |
| Foco | Ring existente en ambos temas; Dialog conserva trap, Escape y restauración de foco |

Board, terminal, editor y tablas son superficies bidimensionales: pueden tener
scroll local. No reciben un max-width global. Las variantes icon existentes son
excepciones compactas; su aceptación táctil sigue pendiente de auditoría.

Las fixtures `web/e2e/ui-consistency-fixture.tsx` se sirven únicamente por Vite
para Playwright. No se añaden rutas ni enlaces a la navegación del producto.
Los formularios de roles de A mantienen sus props y archivos; deben comprobarse
juntos tras integrar A y B. No se declara contraste AA global sin medición.
