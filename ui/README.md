# CRFty UI

React + TypeScript frontend for the CRFty Tauri shell (issue #36 is the
design record). Managed with pnpm.

```bash
pnpm install
pnpm dev           # Vite dev server (polls for changes — WSL2 /mnt/c)
pnpm test          # vitest (includes Python-parity fixtures)
pnpm typecheck     # tsc -b
pnpm lint          # oxlint
pnpm format        # oxfmt (format:check in CI)
pnpm build         # tsc -b && vite build
```

## Layout

- `src/components/ui/` — shadcn (Base UI) primitives after the house pass:
  token colors only, house radius/density, reviewed in both themes.
- `src/components/layout/` — sidebar shell and the `VIEWS`/`DEV_VIEWS` registries.
- `src/features/<view>/` — one folder per view; empty states only until the
  generated bindings land (no hand-authored IPC/domain types, issue #33).
- `src/lib/format/` — display formatters ported from the Python app;
  parity enforced by fixtures regenerated with
  `scripts/generate-parity-fixtures.py /path/to/main-checkout`.
- `src/dev/` — dev-gated (never in release bundles): the kitchen sink
  (token/primitive gallery + approved view mockups for queue and
  statistics) and the winning drag-spike reference (`@dnd-kit/react`).

Design decisions and mockup verdicts are recorded on issue #36 — read the
D6 (drag), D7 (statistics), and D11 (queue) comments before touching those
areas.
