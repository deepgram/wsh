use std::env;
use std::path::Path;
use std::process::Command;

fn main() {
    // Re-run when web sources change
    println!("cargo:rerun-if-changed=web/src/");
    println!("cargo:rerun-if-changed=web/index.html");
    println!("cargo:rerun-if-changed=web/package.json");
    println!("cargo:rerun-if-changed=web/vite.config.ts");
    println!("cargo:rerun-if-changed=web/tsconfig.json");

    if env::var("WSH_SKIP_WEB_BUILD").is_ok() {
        eprintln!("cargo:warning=WSH_SKIP_WEB_BUILD is set — skipping web frontend build");
        ensure_web_dist();
        return;
    }

    let bun = which_bun();

    let web_dir = Path::new("web");

    // bun install (only if node_modules is missing)
    if !web_dir.join("node_modules").exists() {
        eprintln!("  Installing web dependencies...");
        let status = Command::new(&bun)
            .args(["install"])
            .current_dir(web_dir)
            .status()
            .expect("failed to run `bun install`");

        if !status.success() {
            panic!(
                "\n\n\
                 error: `bun install` failed (exit {})\n\n\
                 Try running manually:\n\
                 \x20   cd web && bun install\n\n",
                status.code().map_or("signal".into(), |c| c.to_string())
            );
        }
    }

    // bun run build
    eprintln!("  Building web frontend...");
    let status = Command::new(&bun)
        .args(["run", "build"])
        .current_dir(web_dir)
        .status()
        .expect("failed to run `bun run build`");

    if !status.success() {
        panic!(
            "\n\n\
             error: `bun run build` failed (exit {})\n\n\
             Try running manually:\n\
             \x20   cd web && bun run build\n\n",
            status.code().map_or("signal".into(), |c| c.to_string())
        );
    }
}

/// Find bun or give a helpful error.
fn which_bun() -> String {
    // Allow override via env var
    if let Ok(path) = env::var("BUN") {
        return path;
    }

    // Check PATH
    if Command::new("bun")
        .arg("--version")
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
    {
        return "bun".into();
    }

    panic!(
        "\n\n\
         error: `bun` not found\n\n\
         The web frontend requires Bun to build.\n\n\
         Options:\n\
         \x20   1. Enter the nix dev shell:  nix develop\n\
         \x20   2. Install bun:              curl -fsSL https://bun.sh/install | bash\n\
         \x20   3. Skip the web build:       WSH_SKIP_WEB_BUILD=1 cargo build\n\
         \x20   4. Point to bun manually:    BUN=/path/to/bun cargo build\n\n"
    );
}

/// Ensure web-dist/ has at least an index.html so rust-embed doesn't fail.
fn ensure_web_dist() {
    let index = Path::new("web-dist/index.html");
    if !index.exists() {
        std::fs::create_dir_all("web-dist").expect("failed to create web-dist/");
        std::fs::write(
            index,
            "<html><body><p>Web UI not built. Run: cd web &amp;&amp; bun run build</p></body></html>\n",
        )
        .expect("failed to write placeholder index.html");
    }
}
