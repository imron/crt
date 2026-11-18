fn main() {
    // Capture git commit hash at build time using git2
    let git_hash = get_git_hash().unwrap_or_else(|| "unknown".to_string());
    println!("cargo:rustc-env=CRT_BUILD_HASH={git_hash}");

    // Rerun if git HEAD changes
    println!("cargo:rerun-if-changed=.git/HEAD");
    println!("cargo:rerun-if-changed=.git/refs/");
}

fn get_git_hash() -> Option<String> {
    let repo = git2::Repository::discover(".").ok()?;
    let head = repo.head().ok()?;
    let commit = head.peel_to_commit().ok()?;
    Some(commit.id().to_string())
}
