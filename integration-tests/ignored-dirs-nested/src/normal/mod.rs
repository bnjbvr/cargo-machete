use serde::{Deserialize, Serialize};

#[derive(Serialize, Deserialize)]
pub struct TestStruct {
    pub name: String,
}

pub fn use_serde() {
    let test = TestStruct {
        name: "normal".to_string(),
    };
    println!("Normal function using serde: {}", test.name);
}

#[derive(Serialize)]
pub struct Data {
    pub value: String,
}
