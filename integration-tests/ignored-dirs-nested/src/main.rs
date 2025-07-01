use log::info;

mod ignored;
mod normal;
mod other;

fn main() {
    info!("This uses log, so it should not be reported as unused");
    normal::use_serde();
    // Note: intentionally not calling functions from ignored directories
}
