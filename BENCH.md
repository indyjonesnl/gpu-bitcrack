# BENCH.md — Benchmarking

A practical, repeatable plan to measure and compare the performance of **gpu-bitcrack** across machines and commits.

> **Goal:** quantify **keys/sec**, understand where time is spent (GPU vs CPU), and catch regressions.

---

## Quick start

```bash
# Build optimized binary
cargo build --release

# Throughput (MISS) benchmark — 2,097,152 keys with no hit (use an address that won't match)
hyperfine -w 2 -r 5 '
  ./target/release/gpu-bitcrack 100000:2fffff 1FeexV6bAHb8ybZjqQMjJrcCrHGW9sb6uF --batch 1000000
'

# Known-hit benchmark — 2,097,152 keys, guaranteed match
# Range size: 0x3fffff - 0x200000 + 1 = 2,097,152 keys
hyperfine -w 2 -r 5 '
  ./target/release/gpu-bitcrack 200000:3fffff 1CfZWK1QTQE3eS9qn61dQjV89KDjZzfNcv --batch 1000000
'
````

* Install `hyperfine` (recommended): macOS `brew install hyperfine`, Debian/Ubuntu `apt install hyperfine`.
* On shared GPUs, **serialize tests**: `cargo test -j 1 -- --test-threads=1` or `serial_test` (see AGENTS.md).

---

## 1) Metrics

Record at least:

* **Throughput:** `keys/sec` (higher is better)

    * `keys/sec = range_size / elapsed_seconds`
* **Batch size:** `--batch` used
* **Hit latency (known-hit runs):** elapsed time until `FOUND!`
* **Binary hash:** `git rev-parse --short HEAD`
* **Backend:** WGPU backend (Metal/Vulkan/DX12) — see program banner or `RUST_LOG=wgpu_core=info`

**Optional (nice-to-have):**

* **GPU time vs CPU time** (when streaming/pipelining is implemented)
* **Energy / Power:** macOS `powermetrics`, Linux `nvidia-smi` (if applicable)
* **Thermals:** note sustained vs burst throughput (throttling)

---

## 2) Environment capture

Include this block in every benchmark report:

```
Commit:            <git sha>
Date:              <YYYY-MM-DD>
OS:                <e.g., macOS 15.x, Linux 6.x>
CPU:               <e.g., M3 Pro 11C/14C | i9-12900K>
GPU:               <e.g., Apple M3 Pro 14c GPU | RTX 3080>
RAM/VRAM:          <e.g., 32 GB unified | 10 GB VRAM>
Rust:              $(rustc --version)
Cargo:             $(cargo --version)
WGPU backend:      <Metal | Vulkan | DX12 | GL>
Build:             release (LTO=<on/off>, debug-assertions=<on/off>)
Env flags:         RUSTFLAGS="<...>"  (if any)
Thermal settings:  <performance mode / power adapter plugged>
```

**Tips (macOS):**

* Use **Metal** backend by default.
* Power and temperature can be sampled with:

  ```bash
  sudo powermetrics --samplers gpu_power --show-influx --samplers tasks --timeout 10
  ```

---

## 3) Canonical ranges

Use these standard ranges to keep results comparable:

| Name        | Range (hex)         | Size (keys) | Contains hit? | Target address                       |
| ----------- | ------------------- | ----------- | ------------- | ------------------------------------ |
| **K2-MISS** | `100000:2fffff`     | 1,966,080   | No (random)   | `1FeexV6bAHb8ybZjqQMjJrcCrHGW9sb6uF` |
| **K2-HIT**  | `200000:3fffff`     | 2,097,152   | **Yes**       | `1CfZWK1QTQE3eS9qn61dQjV89KDjZzfNcv` |
| **K8-MISS** | `01000000:013fffff` | 262,144     | No (random)   | `1FeexV6bAHb8ybZjqQMjJrcCrHGW9sb6uF` |

> Rationale: **K2-HIT** has a single known match; **K2-MISS** is a similar-size negative case; **K8-MISS** is a smaller quick check.

---

## 4) How to compute keys/sec

For each run, compute:

```
keys/sec = range_size / elapsed_seconds
```

Examples:

* **K2-HIT** size = `0x3fffff - 0x200000 + 1 = 2,097,152` keys
* If elapsed = `1.80 s` → `1,165,085 keys/sec`

`hyperfine` prints mean and stddev times; compute keys/sec from the mean.

---

## 5) Commands

### 5.1 MISS benchmark (throughput focus)

```bash
hyperfine -w 2 -r 7 '
  ./target/release/gpu-bitcrack 100000:2fffff 1AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA --batch 1000000
