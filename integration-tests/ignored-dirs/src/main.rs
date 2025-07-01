use log::info;

mod ignored_dir;
mod normal_dir;

fn main() {
    info!("Hello from main!");
    normal_dir::normal_function();
    // Note: we don't call ignored_dir::ignored_function() intentionally
}
