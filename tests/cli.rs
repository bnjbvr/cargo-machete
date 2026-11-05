use std::process::Command;

// https://github.com/bnjbvr/cargo-machete/issues/143
#[test]
fn test_error_when_no_cargo_toml() {
    let output = Command::new(env!("CARGO_BIN_EXE_cargo-machete"))
        .arg(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/integration-tests/empty-dir"
        ))
        .output()
        .expect("failed to run cargo-machete");
    assert_eq!(output.status.code(), Some(2));
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("found no Cargo.toml"), "stderr: {stderr}");
}
