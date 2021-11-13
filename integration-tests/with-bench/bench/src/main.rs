#[macro_use]
extern crate bencher;

use bencher::Bencher;

const N: usize = 10;

fn func(bench: &mut Bencher) {
    let mut a = (0..100).into_iter().collect::<Vec<_>>();
    bench.iter(|| {
        for _ in 0..N {
            sortlib::sort_array(&mut a);
        }
    });
}

benchmark_group!(group, func);

benchmark_main!(group);
