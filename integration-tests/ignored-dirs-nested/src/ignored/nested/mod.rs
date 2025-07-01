use rand::random;

pub mod deep;

pub fn use_rand() {
    let mut rng = rand::thread_rng();
    println!("Random number from ignored/nested: {}", rng.gen::<u32>());
}

pub fn get_random_number() -> u32 {
    random()
}
