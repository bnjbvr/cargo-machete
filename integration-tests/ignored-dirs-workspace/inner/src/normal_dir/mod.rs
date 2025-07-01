use serde::{Deserialize, Serialize};

#[derive(Serialize, Deserialize)]
pub struct TestStruct {
    pub name: String,
}

pub fn normal_function() {
    let test = TestStruct {
        name: "test".to_string(),
    };
    println!("Test: {}", test.name);
}
