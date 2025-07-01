use rand::Rng;

pub fn ignored_function() {
    let mut rng = rand::thread_rng();
    println!("Random from ignored dir: {}", rng.gen::<u32>());
}
