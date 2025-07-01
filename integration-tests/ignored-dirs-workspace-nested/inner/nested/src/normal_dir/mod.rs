use rand::Rng;

pub fn normal_function() {
    let mut rng = rand::thread_rng();
    println!("Random from normal dir: {}", rng.gen::<u32>());
}
