# Integration harness (Phase 1+)

Docker-composed multi-node test environment, patterned after the Batocera
federation's `.github/scripts/swarm-*.sh`:

- `stun-server` + `caddy`
- 2 headless server nodes (swarm-media + swarm-p2p + swarm-stun-client)
- 1 headless client node
- Simulated NATed networks (per-node bridge networks + NAT containers) for the
  Phase 4 hole-punch matrix.

Driver exercises: register → presence → signal/punch → manifest sync → stream
→ seek, asserting byte-exactness (fingerprint the received bytes) and timing
budgets. Runs in CI on changes to `crates/` or `apps/stun-server`.
