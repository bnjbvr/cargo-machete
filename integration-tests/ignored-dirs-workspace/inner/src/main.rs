use log_once::warn_once;

mod normal_dir;
mod workspace_ignored_dir;

fn main() {
    warn_once!("Hello from workspace inner main!");
    normal_dir::normal_function();
    // Note: we don't call workspace_ignored_dir::ignored_function() intentionally
}
