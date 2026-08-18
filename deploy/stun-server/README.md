# Containerized SWARM STUN server

This Compose project runs the public SWARM rendezvous service and Caddy. Caddy
terminates HTTPS/WSS on TCP 443; the server receives reflector traffic directly
on UDP 443 and UDP 3478. SQLite and Caddy state live in named Docker volumes.

## Prerequisites

- Docker Engine with the Compose v2 plugin
- A public host with TCP 80/443 and UDP 443/3478 allowed through both the cloud
  firewall and the host firewall
- A DNS A record (and AAAA only when IPv6 is configured) pointing at the host

TCP 80 is required for Caddy's initial ACME HTTP challenge and HTTP-to-HTTPS
redirect. Do not put the UDP ports behind an HTTP reverse proxy.

## Configure and start

From this directory:

```sh
cp .env.example .env
# Edit .env and set SWARM_DOMAIN before continuing.
docker compose config
docker compose build --pull stun-server
docker compose up -d
docker compose ps
```

After DNS and certificate issuance settle, verify the service:

```sh
# Replace the example hostname with the SWARM_DOMAIN value from .env.
curl --fail "https://swarm.example.com/health"
docker compose logs --tail=100 stun-server caddy
```

Open `https://<your-domain>/api/docs` for Swagger or the domain root for the
account UI.

## Configuration

The committed `.env.example` documents every deployment setting. Keep the real
`.env` file on the host; it is ignored by both Git and the Docker build context.
SMTP is optional. Without it, verification and password-reset links are written
to the server log for development and operator-assisted use.

The container sets these stable internal paths and listeners:

| Setting | Container value |
|---|---|
| `SWARM_DATABASE_PATH` | `/data/swarm.sqlite` |
| `SWARM_HTTP_BIND` | `0.0.0.0:8080` |
| `SWARM_PUBLIC_URL` | `https://${SWARM_DOMAIN}` |
| `SWARM_REFLECTOR_PORTS` | `9443,3478` |
| `SWARM_STATIC_DIR` | `/opt/swarm/static` |

The Compose port mapping exposes internal UDP 9443 as public UDP 443. The
fallback reflector uses UDP 3478 on both sides.

## Data and operations

- `stun-data` contains SQLite, including its WAL and shared-memory files.
- `caddy-data` contains certificates and private keys.
- `caddy-config` contains Caddy runtime state.

Inspect and update the deployment with:

```sh
docker compose logs -f stun-server
docker compose build --pull stun-server
docker compose up -d
```

Back up `stun-data` with a SQLite-aware backup or with the service stopped. Do
not copy only `swarm.sqlite` while the container is running because committed
transactions may still be present in its WAL file.

To stop without deleting data:

```sh
docker compose down
```

Do not add `--volumes` unless the SQLite database and Caddy state are intended
to be permanently deleted.
