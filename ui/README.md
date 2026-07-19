# CRFty UI

React + TypeScript frontend for the CRFty Tauri shell (issue #36 is the
design record). Managed with pnpm.

```bash
pnpm install
pnpm dev        # Vite dev server
pnpm test       # vitest (includes Python-parity fixtures)
pnpm typecheck  # tsc -b
pnpm lint       # oxlint
pnpm build      # tsc -b && vite build
```

Parity fixtures for the ported formatters are regenerated with
`scripts/generate-parity-fixtures.py /path/to/main-checkout`.
