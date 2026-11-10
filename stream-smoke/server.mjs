#!/usr/bin/env node
import http from 'node:http';
import fs from 'node:fs';
import net from 'node:net';
import path from 'node:path';
import { fileURLToPath } from 'node:url';

const __dirname = path.dirname(fileURLToPath(import.meta.url));
const port = Number(process.env.REACT_STREAM_SMOKE_PORT || process.env.PORT || 8787);
const host = process.env.REACT_STREAM_SMOKE_HOST || '127.0.0.1';
const twitchChannel = normalizeChannel(process.env.TWITCH_CHANNEL || process.env.REACT_TWITCH_CHANNEL || '');
const logPath = process.env.REACT_STREAM_SMOKE_LOG || path.join(process.cwd(), 'stream-smoke-events.jsonl');
const maxEvents = Number(process.env.REACT_STREAM_SMOKE_MAX_EVENTS || 100);
const history = [];
const clients = new Set();
let twitchSocket = null;
let twitchReconnectTimer = null;
let twitchBackoffMs = 1000;

function normalizeChannel(value) {
  return String(value || '').trim().replace(/^#/, '').replace(/^@/, '').toLowerCase();
}

function json(res, status, payload) {
  const body = JSON.stringify(payload);
  res.writeHead(status, {
    'content-type': 'application/json; charset=utf-8',
    'content-length': Buffer.byteLength(body),
    'cache-control': 'no-store',
  });
  res.end(body);
}

function readBody(req, limit = 16 * 1024) {
  return new Promise((resolve, reject) => {
    let body = '';
    req.setEncoding('utf8');
    req.on('data', (chunk) => {
      body += chunk;
      if (body.length > limit) {
        reject(new Error('request body too large'));
        req.destroy();
      }
    });
    req.on('end', () => resolve(body));
    req.on('error', reject);
  });
}

function parseBody(raw, contentType = '') {
  if (!raw) return {};
  if (contentType.includes('application/json')) return JSON.parse(raw);
  const params = new URLSearchParams(raw);
  return Object.fromEntries(params.entries());
}

function eventFromPayload(payload, fallbackKind) {
  const msg = String(payload.msg || payload.message || payload.body || '').trim();
  if (!msg) throw new Error('msg is required');
  return {
    id: `${Date.now()}-${Math.random().toString(36).slice(2, 8)}`,
    ts: new Date().toISOString(),
    source: String(payload.source || fallbackKind || 'linus').slice(0, 40),
    kind: String(payload.kind || fallbackKind || 'status').slice(0, 40),
    user: String(payload.user || payload.username || payload.source || 'Linus').slice(0, 80),
    msg: msg.slice(0, 500),
  };
}

function publish(event) {
  history.push(event);
  while (history.length > maxEvents) history.shift();
  fs.mkdirSync(path.dirname(logPath), { recursive: true });
  fs.appendFileSync(logPath, `${JSON.stringify(event)}\n`, 'utf8');
  const frame = `id: ${event.id}\nevent: message\ndata: ${JSON.stringify(event)}\n\n`;
  for (const res of clients) res.write(frame);
}

function serveStatic(res, fileName, contentType) {
  const filePath = path.join(__dirname, 'public', fileName);
  const body = fs.readFileSync(filePath);
  res.writeHead(200, {
    'content-type': contentType,
    'content-length': body.length,
    'cache-control': 'no-store',
  });
  res.end(body);
}

function handleEvents(req, res) {
  res.writeHead(200, {
    'content-type': 'text/event-stream; charset=utf-8',
    'cache-control': 'no-cache, no-transform',
    connection: 'keep-alive',
    'x-accel-buffering': 'no',
  });
  res.write(': connected\n\n');
  for (const event of history) {
    res.write(`id: ${event.id}\nevent: message\ndata: ${JSON.stringify(event)}\n\n`);
  }
  clients.add(res);
  req.on('close', () => clients.delete(res));
}

function connectTwitch() {
  if (!twitchChannel || twitchSocket || twitchReconnectTimer) return;
  twitchSocket = net.createConnection({ host: 'irc.chat.twitch.tv', port: 6667 }, () => {
    twitchBackoffMs = 1000;
    const nick = `justinfan${Math.floor(Math.random() * 90000) + 10000}`;
    twitchSocket.write(`NICK ${nick}\r\n`);
    twitchSocket.write(`JOIN #${twitchChannel}\r\n`);
    publish({
      id: `${Date.now()}-twitch-join`,
      ts: new Date().toISOString(),
      source: 'system',
      kind: 'status',
      user: 'Linus',
      msg: `Twitch chat connected to #${twitchChannel}`,
    });
  });

  twitchSocket.setEncoding('utf8');
  twitchSocket.on('data', (chunk) => {
    for (const line of chunk.split(/\r?\n/)) {
      if (!line) continue;
      if (line.startsWith('PING')) {
        twitchSocket.write(line.replace('PING', 'PONG') + '\r\n');
        continue;
      }
      const match = line.match(/^:([^!]+)!.* PRIVMSG #[^ ]+ :(.+)$/);
      if (match) {
        publish({
          id: `${Date.now()}-${Math.random().toString(36).slice(2, 8)}`,
          ts: new Date().toISOString(),
          source: 'twitch',
          kind: 'chat',
          user: match[1].slice(0, 80),
          msg: match[2].slice(0, 500),
        });
      }
    }
  });
  twitchSocket.on('error', (err) => {
    publish({
      id: `${Date.now()}-twitch-error`,
      ts: new Date().toISOString(),
      source: 'system',
      kind: 'error',
      user: 'Linus',
      msg: `Twitch chat error: ${err.message}`,
    });
  });
  twitchSocket.on('close', () => {
    twitchSocket = null;
    twitchReconnectTimer = setTimeout(() => {
      twitchReconnectTimer = null;
      twitchBackoffMs = Math.min(twitchBackoffMs * 2, 30000);
      connectTwitch();
    }, twitchBackoffMs);
  });
}

const server = http.createServer(async (req, res) => {
  try {
    const url = new URL(req.url || '/', `http://${req.headers.host || `${host}:${port}`}`);
    if (req.method === 'GET' && (url.pathname === '/' || url.pathname === '/overlay')) {
      return serveStatic(res, 'overlay.html', 'text/html; charset=utf-8');
    }
    if (req.method === 'GET' && url.pathname === '/events') return handleEvents(req, res);
    if (req.method === 'GET' && url.pathname === '/healthz') {
      return json(res, 200, { ok: true, clients: clients.size, events: history.length, twitchChannel: twitchChannel || null });
    }
    if (req.method === 'POST' && (url.pathname === '/overlay-message' || url.pathname === '/chat-test')) {
      const payload = parseBody(await readBody(req), req.headers['content-type'] || '');
      const event = eventFromPayload(payload, url.pathname === '/chat-test' ? 'chat' : 'status');
      publish(event);
      return json(res, 200, { ok: true, event });
    }
    json(res, 404, { ok: false, error: 'not found' });
  } catch (err) {
    json(res, 400, { ok: false, error: err.message });
  }
});

server.listen(port, host, () => {
  console.log(`REACT stream smoke overlay: http://${host}:${port}/overlay`);
  console.log(`SSE events: http://${host}:${port}/events`);
  console.log(`Message endpoint: POST http://${host}:${port}/overlay-message {"user":"Linus","msg":"..."}`);
  if (twitchChannel) connectTwitch();
  else console.log('Twitch chat disabled until TWITCH_CHANNEL is set. Use /chat-test for local smoke.');
});
