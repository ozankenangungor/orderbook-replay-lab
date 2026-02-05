use std::fs;
use std::process::Command;

use replay::ReplayReader;
use tempfile::tempdir;

#[test]
fn gen_is_deterministic_and_replayable() {
    let dir = tempdir().expect("temp dir");
    let first = dir.path().join("first.log");
    let second = dir.path().join("second.log");

    let exe = env!("CARGO_BIN_EXE_rust-latency-lob");
    for path in [&first, &second] {
        let output = Command::new(exe)
            .args([
                "gen",
                "--output",
                path.to_str().expect("path str"),
                "--symbol",
                "BTC-USD",
                "--events",
                "5",
            ])
            .output()
            .expect("run gen");
        assert!(output.status.success());
    }

    let first_contents = fs::read_to_string(&first).expect("read first");
    let second_contents = fs::read_to_string(&second).expect("read second");
    assert_eq!(first_contents, second_contents);

    let mut reader = ReplayReader::open(&first).expect("open replay");
    let mut count = 0u64;
    while let Some(_) = reader.next_event().expect("read event") {
        count += 1;
    }
    assert_eq!(count, 5);
}
