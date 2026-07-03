# SnapClone Relay Server

A tiny WebSocket relay for symmetric-NAT and restrictive-firewall scenarios.
Both peers connect outbound — no inbound ports needed on the relay server.

## Deploy (any Linux VPS, ~$5/mo)

```bash
# Install Node.js 18+
curl -fsSL https://deb.nodesource.com/setup_18.x | bash -
apt-get install -y nodejs

# Install relay
cd /opt
git clone ... snapclone
cd snapclone/relay-server
npm install

# Run (plain ws:// on port 9900)
node relay.js

# For production: put behind nginx for wss:// on port 443
```

## nginx config (recommended — wss:// on 443 passes through all firewalls)

```nginx
server {
    listen 443 ssl;
    server_name relay.yourdomain.com;
    
    ssl_certificate /etc/letsencrypt/live/relay.yourdomain.com/fullchain.pem;
    ssl_certificate_key /etc/letsencrypt/live/relay.yourdomain.com/privkey.pem;
    
    location /relay/ {
        proxy_pass http://127.0.0.1:9900;
        proxy_http_version 1.1;
        proxy_set_header Upgrade $http_upgrade;
        proxy_set_header Connection "upgrade";
        proxy_set_header Host $host;
        proxy_read_timeout 120s;
    }
}
```

## Configure in app

Settings → Network → Relay URL:
- Plain:  `ws://your-server-ip:9900`
- TLS:    `wss://relay.yourdomain.com` (recommended for public WiFi / corporate)

## Security

- End-to-end encrypted: relay sees only opaque AES-256-GCM ciphertext
- Session tokens: SHA-256(sorted peer IDs) — unique per peer pair
- No authentication needed: without both sc-IDs you cannot compute the token

## When is relay used?

1. Hole punch fails (symmetric NAT — AT&T mobile, many corporate carriers)
2. All P2P attempts exhausted after 10 retries
3. Both peers on same public WiFi with client isolation

## Scenarios covered by relay

| Scenario | Without relay | With relay |
|----------|---------------|------------|
| Both on home WiFi | ✓ LAN direct | ✓ |
| Home WiFi ↔ AT&T CGNAT | ✗ hole punch fails | ✓ |
| Corporate WiFi (UDP blocked) | ✗ STUN fails | ✓ (wss://443) |
| Public WiFi with client isolation | ✗ | ✓ |
| Two different countries | ✓ hole punch (most ISPs) | ✓ |
