# AGENTS.md

> A field manual for AI agents (and humans) to understand, run, profile, and improve this project quickly and safely.

## TL;DR

- **Goal:** Search a given **hex private-key range** for a Bitcoin **P2PKH** address match.
- **GPU role:** WGSL shaders emit candidates (`shaders/seq.wgsl`) and hash them (`shaders/hash160.wgsl`) to report matches.
  CPU (Rust) derives pubkeys with **libsecp256k1** and compares **HASH160** to the target.
- **Binary:** `gpu-bitcrack`
- **Fast start:**
  ```bash
  cargo build --release
  ./target/release/gpu-bitcrack 200000:3fffff 1CfZWK1QTQE3eS9qn61dQjV89KDjZzfNcv --batch 1000000
  ```
- **Run tests sequentially (GPU shared):**
  ```bash
  cargo test -j 1 -- --test-threads=1 --ignored
  ```
  Or mark GPU tests with `serial_test` and run normally.

---

## Repository map

```
gpu-bitcrack/
├─ Cargo.toml
├─ shaders/
│  ├─ seq.wgsl            # WGSL compute: emits n sequential 256-bit scalars (LE words)
│  └─ hash160.wgsl       # WGSL compute: SHA-256+RIPEMD-160 filter
└─ src/
   └─ main.rs             # CLI, GPU dispatch, CPU verify (secp256k1 -> P2PKH)
tests/
└─ integration_search.rs  # rstest/assert_cmd functional tests (may be ignored/serialized)
```

### Key crates
- `wgpu` (Metal/Vulkan/DX12) – GPU compute
- `secp256k1` – elliptic curve ops (CPU)
- `sha2`, `ripemd` – HASH160
- `bs58` – Base58Check
- `rayon` – CPU parallelism
- `clap` – CLI
- `bytemuck` – POD/Zeroable for GPU params

---

## How it works (current design)

1. **Parse input**
    - Keyspace: `START:END` (hex). Accepts odd-length hex by **left-padding one zero**.
    - Address: P2PKH Base58 (version byte `0x00`) → decode to **HASH160 (20 bytes)**.

2. **GPU batch generation**
    - Uniform `Params` includes the 256‑bit `start` (8×`u32`, little‑endian) and `n` (batch size).
    - Kernel writes `n` scalars: `start + idx`, each as 8×`u32` in **little‑endian** limbs.

3. **CPU verification**
    - For each 32‑byte LE scalar: convert to **big‑endian** (libsecp expects BE).
    - Skip invalid scalars (0 or ≥ curve order).
    - Derive **compressed pubkey (33B)** → `HASH160` → compare with target.
    - If any match → print **FOUND** with `address`, `WIF (compressed)`, and `priv_hex`.

4. **Iteration**
    - Advance the cursor by `batch`, repeat until end of range; otherwise **Not found**.

**Why this split?** Implementing full EC scalar multiplication + Base58 on GPU is complex; this keeps correctness high and still leverages the GPU for high-throughput candidate generation. Next steps move more hashing to GPU and overlap work for throughput.

---

## Build, run, test

### Build
```bash
# macOS: install Xcode CLT for Metal
xcode-select --install

cargo build --release
```

### Run
```bash
./target/release/gpu-bitcrack <START:END-hex> <Base58-P2PKH> [--batch <N>]
# Example (known hit):
./target/release/gpu-bitcrack 200000:3fffff 1CfZWK1QTQE3eS9qn61dQjV89KDjZzfNcv --batch 1000000
# Example (likely miss):
./target/release/gpu-bitcrack 1000000:1ffffff 15JhYXn6Mx3oF4Y7PcTAv2wVVAuCFFQNiP
```

- `--batch` controls candidates per GPU dispatch (default `1_000_000`). Memory ≈ `batch * 32` bytes (e.g., 1e6 → ~32 MiB).

### Tests

Integration tests use `rstest` + `assert_cmd`. GPU functional tests can be heavy:

- **Run all sequentially:**
  ```bash
  cargo test -j 1 -- --test-threads=1 --ignored
  ```
- **Serialize only GPU tests:** enable in `Cargo.toml` and annotate:
  ```toml
  [dev-dependencies]
  rstest = "0.21"
  assert_cmd = "2.0"
  predicates = "3.1"
  serial_test = { version = "3", features = ["file_locks"] }
  ```
  ```rust
  use serial_test::file_serial;

  #[file_serial(gpu)]
  #[rstest]
  #[case("200000:3fffff", "1CfZWK1QTQE3eS9qn61dQjV89KDjZzfNcv")]
  fn finds_known_address(#[case] range: &str, #[case] target: &str) { /* ... */ }
  ```

---

## Code hotspots & invariants

- **Endianness:** GPU emits **LE** limbs; convert to **BE** bytes before `SecretKey::from_slice`.
- **Range math:** Ranges are **inclusive**. Batch loop advances with `add_small_u256_le`.
- **Validity:** Skip scalar `0` and any invalid per libsecp256k1 (creation returns `Err`).
- **Address type:** Only **P2PKH mainnet** (`version 0x00`) is supported in `decode_p2pkh_to_hash160`.
- **Safety:** This tool searches deterministic ranges—**not** a key generator. Do **not** rely on any PRNG here for security-sensitive key material.

---

## Quality gates for contributions (agents, follow this checklist)

