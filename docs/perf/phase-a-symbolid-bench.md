# Phase A Benchmark Note (SymbolId migration)

Command used for both runs:

```sh
cargo bench -p orderbook --bench orderbook_bench -- --measurement-time 2 --warm-up-time 1 --sample-size 20
```

Environment:
- branch: `perf/baseline`
- date: 2026-02-26
- benchmark target: `orderbook_apply/*`

## Before vs After (median)

| Benchmark | Before | After | Delta |
| --- | ---:| ---:| ---:|
| `orderbook_apply/updates/small` | 17.144 us | 18.941 us | +10.5% |
| `orderbook_apply/updates_with_top_of_book/small` | 17.761 us | 19.064 us | +7.3% |
| `orderbook_apply/updates/medium` | 84.818 us | 80.547 us | -5.0% |
| `orderbook_apply/updates_with_top_of_book/medium` | 98.130 us | 81.483 us | -17.0% |

Interpretation:
- results are mixed and noisy at this sample size;
- no consistent throughput regression in medium workloads;
- follow-up should rerun with longer measurement windows and pinned CPU isolation.
