# DevStack — UI v2 design system

## Scope
A standalone Hallmark rebuild for the Tauri desktop dashboard. The legacy `crates/app/ui/` was removed at the user’s request; `crates/app/ui-v2/` is the active frontend.

## Genre and structure
- Genre: modern-minimal, technical, professional.
- Macrostructure: Command Center / operational workbench.
- Information architecture: persistent local-machine rail, overview metrics, project work surface, runtime controls, package management, and live activity log.
- Truthfulness: only local runtime state is shown. No fabricated teammates, avatars, sync status, or usage metrics.

## Tokens
- Paper: `oklch(15% 0.018 255)`
- Surfaces: `oklch(19% 0.018 255)` → `oklch(27% 0.026 255)`
- Ink: `oklch(96% 0.012 255)` with muted `oklch(68% 0.016 255)`
- Accent: restrained cobalt `oklch(72% 0.17 255)`
- Status: green running, amber partial, red destructive/error.
- Typography: Space Grotesk display, Inter body, JetBrains Mono data.
- Spacing: 4pt-derived tokens in `ui-v2/tokens.css`.

## Interaction
- Buttons, custom listboxes, dialogs, search, service controls, package installation, and port reset are wired to existing Tauri commands.
- Listboxes support mouse, Arrow keys, Home/End, Escape, focus return, and visible focus rings.
- Destructive actions use a confirmation dialog; async actions show quiet status toasts.
- Motion is limited to transform/opacity transitions and respects reduced motion.

## Responsive contract
Verified with mocked Tauri data at 1280, 768, 414, 375, and 320px. Root uses `overflow-x: clip`; mobile navigation becomes a compact row and work surfaces stack without horizontal scrolling.

## Runtime
`tauri.conf.json` points `frontendDist` to `ui-v2`. Existing backend commands and IDs remain stable. Team sync is intentionally not implemented because the app is local-only.
