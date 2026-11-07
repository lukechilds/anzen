// Expo handles its HTML before Metro's enhanceMiddleware. A loopback-only front proxy
// applies WASM isolation headers consistently to HTML, static files and bundle responses.
import http from 'node:http';
import net from 'node:net';
import { spawn } from 'node:child_process';
import { dirname, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';

const mobile = resolve(dirname(fileURLToPath(import.meta.url)), '..');
const server = http.createServer((request, response) => {
  const headers = {
    'cross-origin-opener-policy': 'same-origin',
    'cross-origin-embedder-policy': 'require-corp',
  };
  const upstream = http.request({ hostname: 'localhost', port: 8082, path: request.url, method: request.method, headers: request.headers }, incoming => {
    response.writeHead(incoming.statusCode || 502, { ...incoming.headers, ...headers });
    incoming.pipe(response);
  });
  upstream.on('error', () => { if (!response.headersSent) response.writeHead(503, { ...headers, 'Content-Type': 'text/plain' }); response.end('Anzen web server is starting. Refresh in a moment.'); });
  request.on('aborted', () => upstream.destroy());
  request.pipe(upstream);
});
server.on('upgrade', (request, socket, head) => {
  const upstream = net.connect(8082, 'localhost', () => {
    upstream.write(`${request.method} ${request.url} HTTP/${request.httpVersion}\r\n${request.rawHeaders.reduce((text, value, i, all) => i % 2 === 0 ? text + `${value}: ${all[i + 1]}\r\n` : text, '')}\r\n`);
    if (head.length) upstream.write(head);
    socket.pipe(upstream).pipe(socket);
  });
  upstream.on('error', () => socket.destroy()); socket.on('error', () => upstream.destroy());
  socket.on('close', () => upstream.destroy());
});
let child, stopping = false;
function stop(signal = 'SIGTERM') {
  if (stopping) return;
  stopping = true; child?.kill(signal); server.close(); server.closeAllConnections();
}
server.on('error', error => { console.error(error.message); stop(); process.exitCode = 1; });
server.listen(8081, 'localhost', () => {
  console.log('Anzen browser UI: http://localhost:8081 (isolated WASM workers)');
  child = spawn(process.execPath, [resolve(mobile, 'node_modules/expo/bin/cli'), 'start', '--web', '--port', '8082', '--localhost'], { cwd: mobile, stdio: 'inherit', env: process.env });
  child.on('error', error => { console.error(error.message); stop(); process.exitCode = 1; });
  child.on('exit', code => { stop(); process.exitCode = code || 0; });
});
process.on('SIGINT', () => stop('SIGINT'));
process.on('SIGTERM', () => stop());
