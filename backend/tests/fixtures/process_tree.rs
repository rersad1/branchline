use std::{time::Duration, process::Command};
fn main() {
    let args: Vec<_> = std::env::args().collect();
    if args.get(1).is_some_and(|v| v == "sleep") { std::thread::sleep(Duration::from_secs(60)); return; }
    assert_eq!(&args[1..6], ["-c", "core.longpaths=true", "-c", "core.fsmonitor=false", "-C"]);
    let directory = std::path::PathBuf::from(&args[6]);
    if directory.ends_with("bad") { std::process::exit(17); }
    if directory.ends_with("large") { println!("{}", "x".repeat(8192)); return; }
    if directory.ends_with("ok") { println!("{}", directory.display()); return; }
    let child = Command::new(std::env::current_exe().unwrap()).arg("sleep").spawn().unwrap();
    std::fs::write(directory.join("pids"), format!("{} {}", std::process::id(), child.id())).unwrap();
    std::thread::sleep(Duration::from_secs(60));
}
