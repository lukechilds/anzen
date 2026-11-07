// Static files only: no keys, wallet endpoints, chain proxy, or application backend.
import http from 'node:http';
import { readFile } from 'node:fs/promises';
import { resolve, extname, sep } from 'node:path';
const root = resolve('dist'), port = Number(process.env.ANZEN_STATIC_PORT || 8083);
const types = { '.html': 'text/html', '.js': 'text/javascript', '.mjs': 'text/javascript', '.wasm': 'application/wasm', '.css': 'text/css', '.json': 'application/json', '.svg': 'image/svg+xml', '.png': 'image/png', '.ttf': 'font/ttf' };
const server = http.createServer(async (request, response) => {
  const headers = { 'Cross-Origin-Opener-Policy': 'same-origin', 'Cross-Origin-Embedder-Policy': 'require-corp', 'X-Content-Type-Options': 'nosniff', 'Cache-Control': 'no-cache', 'Referrer-Policy': 'no-referrer' };
  try {
    if (request.method !== 'GET' && request.method !== 'HEAD') { response.writeHead(405, headers); response.end(); return; }
    const path = resolve(root, '.' + decodeURIComponent(new URL(request.url, 'http://localhost').pathname));
    if (path !== root && !path.startsWith(root + sep)) throw new Error('Outside static root');
    const file = path === root ? resolve(root, 'index.html') : path;
    const data = await readFile(file);
    response.writeHead(200, { ...headers, 'Content-Type': types[extname(file)] || 'application/octet-stream' });
    response.end(request.method === 'HEAD' ? undefined : data);
  } catch { response.writeHead(404, headers); response.end('Not found'); }
});
server.listen(port, 'localhost', () => console.log(`Anzen static preview: http://localhost:${port}`));
