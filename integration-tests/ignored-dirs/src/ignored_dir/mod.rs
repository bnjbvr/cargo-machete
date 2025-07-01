use log_once::warn_once;

pub fn ignored_function() {
    warn_once!("This should be ignored!");
}
