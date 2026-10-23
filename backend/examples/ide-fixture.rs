//! Isolated D6 IDE launcher boundary fixture; never a real editor compatibility claim.
fn main() {
    let cwd = std::env::current_dir().unwrap();
    let report = serde_json::json!({"cwd":cwd,"argv":std::env::args().skip(1).collect::<Vec<_>>(),"pid":std::process::id()});
    std::fs::write(
        cwd.parent().unwrap().join("ide-launch.json"),
        serde_json::to_vec(&report).unwrap(),
    )
    .unwrap();
    println!("IDE fixture ready");
    std::thread::sleep(std::time::Duration::from_secs(120));
}
