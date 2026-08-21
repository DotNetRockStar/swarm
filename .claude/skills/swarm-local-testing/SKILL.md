---
name: swarm-local-testing
description: Use when manually testing SWARM end to end in the desktop media-server GUI and a real Fire TV. Covers run_now.sh, GUI-owned server lifecycle, managed activation and LAN pairing flows, deploy_tv.sh/logs, LAN URL and VPN pitfalls, persisted state, and debug-only cleartext networking.
---

# Running SWARM locally on a real TV

Use repo-root `./run_now.sh`. It builds and starts exactly two products:

- the internet-style SWARM rendezvous service locally; and
- one native media-server GUI, whose `ServerCore` is the media server.

There is no separate headless `swarm-serverd`. Starting a second media server
causes duplicate LAN discovery entries and tests the wrong product lifecycle.
Closing the GUI window hides it to the tray; **Quit SWARM** stops it. Ctrl+C in
the `run_now.sh` terminal stops both processes and frees their TCP/UDP ports.

```bash
./run_now.sh
```

The script passes `SWARM_RENDEZVOUS_URL` to the GUI. The media server creates
and owns its managed swarm automatically. `.run/` stores local rendezvous
SQLite state only; GUI settings, identity, library data, and media roots live
in the platform app-data directory and intentionally survive script restarts.

## Connect a TV

Managed activation is the normal remote-network flow:

1. On the TV choose **Add Server**, then **Show Code**.
2. Open the media server's **Swarm** tab.
3. Enter the TV's short-lived activation code and approve it.
4. The client stores the connection and opens the SWARM page.

The SWARM service transports activation and rendezvous, but users should not
need to know the implementation term STUN. The legacy account/swarm/join-code
web UI remains a compatibility fallback, not the first-run path.

For a same-LAN connection, the TV lists **Servers on LAN** automatically. Click
the server, choose **Pair a client** on the media-server Swarm page, and enter
its six-digit code on the TV. LAN transport takes precedence when the same
server is also available through its swarm and does not require the internet
rendezvous service after trust is established.

## LAN URL and VPN diagnosis

The script prints local and LAN SWARM-service URLs. `127.0.0.1` on a TV means
the TV itself; use the printed LAN address from another device. A full-tunnel
VPN can make a routing probe select a `utun` address, so the script prefers
physical interfaces (`en0`, `en1`, `eth0`). If it still selects the wrong one,
inspect `ifconfig`/the default route and prove reachability from another LAN
device rather than trusting a plausible IP.

```bash
curl -s -o /dev/null -w "%{http_code}\n" http://<LAN_IP>:8080/health
```

## Real Fire TV deployment and logs

Use `./deploy_tv.sh [ip] [-f]` to build, install the correct ABI split,
force-stop the old app, launch the fully qualified activity, and poll for a
delayed crash. Use `./tv_logs.sh [ip]` for a live filtered log or `-d` for a
one-time dump. The TV needs ADB debugging enabled and its one-time authorization
prompt accepted.

Sideload debug builds for local HTTP/WSS testing. The debug source set permits
cleartext networking; the main/release manifest intentionally does not, for
Fire TV Appstore compliance. Never move that exception into release-visible
resources.

For launcher-icon work, assemble and install the APK: Fire TV masks/crops icons
differently from a desktop image viewer. For long playback tests, confirm the
screensaver stays suppressed while video or music is actively playing and is
allowed again after pause/close.

## Useful overrides

- `SWARM_STUN_PORT` (default `8080`): local rendezvous HTTP port.
- `SWARM_GUI_PEER_PORT` (default `8544`): GUI media-server QUIC port.
- `SWARM_RUN_DIR` (default `.run`): local rendezvous state directory.
- `RUST_LOG` (default `info`): raise to `debug` for signaling/punch issues.

Do not use the removed `SWARM_PEER_PORT`, `SWARM_MEDIA_ROOT`,
`SWARM_STUN_CODE`, or headless startup flow with `run_now.sh`; choose roots and
pair clients in the GUI.
