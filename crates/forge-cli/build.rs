use std::env;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

fn main() {
    let manifest_dir = PathBuf::from(env::var("CARGO_MANIFEST_DIR").expect("CARGO_MANIFEST_DIR"));
    let repo_root = manifest_dir
        .parent()
        .and_then(Path::parent)
        .expect("forge-cli is expected to live under crates/forge-cli");
    let web_dir = repo_root.join("web");

    println!("cargo:rerun-if-env-changed=FORGE_SKIP_WEB_BUILD");
    println!(
        "cargo:rerun-if-changed={}",
        web_dir.join("package.json").display()
    );
    println!(
        "cargo:rerun-if-changed={}",
        web_dir.join("pnpm-lock.yaml").display()
    );
    println!(
        "cargo:rerun-if-changed={}",
        web_dir.join("index.html").display()
    );
    println!(
        "cargo:rerun-if-changed={}",
        web_dir.join("vite.config.ts").display()
    );
    println!(
        "cargo:rerun-if-changed={}",
        web_dir.join("tsconfig.json").display()
    );
    println!(
        "cargo:rerun-if-changed={}",
        web_dir.join("tsconfig.app.json").display()
    );
    println!(
        "cargo:rerun-if-changed={}",
        web_dir.join("tsconfig.node.json").display()
    );
    println!(
        "cargo:rerun-if-changed={}",
        web_dir.join("tailwind.config.ts").display()
    );
    println!(
        "cargo:rerun-if-changed={}",
        web_dir.join("postcss.config.cjs").display()
    );
    println!(
        "cargo:rerun-if-changed={}",
        web_dir.join("public").display()
    );
    println!("cargo:rerun-if-changed={}", web_dir.join("src").display());

    if env::var_os("FORGE_SKIP_WEB_BUILD").is_some() {
        println!("cargo:warning=skipping frontend build because FORGE_SKIP_WEB_BUILD is set");
        return;
    }

    run(&web_dir, "pnpm", &["install", "--frozen-lockfile"]);
    run(&web_dir, "pnpm", &["run", "build"]);
}

fn run(web_dir: &Path, program: &str, args: &[&str]) {
    println!(
        "cargo:warning=running `{program} {}` in {}",
        args.join(" "),
        web_dir.display()
    );

    let status = Command::new(program)
        .args(args)
        .current_dir(web_dir)
        .stdin(Stdio::null())
        .status()
        .unwrap_or_else(|error| {
            panic!(
                "failed to run `{program}` in {}: {error}. Install pnpm or set FORGE_SKIP_WEB_BUILD=1 to build only the Rust CLI.",
                web_dir.display()
            )
        });

    if !status.success() {
        panic!("`{program} {}` failed with status {status}", args.join(" "));
    }
}
