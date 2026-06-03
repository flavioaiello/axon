use std::{env, fs, process::Command};

fn main() {
    println!("cargo:rerun-if-env-changed=AXON_BUILD_COMMIT");
    println!("cargo:rerun-if-changed=.git/HEAD");
    println!("cargo:rerun-if-changed=.git/packed-refs");

    if let Some(git_ref) = current_git_ref() {
        println!("cargo:rerun-if-changed=.git/{git_ref}");
    }

    let commit = env::var("AXON_BUILD_COMMIT")
        .ok()
        .and_then(short_commit)
        .or_else(git_commit)
        .unwrap_or_else(|| "unknown".to_string());

    println!("cargo:rustc-env=AXON_BUILD_COMMIT={commit}");
    println!("cargo:rustc-env=AXON_VERSION=main (commit {commit})");
}

fn git_commit() -> Option<String> {
    let output = Command::new("git")
        .args(["rev-parse", "--short=12", "HEAD"])
        .output()
        .ok()?;

    output.status.success().then_some(())?;
    short_commit(String::from_utf8_lossy(&output.stdout).to_string())
}

fn current_git_ref() -> Option<String> {
    let head = fs::read_to_string(".git/HEAD").ok()?;
    head.strip_prefix("ref: ")
        .map(str::trim)
        .and_then(|git_ref| non_empty(git_ref.to_string()))
}

fn non_empty(value: String) -> Option<String> {
    if value.is_empty() { None } else { Some(value) }
}

fn short_commit(value: String) -> Option<String> {
    let value = non_empty(value.trim().to_string())?;
    Some(value.chars().take(12).collect())
}
