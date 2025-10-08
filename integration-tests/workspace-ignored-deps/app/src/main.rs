use serde::{Deserialize, Serialize};

#[derive(Serialize, Deserialize)]
struct Config {
    name: String,
    debug: bool,
}

fn main() {
    let config = Config {
        name: "test app".to_string(),
        debug: true,
    };

    println!("Config: {} (debug: {})", config.name, config.debug);
}
