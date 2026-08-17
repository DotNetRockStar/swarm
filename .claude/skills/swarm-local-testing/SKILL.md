---
name: swarm-local-testing
description: Use when the user wants to manually try out SWARM end-to-end (browser, curl, or a real device like a Fire TV) rather than run automated tests - covers run_now.sh, the LAN-vs-local URL distinction, the VPN interface gotcha, and the join-code linking flow.
---

# Running SWARM locally for manual/real-device testing

For manual, hands-on testing (as opposed to automated `cargo test`/
`./gradlew test` — see `swarm-interop-test` for those), use the repo-root
`run_now.sh` script rather than hand-starting binaries. It runs the real
`swarm-stun-server` + `swarm-serverd` binaries, bound so a real device on
the LAN (a Fire TV, a phone, another computer) can actually reach them —
not just this machine.

```bash
./run_now.sh
```

State (SQLite DBs, the media root) lives in gitignored `.run/` next to the
script, so re-runs are stateful across restarts unless you delete that
directory. Ctrl+C stops both processes cleanly.

## Why "LAN" vs "local" URL matters

The script prints two URLs:
```
local  http://127.0.0.1:8080     (browser on this machine only)
LAN    http://192.168.x.x:8080   <- use this one for any other device
```
**Always give the Fire TV client (or any other real device) the LAN URL.**
127.0.0.1 on the TV means the TV itself, not this dev machine — a
same-device-only mistake that looks like a network failure but isn't one.

## The VPN gotcha — read this if the printed LAN IP looks wrong

If this machine has a full-tunnel VPN active (Tailscale, corporate VPN,
WireGuard, etc.), the *naive* way to detect "my LAN IP" — open a UDP
socket, `connect()` to a well-known address with no packet actually sent,
read back the local address the OS picked — returns the **VPN tunnel's**
internal address (e.g. `10.2.0.2` via a `utunN` interface), because the
VPN captured the default route. That address is real but useless here: no
device on your physical Wi-Fi can reach it.

This is exactly the same trick `swarm_p2p::local_addr::detect_local_ipv4`
uses for the real server's `peer_addr` self-report — so **this is a real,
documented, unfixed limitation of the actual Rust server too**, not just a
shell-script quirk. A production SWARM server running behind a full-tunnel
VPN will self-report an address nothing on the LAN can dial. It's
understood and written down, not silently ignored — see the "Ongoing/
ignored limitations" note below and don't rediscover it as a surprise.

`run_now.sh`'s `detect_lan_ip()` works around this *for local manual
testing* by checking named physical interfaces first:
```bash
for iface in en0 en1 eth0; do
    ip="$(ipconfig getifaddr "$iface" 2>/dev/null)" && [ -n "$ip" ] && { echo "$ip"; return; }
done
# falls back to the UDP-probe trick only if none of those exist
```
If the printed LAN URL still looks wrong (e.g. you're on `en1` not `en0`,
or a different platform's interface naming), check `ifconfig` and
`route -n get default` to find which interface actually owns the default
route, and confirm with a real reachability test — don't assume, curl it:
```bash
curl -s -o /dev/null -w "%{http_code}\n" http://<LAN_IP>:8080/
```
A `200` (or any real HTTP response) confirms it's actually reachable, not
just plausible-looking.

## First-run linking flow

1. Run `./run_now.sh`, open the printed **local** URL in a browser on this
   machine (Swagger UI lives at `/api/docs` if you want to drive the API
   directly instead).
2. Register an account, create a swarm, mint a join code (8-digit,
   single-use, ~15 min TTL).
3. Link the media server to that swarm — two options:
   - Stop the script (Ctrl+C) and re-run with the code baked in so the
     headless daemon auto-registers on startup:
     ```bash
     SWARM_STUN_URL=http://<LAN_IP>:8080 SWARM_STUN_CODE=<code> ./run_now.sh
     ```
   - Or use the Tauri GUI (`cargo run -p swarm-server --features gui --bin
     swarm-server-app`) and paste the code into its UI instead of the
     headless daemon.
4. Drop real media files into `.run/media/` (or whatever `SWARM_RUN_DIR`
   points at) — the server scans this as its media root.
5. Point the Fire TV client (or `PunchConnectInteropTest`-style manual
   client) at the **LAN** STUN URL from step 1/3.

## Useful env var overrides

`SWARM_STUN_PORT` (default 8080), `SWARM_PEER_PORT` (default 8543),
`SWARM_RUN_DIR` (default `.run` — point this elsewhere to keep multiple
independent local swarms side by side), `RUST_LOG` (default `info`,
bump to `debug` for signaling/punch troubleshooting).

## Ongoing/ignored limitations worth remembering here

- No UPnP/NAT-PMP yet (the "forwarded" candidate kind exists in the
  protocol but nothing populates it) — cross-network testing beyond
  simple hole-punch isn't possible yet.
- No real Fire TV device has been used against this script in this
  session's work — all client-side UI work has been compile/manifest-
  verified only, not hands-on-hardware verified.
- Whether kwik accepts a real non-exportable `AndroidKeyStore` private key
  (as opposed to the in-memory test keys every automated test uses) is
  unverified — this is the first thing a real-device test would exercise
  that no automated test currently covers.
