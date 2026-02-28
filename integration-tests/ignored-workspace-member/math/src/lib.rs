use rand::Rng;

pub fn add_random(a: i32, b: i32) -> i32 {
    let mut rng = rand::thread_rng();
    a + b + rng.gen_range(1..10)
}