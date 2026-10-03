//! Transform throughput: `cargo run --release --example bench -- <file-list> [threads]`
fn main() {
    let list = std::fs::read_to_string(std::env::args().nth(1).expect("file list")).unwrap();
    let threads: usize = std::env::args().nth(2).map_or(1, |t| t.parse().unwrap());
    let files: Vec<(String, String)> =
        list.lines().filter(|l| !l.is_empty()).map(|f| (f.to_string(), std::fs::read_to_string(f).unwrap())).collect();
    let bytes: usize = files.iter().map(|f| f.1.len()).sum();
    let options = vue_ts_mapper::options::default_options(99.0, "vue", "/types");
    let next = std::sync::atomic::AtomicUsize::new(0);
    let start = std::time::Instant::now();
    std::thread::scope(|s| {
        for _ in 0..threads {
            s.spawn(|| {
                while let Some((f, c)) = files.get(next.fetch_add(1, std::sync::atomic::Ordering::Relaxed)) {
                    let _ = vue_ts_mapper::transform(f, c, &options, true);
                }
            });
        }
    });
    let t = start.elapsed().as_secs_f64();
    println!("{} files, {:.1} MB in {:.2}s with {threads} thread(s): {:.0} files/s, {:.2} ms/file", files.len(), bytes as f64 / 1e6, t, files.len() as f64 / t, t * 1000.0 / files.len() as f64);
}
