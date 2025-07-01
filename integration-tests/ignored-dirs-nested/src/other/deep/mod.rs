// This directory is ignored via full path specification: "src/other/deep"
// If we used any dependencies here, they would be ignored

pub fn ignored_by_full_path() {
    println!("This function is in a directory ignored by full path specification");
}

// This module is in src/other/deep which should be ignored
// according to the full path specification in Cargo.toml

pub fn ignored_deep_function() {
    println!("This should be ignored");
}
