fn main() {
    log::info!("main [dependencies]");
}

#[test]
fn test_log_once() {
    log_once::info_once!("[dev-dependencies]");
}
