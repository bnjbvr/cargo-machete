use log::info;

mod backward;
mod forward;

fn main() {
    info!("This uses log, so it should not be reported as unused");
}