'
```

### 5.2 Known-hit benchmark

```bash
hyperfine -w 2 -r 7 '
  ./target/release/gpu-bitcrack 200000:3fffff 1CfZWK1QTQE3eS9qn61dQjV89KDjZzfNcv --batch 1000000
'
```

### 5.3 Batch-size sweep

Find the sweet spot for your GPU by sweeping `--batch`:

```bash
for b in 262144 524288 750000 1000000 1500000 2000000; do
  echo "Batch=$b"
  hyperfine -w 2 -r 5 "
    ./target/release/gpu-bitcrack 100000:2fffff 1AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA --batch $b
  "
done
```

> Memory use ≈ `batch * 32` bytes (e.g., 2,000,000 → \~64 MiB).

---

## 6) Results template

Copy-paste and fill in:

```
### Machine
OS: macOS 15.x
CPU: Apple M3 Pro (11C CPU)
GPU: Apple 14-core
RAM: 36 GB unified
Rust: rustc 1.xx.x (stable)
Backend: Metal

### Build
Profile: release
LTO: off
RUSTFLAGS: (none)

### Runs
K2-MISS (batch=1,000,000): mean 1.92 s (std 0.03) → 1,024,556 keys/sec
K2-HIT  (batch=1,000,000): mean 1.87 s (std 0.02) → 1,121,302 keys/sec

### Notes
- Fan curve stable, no thermal throttle observed.
- Best batch for this machine: 1,000,000.
```

Optionally export `hyperfine` results and attach the JSON/CSV into your PR:

```bash
hyperfine -w 2 -r 7 --export-json bench.json --export-markdown bench.md '...'
```

---

## 7) Interpreting results

* **K2-HIT vs K2-MISS** should be similar; a hit ends slightly earlier depending on key position inside the range.
* If **MISS >> HIT** time, investigate early-abort logic and Rayon worker cancellation.
* If larger batches **hurt** performance, you may be saturating CPU verification or causing paging/VRAM pressure.
* Watch for **stddev spikes** (thermal throttling / background tasks).

---

## 8) Regression gates

Set a threshold to fail CI on slowdowns (manual for now):

* If mean keys/sec drops by **>5%** vs last baseline on K2-MISS with the same batch, flag the PR.
* Keep a rolling baseline in this file or in `bench/baseline.json`.

---

## 9) Future phases (what to benchmark when they land)

1. **GPU HASH160 filter:** measure host readback size, hits/sec, and CPU usage drop.
2. **Streaming pipeline:** report **GPU time**, **CPU time**, **overlap efficiency** = `(GPU_time + CPU_time - wall_time) / (GPU_time + CPU_time)`.
3. **Multi-target search:** measure scaling vs number of target hashes.
4. **EC addition chain:** compare full scalar-mul vs addition-from-base method on sequential ranges.
5. **Bech32 & Taproot:** add canonical targets and ranges for P2WPKH/P2TR.

---

## 10) Troubleshooting

* **Odd number of digits:** hex inputs must be even-length; code auto left-pads a `0`, but double-check.
* **GPU not found:** ensure Metal/Vulkan/DX12 available; on macOS install Xcode CLT; on Linux install Vulkan drivers.
* **Parallel tests fighting for the GPU:** run with `cargo test -j 1 -- --test-threads=1` or use `serial_test` file locks.
* **Noisy results:** close other GPU/CPU-heavy apps; pin performance mode if available.

---
