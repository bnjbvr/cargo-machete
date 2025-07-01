use serde::{Deserialize, Serialize};

#[derive(Serialize, Deserialize)]
pub struct GeneratedData {
    pub value: String,
}
