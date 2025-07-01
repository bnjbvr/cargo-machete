use tokio::time::{sleep, Duration};

pub async fn normal_function() {
    sleep(Duration::from_millis(100)).await;
}
