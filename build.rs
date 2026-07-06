// Embed the git commit count so every build carries an auto-incrementing
// release number: <CARGO_PKG_VERSION>-r<commit count>, e.g. "0.1.1-r47".
fn main() {
    let count = std::process::Command::new("git")
        .args(["rev-list", "--count", "HEAD"])
        .output()
        .ok()
        .filter(|o| o.status.success())
        .and_then(|o| String::from_utf8(o.stdout).ok())
        .map(|s| s.trim().to_string())
        .unwrap_or_default();
    let version = if count.is_empty() {
        env!("CARGO_PKG_VERSION").to_string()
    } else {
        format!("{}-r{}", env!("CARGO_PKG_VERSION"), count)
    };
    println!("cargo:rustc-env=PHOTONDNS_VERSION={}", version);
    println!("cargo:rerun-if-changed=.git/HEAD");
    println!("cargo:rerun-if-changed=.git/refs/heads");
}
