use std::time::Duration;
use tokio::time::sleep;

pub async fn async_sleep() {
    sleep(Duration::from_millis(100)).await;
}
