---
name: swarm-contract-fixtures
description: Use when porting or changing a wire type shared between swarm-core (Rust) and the Kotlin :core module (rest/, peer/, signal/, capability/ packages) - mirroring a struct/enum by hand and verifying its JSON shape against real serde_json output instead of guessing.
---

# Mirroring a Rust wire contract into Kotlin

`swarm-core` is the single source of truth for every wire type (REST, the
peer QUIC protocol, WSS signaling). The Kotlin `:core` module hand-mirrors
these types field-for-field — there is no code generator. Every mirrored
type in this codebase was verified against **real** `serde_json::to_string`
output, not hand-guessed JSON. This is not optional polish: it already
caught a real bug (`ByteRange`, see below) that guessing would have missed.

## Why guessing fails

Serde's default enum representation depends on annotations that are easy to
misjudge from the Rust source alone:

- `#[serde(tag = "type", rename_all = "snake_case")]` on an enum produces
  **adjacently tagged** JSON: `{"type":"hello","field":...}` — fields
  flattened alongside the tag in one object. This matches
  kotlinx.serialization's own default sealed-class shape, so a plain
  `@Serializable sealed class` + `@SerialName` per variant is enough (see
  `signal/Contracts.kt`'s `SignalMessage`/`SignalPayload` — note
  `SignalPayload` needs `@JsonClassDiscriminator("kind")` since its tag key
  isn't the default `"type"`).
- An enum with **no** explicit `#[serde(tag = ...)]` (struct variants) is
  **externally tagged** by default: `{"from_to": {"start":1,"end":2}}` —
  the variant name becomes an outer key wrapping the fields. This does
  *not* match kotlinx.serialization's default and needs a hand-written
  `KSerializer` (see `peer/Contracts.kt`'s `ByteRange`/`ByteRangeSerializer`
  — this exact mismatch was found and fixed this way).
- `#[serde(skip_serializing_if = "Option::is_none")]` omits the field
  entirely when `None`, not just serializes `null`. Kotlin's mirror needs
  `explicitNulls = false` (already set on `SwarmJson`, `:core/rest/SwarmJson.kt`)
  plus a default value (`= null`) on the property so decode tolerates the
  field being absent.
- `#[serde(rename_all = "snake_case")]` converts *variant names* to
  snake_case for enum/sealed-class discriminator values — kotlinx's
  `JsonNamingStrategy.SnakeCase` (also on `SwarmJson`) only touches
  *property* names, never enum entries or sealed-class discriminators.
  Every enum entry and sealed-class variant needs an explicit
  `@SerialName("snake_case_value")`.

Don't try to resolve these by re-reading the Rust source extra carefully —
capture the real output and compare against it.

## Procedure

1. **Identify the exact Rust type(s)** in `crates/swarm-core/src/{rest,peer,signal,capability}.rs`.

2. **Capture real fixtures.** Add a throwaway example binary:

   ```
   crates/swarm-core/examples/fixture_printer.rs
   ```

   Construct one value per variant/interesting case (including edge cases:
   an `Option` field both `Some` and `None`, an empty `Vec`, etc.) and
   `println!("{label}: {}", serde_json::to_string(&value).unwrap())` for
   each. Run it:

   ```bash
   export PATH="$HOME/.rustup/toolchains/stable-aarch64-apple-darwin/bin:$PATH"
   cargo run --example fixture_printer -p swarm-core
   ```

   Copy the exact printed strings — these are your test fixtures, verbatim.

3. **Delete the example immediately after capturing output.** It's
   scratch, not part of the crate (`crates/swarm-core/examples/` has never
   had a tracked file in it — `git status` should show nothing after
   deleting). Never commit it.

4. **Write the Kotlin data class(es)** in the matching `:core` package,
   matching the Rust module layout (`swarm_core::signal` → `:core/signal`,
   etc.). Use `SwarmJson` (`:core/rest/SwarmJson.kt`) for all
   encode/decode — never a bare `Json { }` instance, or the naming
   strategy and null-handling config won't match.

5. **Write a `ContractsTest.kt` in the mirrored package's test
   directory**, one test per fixture, asserting *both* directions:

   ```kotlin
   private fun roundtrip(value: SignalMessage, json: String) {
       assertEquals(value, SwarmJson.decodeFromString<SignalMessage>(json))
       assertEquals(json, SwarmJson.encodeToString(value))
   }
   ```

   Decode-only isn't enough — it can pass with a wrong `@SerialName` if the
   value happens to still parse; asserting the *encoded* string matches the
   captured fixture exactly catches that.

6. Run `./gradlew :core:test --tests "*ContractsTest*"` (fast, no
   subprocess needed) and confirm every fixture round-trips.

## Real precedent in this repo

- `crates/swarm-core/src/rest.rs` ↔ `core/src/main/kotlin/.../rest/Contracts.kt`
  + `ContractsTest.kt`
- `crates/swarm-core/src/peer.rs` ↔ `.../peer/Contracts.kt` + `ContractsTest.kt`
  (the `ByteRange` externally-tagged case)
- `crates/swarm-core/src/signal.rs` + `capability.rs` ↔ `.../signal/Contracts.kt`,
  `.../capability/Contracts.kt` + `signal/ContractsTest.kt` (the adjacently-tagged,
  custom-discriminator-key case)
