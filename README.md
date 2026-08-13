# multimerge_pg

**multimerge_pg** is a high-performance, parallel sorting engine built in Rust for PostgreSQL. It was designed to bypass the physical and architectural limitations of the standard PostgreSQL `ORDER BY` clause when handling massive datasets that exceed the `work_mem` limit.

While PostgreSQL is a masterpiece of relational engineering, its standard External Merge Sort becomes I/O-bound as datasets grow. **multimerge_pg** solves this by leveraging **Multithreading**, **Binary Vectorization**, and **Arena Allocation** to keep data strictly within RAM, preserving hardware longevity and slashing execution costs in cloud environments.

This repository now runs on the **v2 core** — a full replacement of the sorting engine, described below.

---

# 🔗 Core Algorithm & Academic Background

> 📌 **Note:** The mathematical foundations, dynamic heuristics, and exhaustive standalone benchmarks of the Multimerge engine are fully detailed and tested in the primary repository.
> 👉 **[Core Multimerge Sorting Repository](https://github.com/fbcouto/adaptive-parallel-multimerge-sort)**

The core theoretical foundation of this parallel architecture is based on the original research and paper:

- **Title:** *Multimerge*
- **Authors:** Fernando B. Couto & Fábio S. Couto
- **Conference:** PDPTA'11 — The 2011 International Conference on Parallel and Distributed Processing Techniques and Applications
- **Lecture Series:** WorldComp'11 (The 2011 World Congress in Computer Science, Computer Engineering, and Applied Computing)
- The architecture implements a hybrid processing model based on the original **Multimerge** paper published in **PDPTA'11**.

It modernizes those multi-merge paradigms by utilizing runtime entropy sampling and Rayon's work-stealing parallel scheduler.

---

# 🆕 What's New in the v2 Core

The sorting engine was replaced end-to-end. The previous heuristic could only answer one coarse question — *"is this entire array approximately one monotonic run?"* — and gave up on anything else. The v2 core asks a much more general question — *"where are the monotonic runs, at any granularity, anywhere in this array?"* — which is what unlocks the gains below.

- **Fractal run detection.** A multi-level scan (32,768-element macro blocks, subdivided into 512-element micro blocks) encodes the entire array as signed run-length metadata in parallel, then stitches adjacent blocks together with an O(log N) parallel reduce. This finds an arbitrary number of ascending/descending runs anywhere in the data, not just a single global trend.
- **O(1) entropy shield.** A cheap ~100-element probe near the middle of the array detects pure noise up front and bails straight into a parallel sort, instead of spending time on structure-detection that has nothing to find.
- **Hybrid co-rank + bidirectional parallel merge.** Detected runs are combined with a parallel merge that splits work by rank (co-rank) rather than naive index halving, so both branches of a merge get balanced, cache-aware chunks.
- **Guaranteed stability, on every path.** The previous engine's chaos fallback used an unstable sort, so relative order of equal keys wasn't guaranteed in that branch. The v2 core is stable unconditionally — including on pure-noise input.
- **Cache-aware base case.** The sequential leaf threshold is now derived from L1 cache size instead of a fixed constant.

---

# 🗺️ Engineering Roadmap

The project evolved through several stages:

1. **Binary Vectorization** — Bypassed row-based SQL processing using `COPY BINARY` streams and `BufReader`, eliminating row-processing overhead.
2. **Parallelism (Rayon)** — Implemented a work-stealing parallel merge sort to saturate all physical CPU cores.
3. **Memory Management (Bumpalo)** — Integrated an Arena Allocator to eliminate heap fragmentation for dynamic text data.
4. **Pointer Sorting** — Shifted from moving heavy data structures to sorting 8-byte references (`&str`), maximizing CPU cache efficiency.
5. **Fractal Run Detection** — Replaced the single-global-trend heuristic with multi-level run-length metadata (see above).
6. **Hybrid Co-Rank Bidirectional Merge** — Rank-balanced parallel merge for combining detected runs.
7. **Unconditional Stability** — Closed the gap where the chaos-detection fallback wasn't stable.

---

# 📊 Performance Benchmarks

## Current Engine (v2) — Validated Results

Measured against native `ORDER BY` on the same machine, same run, 200M `i32` rows:

| Data Shape | Rust Time | PG Native Time | Speedup | PG Disk Spill |
|---|---|---|---|---|
| Uniform random | 243.2s | 496.8s | **2.04x** | 2.35 GB |
| Two sorted runs, concatenated (partial order) | 189.9s | 343.7s | 1.81x † | 1.15 GB |

† **Methodology caveat:** in this specific test, PostgreSQL's planner parallelized the native query differently between the two rows (`Parallel Append` over the `UNION ALL` structure launched 2 extra worker processes for the structured case; the random case ran single-threaded). Part of the native side's improvement here comes from that extra parallelism, not purely from the data being easier to sort. See the pure-algorithm comparison below for a controlled measurement of that effect, and the reproduction steps for how to pin `max_parallel_workers_per_gather` for a clean re-run.

Pure sorting-algorithm speed (the SPI-based entry points), independent of file I/O, isn't included in the table above — see the in-memory benchmark below for that isolated number.

## Pure-Algorithm Comparison (in-memory, no Postgres, no disk I/O)

Measured with `criterion` in the sibling [`adaptive-parallel-multimerge-sort`](https://github.com/fbcouto/adaptive-parallel-multimerge-sort) repository, 5M `i32` elements, median of 20 samples:

| Data Shape | v2 Engine | v1 Engine | Rayon `par_sort_unstable` | Rayon `par_sort` (stable) |
|---|---|---|---|---|
| Uniform random (0..1,000,000) | 59.7 ms | 47.4 ms | 42.9 ms | 62.8 ms |
| Two sorted runs, concatenated | **10.2 ms** | 54.2 ms | 45.0 ms | — |

Reading this table:

- **On structured data, v2 is 5.3x faster than v1 and 4.4x faster than Rayon's own parallel sort.** v1's trend detector never recognized the two-run structure (its output there is statistically indistinguishable from a plain `par_sort_unstable`), while v2's fractal detection finds it.
- **On pure noise, v2 is *not* slower than the correct baseline.** v2's chaos fallback calls Rayon's *stable* `par_sort()`, not `par_sort_unstable()` — that's the fair comparison, since v2 guarantees stability unconditionally and v1 does not. Against that baseline, v2 is ~5% faster (59.7ms vs 62.8ms). The apparent "penalty" against `par_sort_unstable` (42.9ms) is the well-understood, unavoidable cost of stability, paid by any stable sort — not a flaw in the entropy detector.

## Legacy Engine (v1, pre-upgrade) — Historical Results

<p align="center">
  <img src="./comparacaoPostgree.JPG" alt="Performance Evolution" width="800">
</p>

The table and chart below reflect the **previous** engine and have not been re-validated at every size against v2. They're kept for historical reference; the 200M row above is the current, validated number.

| Records | Rust Time | PG Native | Rust Speedup | PG Disk Usage | Rust Disk Usage |
|---|---|---|---|---|---|
| 10 Million | 8.3s | 8.6s | 1.03x | 117 MB | 0 |
| 50 Million | 39.8s | 44.3s | 1.11x | 587 MB | 0 |
| 100 Million | 78.8s | 98.3s | 1.25x | 1.15 GB | 0 |
| 200 Million | 160.4s | 281.7s | 1.76x | 2.35 GB | 0 |

Re-running this full sweep with the v2 engine at every size is open future work (see below).

---

# 🚀 Technical Deep Dive

## 1. Memory Management: The "Arena" Advantage

When processing millions of strings or dynamic data, the standard `String` allocation in Rust (and most C engines) causes severe heap fragmentation.

By using `bumpalo` (arena allocation), we treat memory as a contiguous block. Instead of asking the operating system for memory 50 million times, we request one large block (e.g., 512MB) and simply "bump" a pointer forward for each new entry. This eliminates overhead and keeps the CPU's L1/L2 caches fed with contiguous data.

## 2. Why Text Sorting Still Uses `par_sort_unstable`

The v2 engine's headline optimization — skipping zero-initialization of its scratch buffer (`unsafe { buffer.set_len(n) }`) — is sound for primitive types, where every bit pattern is a valid value. It is **not** sound for reference types like `&str`: Rust guarantees references are always valid, and a buffer of uninitialized references briefly violates that, even if nothing ever reads the unwritten slot.

Both text-sorting entry points (`pg_multimerge_binary_copy_text` and `_text_arena`) therefore deliberately use Rayon's `par_sort_unstable` instead of the v2 engine, even though `&str` satisfies the `Copy` bound. Making the v2 engine safe for reference types would mean writing through `MaybeUninit<T>` instead of `set_len` — tracked as future work in the core repository.

## 3. The "Marshalling Tax"

You might notice that in some benchmarks, our engine is only slightly faster or, in the case of complex string marshalling, roughly on par with PostgreSQL.

The reason is the FFI (Foreign Function Interface) boundary. PostgreSQL natively keeps data in its own optimized C structures. To return data from Rust to PostgreSQL, we must re-package (serialize) our sorted results into the format the database expects.

The heavy lifting (the sorting and memory management) is performed significantly faster in Rust, but the final step of handing the data back to PostgreSQL incurs a **"Marshalling Tax."**

## 4. Impact on Infrastructure and Cloud Costs

- **Hardware Longevity:** PostgreSQL's external merge writes over 2GB of temporary data to disk for 200M records. Constantly writing to SSDs degrades their TBW (Total Bytes Written) rating.
- **Cloud IOPS Costs:** In cloud environments (AWS RDS/EBS), you pay for IOPS. Keeping processing in RAM avoids the throttles and extra costs associated with spilling to disk.

**Precision note:** the sorting algorithm itself performs zero disk writes on every path — no temporary run files, no external-merge spill, regardless of entry point. The `COPY BINARY`-based entry points (`pg_multimerge_binary_copy*`) do write one intermediate file to pull data out of PostgreSQL efficiently, deleted immediately after being read into RAM — a single linear pass, categorically different from PostgreSQL's own multi-pass external-merge spill. Only the SPI-based entry points (`pg_multimerge_stream_i32` / `pg_multimerge_stream_i64`) touch zero bytes of disk anywhere in the request.

---

# 🛠️ How to Reproduce

## Prerequisites

- Rust/Cargo installed (built and tested against `nightly-x86_64-pc-windows-msvc` on Windows; adjust if your `pgrx` setup uses a different toolchain).
- `pgrx` 0.18 installed and configured (`cargo pgrx init`).
- PostgreSQL 17 (or compatible).

## Steps

### 1. Clone the repository

```bash
git clone https://github.com/fbcouto/multimerge_pg.git
cd multimerge_pg
```

### 2. Windows only: Clang/bindgen compatibility

`pgrx-pg-sys` generates PostgreSQL bindings via `bindgen`/`libclang`. Clang 19+ turns a long-standing, harmless pointer-signedness mismatch in Postgres's own `port/atomics/generic-msvc.h` into a hard build error. This repo ships a project-level fix — `.cargo/config.toml` — that downgrades it back to a warning, so no manual Clang install or `LIBCLANG_PATH` juggling is needed:

```toml
[env]
BINDGEN_EXTRA_CLANG_ARGS = "-Wno-error=incompatible-pointer-types"
```

This applies automatically to anyone who builds the repo, on any machine.

### 3. Build and run

```bash
cargo pgrx run --release --features pg17
```

### 4. Benchmarking

Inside `psql`:

```sql
CREATE EXTENSION multimerge_pg;
\timing on

-- Uniform random integers
SELECT count(*) FROM pg_multimerge_binary_copy(
    'SELECT (random() * 1000000)::integer FROM generate_series(1, 200000000)',
    'C:/Users/Public/pg_batch.bin'
);

EXPLAIN ANALYZE
SELECT * FROM (SELECT (random() * 1000000)::integer AS val FROM generate_series(1, 200000000)) sub
ORDER BY val;

-- Partially ordered: two sorted runs concatenated (simulates incremental
-- updates / append-only logs). Pin parallelism on the native side so the
-- comparison isn't confounded by the planner choosing a different number
-- of workers than it did for the random-data query above.
SET max_parallel_workers_per_gather = 0;

SELECT count(*) FROM pg_multimerge_binary_copy(
    'SELECT val FROM (
        SELECT generate_series AS val FROM generate_series(1, 100000000)
        UNION ALL
        SELECT generate_series AS val FROM generate_series(1, 100000000)
    ) sub',
    'C:/Users/Public/pg_batch_structured.bin'
);

EXPLAIN ANALYZE
SELECT * FROM (
    SELECT generate_series AS val FROM generate_series(1, 100000000)
    UNION ALL
    SELECT generate_series AS val FROM generate_series(1, 100000000)
) sub
ORDER BY val;
```

### 5. Isolating the algorithm from I/O (recommended)

For a controlled, DB-independent comparison of the sorting algorithm itself, use the `criterion` benchmark in the sibling repository:

```bash
cd ../adaptive-parallel-multimerge-sort
cargo +stable-x86_64-pc-windows-gnu bench
```

That repo also builds a C++/OpenMP comparison engine via FFI, which requires GCC (`__gnu_parallel` isn't available under MSVC) — hence the GNU toolchain, independent of whatever toolchain you use for `multimerge_pg` itself. Results land in the terminal and as an HTML report at `target/criterion/report/index.html`.

---

# 🔭 Future Work

- **Full benchmark sweep with v2.** Re-run the 10M/50M/100M/200M table against native Postgres with the current engine, matching parallelism settings on both sides at every size.
- **`MaybeUninit`-based buffer for reference types**, so `&str` sorting can use the v2 engine's fast path safely (see Technical Deep Dive above).
- **GPU-assisted sort — evaluated and deliberately deferred.** GPU acceleration inside Postgres exists only via third-party extensions (e.g. PG-Strom), never in core. PG-Strom's own history is instructive here: an early `GpuSort` was dropped because full-materialization sort doesn't reduce data volume, so the PCI-E transfer cost dominates; it was only reintroduced once GPU memory grew large enough to hold the working set, and specifically for `LIMIT`/window-function-bounded queries — not general `ORDER BY`. Since every entry point in this project fully materializes its result back to PostgreSQL as rows, it hits exactly the pattern GPU offload struggles with. A narrow `pg_multimerge_top_k`-style function, where the result set is small and the input fits in GPU memory, would be the scenario worth revisiting.

---

# 🧠 Final Notes

This engine stands as a reference case: with Rust, we can push PostgreSQL beyond the theoretical limits of its traditional architecture — and, with the v2 core, do it while making guarantees (stability, cache-aware run detection) the original heuristic didn't.

---

# 📄 License

This project is licensed under the Apache License 2.0.

You may obtain a copy of the license at:

- <https://www.apache.org/licenses/LICENSE-2.0>

Copyright © Fernando B. Couto

Licensed under the Apache License, Version 2.0 (the "License");
you may not use this project except in compliance with the License.
You may obtain a copy of the License at:

<http://www.apache.org/licenses/LICENSE-2.0>

Unless required by applicable law or agreed to in writing, software
distributed under the License is distributed on an "AS IS" BASIS,
WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
See the License for the specific language governing permissions and
limitations under the License.
