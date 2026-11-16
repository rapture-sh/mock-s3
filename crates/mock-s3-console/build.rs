use std::path::PathBuf;

/// Make sure `web/dist` exists before the crate is compiled.
///
/// `rust-embed` resolves its folder when the derive expands, so a missing directory is a
/// *compile* error for this crate and therefore for the whole workspace. That happens in
/// any checkout where nobody has run `pnpm build`.
///
/// A tracked placeholder file does not solve it: `vite build` is configured with
/// `emptyOutDir`, so every console build deletes the directory and takes the placeholder
/// with it, and the next `git add -A` quietly commits its removal. Creating the directory
/// here cannot be undone by a later build, and needs nothing committed.
fn main() {
    let manifest = PathBuf::from(
        std::env::var("CARGO_MANIFEST_DIR").expect("cargo always sets CARGO_MANIFEST_DIR"),
    );
    let dist = manifest.join("..").join("..").join("web").join("dist");

    if let Err(error) = std::fs::create_dir_all(&dist) {
        println!(
            "cargo::warning=could not create {}: {error}",
            dist.display()
        );
    }

    println!("cargo::rerun-if-changed=../../web/dist");
}
