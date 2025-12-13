# Codec Bench (2026-02-28)

Command:

```sh
cargo bench -p codec --bench codec_bench --features bin -- --measurement-time 2 --warm-up-time 1 --sample-size 20
```

Median-ish range (Criterion):

- `codec/json_encode_delta_32`: `1.69-1.73 us`
- `codec/json_encode_snapshot_16`: `585-595 ns`
- `codec/json_decode_delta_32`: `2.38-2.52 us`
- `codec/bin_encode_delta_32`: `228-239 ns`
- `codec/bin_decode_delta_32`: `281-314 ns`

Notes:

- JSON decode remains materially slower than JSON encode due to parsing.
- Binary encode/decode stays sub-microsecond for this synthetic workload.
- Use pinned CPU and longer measurement windows for stricter regression gates.
