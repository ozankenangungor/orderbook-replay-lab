use std::fs;
use std::process::Command;

use replay::ReplayReader;
use tempfile::tempdir;

#[test]
fn gen_is_deterministic_and_replayable() {
    let dir = tempdir().expect("temp dir");
    let first = dir.path().join("first.log");
    let second = dir.path().join("second.log");
    let third = dir.path().join("third.log");

    let exe = env!("CARGO_BIN_EXE_orderbook-replay-lab-rs");
    for path in [&first, &second] {
        let output = Command::new(exe)
            .args([
                "gen",
                "--output",
                path.to_str().expect("path str"),
                "--symbol",
                "BTC-USD",
                "--events",
                "20",
                "--seed",
                "7",
            ])
            .output()
            .expect("run gen");
        assert!(output.status.success());
    }

    let output = Command::new(exe)
        .args([
            "gen",
            "--output",
            third.to_str().expect("path str"),
            "--symbol",
            "BTC-USD",
            "--events",
            "20",
            "--seed",
            "8",
        ])
        .output()
        .expect("run gen");
    assert!(output.status.success());

    let first_contents = fs::read_to_string(&first).expect("read first");
    let second_contents = fs::read_to_string(&second).expect("read second");
    let third_contents = fs::read_to_string(&third).expect("read third");
    assert_eq!(first_contents, second_contents);
    assert_ne!(first_contents, third_contents);

    let mut reader = ReplayReader::open(&first).expect("open replay");
    let mut count = 0u64;
    while reader.next_event().expect("read event").is_some() {
        count += 1;
    }
    assert_eq!(count, 20);
}
