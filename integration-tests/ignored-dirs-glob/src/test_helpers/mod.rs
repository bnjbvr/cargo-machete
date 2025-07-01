use regex::Regex;

pub fn helper_function() {
    let re = Regex::new(r"test").unwrap();
    println!("Helper with regex: {}", re.is_match("test"));
}
