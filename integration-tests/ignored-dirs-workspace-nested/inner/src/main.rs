use log_once::warn_once;

mod normal_dir;

fn main() {
    warn_once!("Hello from workspace inner main!");
    normal_dir::normal_function();
}
