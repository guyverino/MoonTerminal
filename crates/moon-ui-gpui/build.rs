fn main() {
    println!("cargo:rerun-if-changed=../../assets/icons/0.png");
    println!("cargo:rerun-if-changed=../../Cargo.lock");
    println!("cargo:rustc-check-cfg=cfg(moon_profile_debug)");
    emit_build_metadata();
    if std::env::var("PROFILE").is_ok_and(|profile| profile == "debug") {
        println!("cargo:rustc-cfg=moon_profile_debug");
    }

    #[cfg(windows)]
    if let Err(err) = embed_exe_icon() {
        println!("cargo:warning=failed to embed MoonTerminal exe icon: {err}");
    }
}

fn emit_build_metadata() {
    let manifest = std::path::PathBuf::from(
        std::env::var_os("CARGO_MANIFEST_DIR").expect("CARGO_MANIFEST_DIR must be set"),
    );
    let workspace = manifest
        .parent()
        .and_then(std::path::Path::parent)
        .expect("moon-ui-gpui must live under crates/");

    println!(
        "cargo:rustc-env=MOONTERMINAL_GIT_REV={}",
        git_rev(workspace).unwrap_or_else(|| "unknown".to_string())
    );
    println!(
        "cargo:rustc-env=MOONUI_GIT_REV={}",
        moonui_rev(workspace).unwrap_or_else(|| "unknown".to_string())
    );
}

fn moonui_rev(workspace: &std::path::Path) -> Option<String> {
    moonui_rev_from_lock(&workspace.join("Cargo.lock")).or_else(|| {
        let moonui = workspace.parent()?.join("MoonUI");
        if moonui.is_dir() {
            println!(
                "cargo:rerun-if-changed={}",
                moonui.join(".git").join("HEAD").display()
            );
        }
        git_rev(&moonui).map(|rev| format!("local:{rev}"))
    })
}

fn moonui_rev_from_lock(lock_path: &std::path::Path) -> Option<String> {
    let text = std::fs::read_to_string(lock_path).ok()?;
    let mut in_moon_gpui = false;
    for line in text.lines() {
        let line = line.trim();
        if line == "[[package]]" {
            in_moon_gpui = false;
            continue;
        }
        if line == "name = \"moon-gpui\"" {
            in_moon_gpui = true;
            continue;
        }
        if in_moon_gpui && line.starts_with("source = \"git+https://github.com/Moonbot-Tech/MoonUI")
        {
            return line
                .rsplit_once('#')
                .map(|(_, rev)| rev.trim_end_matches('"').to_string());
        }
    }
    None
}

fn git_rev(repo: &std::path::Path) -> Option<String> {
    if !repo.is_dir() {
        return None;
    }
    let output = std::process::Command::new("git")
        .args(["-C", repo.to_str()?, "rev-parse", "--short=12", "HEAD"])
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let mut rev = String::from_utf8(output.stdout).ok()?.trim().to_string();
    let dirty = std::process::Command::new("git")
        .args(["-C", repo.to_str()?, "status", "--porcelain"])
        .output()
        .ok()
        .filter(|out| out.status.success())
        .is_some_and(|out| !out.stdout.is_empty());
    if dirty {
        rev.push_str("+dirty");
    }
    Some(rev)
}

#[cfg(windows)]
fn embed_exe_icon() -> std::io::Result<()> {
    use std::fs::File;
    use std::path::Path;

    let png = File::open("../../assets/icons/0.png")?;
    let image = ico::IconImage::read_png(png)?;

    let mut dir = ico::IconDir::new(ico::ResourceType::Icon);
    dir.add_entry(ico::IconDirEntry::encode(&image)?);

    let out = Path::new(&std::env::var("OUT_DIR").expect("OUT_DIR")).join("moon-terminal.ico");
    dir.write(File::create(&out)?)?;

    let mut res = winresource::WindowsResource::new();
    res.set_icon(out.to_str().expect("icon path must be valid UTF-8"));
    res.compile()
}
