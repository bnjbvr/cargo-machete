use serde::{Deserialize, Serialize};

#[derive(Serialize, Deserialize)]
pub struct Data {
    pub name: String,
    pub value: i32,
}

pub fn create_data() -> Data {
    Data {
        name: "test".to_string(),
        value: 42,
    }
}
