#!/usr/bin/env node
// SnapClone WebSocket relay server
// Pairs two peers by session token and forwards all frames bidirectionally.
//
// Deployment modes (set via env vars):
//
//   Plain WS only (default, for testing):
//     node relay.js
//
//   TLS (wss://) — recommended for production, gets through HTTPS firewalls:
//     TLS_CERT=/etc/letsencrypt/live/example.com/fullchain.pem \
//     TLS_KEY=/etc/letsencrypt/live/example.com/privkey.pem \
//     PORT=443 node relay.js
//
//   Behind nginx (recommended for port 443 sharing):
//     Leave relay on PORT=9900 (plain ws://) and let nginx proxy_pass to it.
//     nginx handles TLS termination; relay stays plain.
//     See relay-server/nginx.conf for a ready-made config.
//
// Clients with a wss:// relay URL work against TLS mode or nginx TLS proxy.
// Clients with a ws:// relay URL work against plain mode or nginx plain proxy.
//
// Install: npm install ws
// Run:     node relay.js

'use strict';
const http  = require('http');
const https = require('https');
const fs    = require('fs');
const { WebSocketServer } = require('ws');

const PORT     = parseInt(process.env.PORT || '9900', 10);
const TLS_CERT = process.env.TLS_CERT || '';
const TLS_KEY  = process.env.TLS_KEY  || '';
const useTls   = TLS_CERT && TLS_KEY;

const sessions = new Map(); // token → { ws, timer }

// ── Connection handler (shared between ws and wss servers) ────────────────────
function onConnection(ws, req) {
    const token = (req.url || '').replace('/relay/', '').split('?')[0].trim();
    if (!token || token.length < 8) {
        ws.close(1008, 'bad token');
        return;
    }

    const waiting = sessions.get(token);
    if (waiting && waiting.ws.readyState === 1 /* OPEN */) {
        // Second peer arrived — wire bidirectionally and clear the wait timer.
        clearTimeout(waiting.timer);
        sessions.delete(token);
        const peer = waiting.ws;
        console.log(`[relay] paired  ${token.slice(0, 16)}  (${req.socket.remoteAddress})`);

        const fwd = (src, dst) =>
            src.on('message', (data, isBinary) => {
                if (dst.readyState === 1) dst.send(data, { binary: isBinary });
            });
        fwd(ws, peer);
        fwd(peer, ws);

        const cleanup = () => {
            try { ws.close();   } catch (_) {}
            try { peer.close(); } catch (_) {}
        };
        ws.on('close',   cleanup);
        peer.on('close', cleanup);
        ws.on('error',   cleanup);
        peer.on('error', cleanup);
    } else {
        // First peer — wait up to 90s for the other side.
        console.log(`[relay] waiting ${token.slice(0, 16)}  (${req.socket.remoteAddress})`);
        const timer = setTimeout(() => {
            if (sessions.get(token)?.ws === ws) {
                sessions.delete(token);
                try { ws.close(1001, 'timeout'); } catch (_) {}
                console.log(`[relay] timeout ${token.slice(0, 16)}`);
            }
        }, 90_000);
        sessions.set(token, { ws, timer });

        const cleanup = () => {
            if (sessions.get(token)?.ws === ws) {
                clearTimeout(timer);
                sessions.delete(token);
            }
        };
        ws.on('close', cleanup);
        ws.on('error', cleanup);
    }
}

// ── Create HTTP(S) server ─────────────────────────────────────────────────────
let server;
if (useTls) {
    const tlsOpts = {
        cert: fs.readFileSync(TLS_CERT),
        key:  fs.readFileSync(TLS_KEY),
    };
    server = https.createServer(tlsOpts, (req, res) => {
        res.writeHead(200); res.end('SnapClone relay OK\n');
    });
    console.log(`[relay] TLS mode  cert=${TLS_CERT}`);
} else {
    server = http.createServer((req, res) => {
        res.writeHead(200); res.end('SnapClone relay OK\n');
    });
    console.log('[relay] plain WS mode (set TLS_CERT + TLS_KEY for wss://)');
}

const wss = new WebSocketServer({ server });
wss.on('connection', onConnection);

server.listen(PORT, '0.0.0.0', () =>
    console.log(`[relay] listening on ${useTls ? 'wss' : 'ws'}://0.0.0.0:${PORT}`));

// ── Periodic stats ────────────────────────────────────────────────────────────
setInterval(() => {
    if (sessions.size > 0)
        console.log(`[relay] waiting sessions: ${sessions.size}`);
}, 60_000);