Before opening a PR, run:
```bash
cargo fmt --all
cargo clippy --all-targets --all-features -- -D warnings
cargo test -j 1 -- --test-threads=1
```

Add (or update) at least one of:
- A **unit or integration test**, or
- A **benchmark** result with keys/sec (see “Benchmarking” below).

Document changes in this file (AGENTS.md) if they affect architecture, flags, or behavior.

---

## Performance playbook

**Baseline metric:** keys verified per second (K/s). Record CPU model, GPU, OS, batch size.

1. **Batch sizing:** Tune `--batch` for your GPU VRAM (start at 1e6 on Apple Silicon). Larger batches reduce dispatch overhead but increase latency and memory.
2. **Overlap compute & verify (TODO):** Double-buffer the storage buffer; dispatch next batch while CPU verifies previous. Helps hide GPU latency.
3. **Avoid allocations:** Reuse buffers when adding streaming; pre-allocate vectors where possible.
4. **CPU parallelism:** Rayon `par_chunks_exact(32)` is used. Confirm `Secp256k1` context use is `Send + Sync` (it is); keep one context per thread or share a static.
5. **Hash offload (Next milestone):** Implement **GPU SHA-256** then **RIPEMD-160**; return only **hit indices** (or compact hit records). This will cut CPU cost dramatically.
6. **Sequential key EC speedup (Advanced):** Use **EC point addition** from a base (`k*G`, `(k+1)*G = P + G`) to avoid full scalar mul per candidate. Requires careful constant-time code and is better suited for GPU or at least SIMD CPU.
7. **Early stop:** On CPU verification, abort remaining work once a match is found; current code already stops the main loop, but consider cooperative cancellation for workers if needed.

---

## Roadmap (ranked)

1. **GPU HASH160 filter** (SHA-256 + RIPEMD-160 on GPU).
    - WGSL kernels; validate against NIST vectors & bitcoin-core test vectors.
    - Host side only reads back small “hit list” buffer.
2. **Streaming pipeline** (overlap GPU/CPU): ping-pong buffers + fences.
3. **Multiple targets** (search a set of addresses; use a hash table of H160 on GPU).
4. **Address types**: Support **P2WPKH (bech32)** and **P2TR (taproot)**.
5. **EC addition chain** (GPU): precompute `G` tables or use point addition for sequential keys.
6. **Cross‑platform CI**: GitHub Actions with optional GPU jobs; fall back to CPU-only tests; gated heavy tests as `#[ignore]`.
7. **Telemetry**: `--stats` prints keys/sec per batch, GPU time, CPU time; optional JSON output.

---

## Known issues / nice-to-haves

- **Odd-length hex** error: fixed by left-padding one zero; keep tests for it.
- **Bytemuck POD**: `Params` must derive `Pod + Zeroable`.
- **Inclusive end off-by-one**: keep the “remaining = end - cur + 1” pattern.
- **Graceful exit**: Add `ctrlc` handler to stop after current batch and report progress.
- **CLI polish**: Validate `--batch >= 1`, friendly errors for malformed addresses/ranges.

---

## Testing strategy

- **Functional hit:** `200000:3fffff` vs `1CfZWK1QTQE3eS9qn61dQjV89KDjZzfNcv` → must **FOUND**.
- **Functional miss:** narrow range with no hit → must print **Not found** and exit 0.
- **Decode hardening:** corrupted Base58 checksum should error.
- **Hex parsing:** odd length, underscores, `0x` prefix accepted.
- **Serialization:** For GPU tests across files, use `serial_test::file_serial("gpu")`.

Example integration test sketch:
```rust
#[file_serial(gpu)]
#[rstest]
#[case("200000:3fffff", "1CfZWK1QTQE3eS9qn61dQjV89KDjZzfNcv")]
fn finds_known_address(#[case] range: &str, #[case] target: &str) {
    // uses assert_cmd to run the binary
}
```

---

## Style & conventions

- **Rust 2021**, `cargo fmt`, `cargo clippy -D warnings`.
- Prefer **explicit endianness** in names (`*_le`, `*_be`).
- Keep GPU structs `#[repr(C)]` and `#[derive(Pod, Zeroable)]`.
- Avoid panics in library-like code; use `anyhow::Result` in `main.rs`.

Commit messages: conventional commits (`feat:`, `fix:`, `perf:`, `refactor:`, `test:`).

---

## Security notes

- This app searches deterministic ranges; it **does not generate secure keys**.
- Never expose real-found keys publicly; if demonstrating, use test vectors or throwaway ranges.
- Keep dependencies updated; prefer audited crypto crates (we use `rust-secp256k1`, `sha2`, `ripemd`).

---

## Useful references for agents

- Bitcoin key formats: WIF, compressed pubkeys, P2PKH **HASH160** pipeline.
- WGPU docs: compute pipelines, buffer usages (`STORAGE`, `MAP_READ`, `COPY_DST/SRC`).
- libsecp256k1: scalar validity, compressed serialization.

---

## Maintainer checklist (before release)

- [ ] `cargo deny` (optional) shows no critical advisories.
- [ ] `cargo audit` clean.
- [ ] Benchmarked baseline keys/sec saved in `BENCH.md`.
- [ ] Heavy GPU tests are `#[ignore]` or `#[file_serial("gpu")]` in CI.
- [ ] AGENTS.md updated if architecture or flags changed.

---

Happy hacking. Move fast, keep correctness sacred, and measure everything (keys/sec or it didn’t happen).
