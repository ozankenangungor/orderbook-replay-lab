# Performance methodology notes

This repository emphasizes deterministic replay and simple, observable performance.
In the replay path, we made small changes that reduce avoidable allocations without
changing behavior:

- Reuse buffers: `ReplayReader` now pre-allocates a larger `BufReader` and a
  reusable line buffer to avoid repeated growth and reallocation.
- Avoid full-line trimming: decode now strips only trailing newline characters
  instead of scanning the entire line for whitespace.

These changes keep the API the same but reduce per-line overhead in hot loops.
