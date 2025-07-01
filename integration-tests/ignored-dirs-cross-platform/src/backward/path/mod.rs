use std::time::Duration;
use tokio::time::sleep;

pub async fn use_tokio_backward() {
    println!("Backward path using tokio");
    sleep(Duration::from_millis(1)).await;
}
