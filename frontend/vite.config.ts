import { defineConfig, type Plugin, type ProxyOptions } from 'vite';
import react from '@vitejs/plugin-react';
import tailwindcss from '@tailwindcss/vite';
import type { IncomingMessage, ServerResponse } from 'node:http';

// Local gateway: validate the browser Origin BEFORE adapting it for the Rust API.
// No credentials in configuration; Authorization is passed through from the UI.
const backendPort = Number(process.env.AWM_PORT ?? 43871);
if (!Number.isInteger(backendPort) || backendPort < 1 || backendPort > 65535) throw new Error('Invalid AWM_PORT');
const target = `http://127.0.0.1:${backendPort}`;
const frontendPort = 5173;
const hosts = new Set([`127.0.0.1:${frontendPort}`, `localhost:${frontendPort}`]);
function guard(req: IncomingMessage, res: ServerResponse, next: () => void) {
  if (!req.url?.startsWith('/api/')) return next();
  const origins = req.rawHeaders.filter((_, i, all) => i % 2 === 1 && all[i - 1].toLowerCase() === 'origin');
  if (!hosts.has(req.headers.host ?? '') || origins.length > 1 ||
      (origins.length === 1 && ![...hosts].some(host => origins[0] === `http://${host}`))) {
    res.writeHead(403, { 'Content-Type': 'application/json' });
    res.end(JSON.stringify({ error: { code: 'forbidden_origin', message: 'Local UI origin required' } }));
    return;
  }
  next();
}
const gateway: Plugin = {
  name: 'local-api-origin',
  configureServer(server) { server.middlewares.use(guard); },
  configurePreviewServer(server) { server.middlewares.use(guard); },
};
const proxy: Record<string, ProxyOptions> = {
  '/api/': {
    target, changeOrigin: true,
    configure(server) {
      server.on('proxyReq', (request, incoming) => {
        if (incoming.headers.origin) request.setHeader('Origin', target);
      });
      server.on('error', (_error, _request, response) => {
        if (response && 'writeHead' in response && !response.headersSent) {
          response.writeHead(502, { 'Content-Type': 'application/json' });
          response.end(JSON.stringify({ error: { code: 'backend_disconnected', message: 'Rust backend unavailable' } }));
        }
      });
      // Never log request headers (in particular Authorization).
    },
  },
};
export default defineConfig({
  plugins: [gateway, react(), tailwindcss()],
  server: { host: '127.0.0.1', port: frontendPort, strictPort: true, proxy },
  preview: { host: '127.0.0.1', port: frontendPort, strictPort: true, proxy },
});
