# ADR 0002: `Ctrl+Enter` no es portable; `F5` como respaldo

## Contexto

El keymap de Hito 2 pide "ejecutar con `Ctrl+Enter`". Verificando la TUI
interactivamente (dentro de `tmux`) se confirmó que `Ctrl+Enter` no llega a
la aplicación como una combinación distinta de `Enter` solo: la mayoría de
terminales (xterm, `tmux` sin passthrough especial, y muchos otros) no tienen
una codificación ANSI estándar para `Ctrl+Enter` y envían exactamente los
mismos bytes que `Enter`. Sin protocolo extendido, `crossterm` no puede
distinguirlos.

El único mecanismo estándar que sí lo permite es el "Kitty keyboard
protocol" (`crossterm::event::PushKeyboardEnhancementFlags` con
`DISAMBIGUATE_ESCAPE_CODES`), soportado por kitty, wezterm, foot, ghostty,
contour y algunos otros — pero no por xterm ni por `tmux` sin configuración
adicional.

## Decisión

1. Al arrancar la TUI, `sqldr` consulta
   `crossterm::terminal::supports_keyboard_enhancement()` y, si el terminal
   lo soporta, activa `DISAMBIGUATE_ESCAPE_CODES` (y lo desactiva al salir).
   En esos terminales, `Ctrl+Enter` funciona tal como pide el keymap.
2. Se agrega `F5` como atajo equivalente para ejecutar la query del editor,
   siempre disponible sin importar el terminal. Es la convención que ya
   usan herramientas SQL de escritorio (DataGrip, DBeaver, Azure Data
   Studio) para "ejecutar", así que no es una tecla arbitraria.
3. La barra de estado muestra `Ctrl+Enter/F5: ejecutar` para no depender de
   que el usuario adivine cuál de las dos funciona en su terminal.

## Consecuencias

- En terminales sin el protocolo extendido (la mayoría hoy), `Ctrl+Enter`
  simplemente inserta una línea nueva en el editor (comportamiento por
  defecto de `tui-textarea` para `Enter`), y el usuario debe usar `F5`.
- No se intentó "adivinar" `Ctrl+Enter` interceptando `Enter` a secas,
  porque el editor es multilínea: convertir `Enter` en "ejecutar" rompería
  la edición de SQL con varias líneas.
