use uuid::Uuid;

pub mod nested;

pub fn use_uuid() {
    let id = Uuid::new_v4();
    println!("Generated UUID in ignored dir: {}", id);
}

pub fn generate_id() -> Uuid {
    Uuid::new_v4()
}
