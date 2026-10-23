# Frontend

React/TypeScript/Vite UI with TanStack Query, Tailwind, Monaco and xterm.
Node.js 22.12+ and npm are required. From `frontend/`:

```sh
npm ci
npm run typecheck
npm test
npm run build
npm run dev
```

Start the [backend](../backend/README.md) separately for browser development.
Open `http://127.0.0.1:5173` and enter its runtime token. It stays in the tab's
memory; reloading requires re-entry. `AWM_PORT` must match the backend when using
a nondefault port. Never put credentials in `VITE_*` variables or URLs.

The local gateway validates Host/Origin and forwards relative `/api/` requests to
the loopback backend. `npm run preview` uses the same gateway after a build; stop
the dev server first. Neither command is a production hosting setup.
Desktop uses typed Tauri IPC and embeds the production frontend from `dist/`.
