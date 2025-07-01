use serde::Serialize;

#[derive(Serialize)]
pub struct ForwardData {
    pub name: String,
}

pub fn use_serde_forward() {
    let data = ForwardData {
        name: "test".to_string(),
    };
    println!("Forward path using serde: {:?}", data.name);
}
