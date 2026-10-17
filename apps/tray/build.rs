fn main() {
    // Embed the entire matching Skill, including references, without a runtime checkout.
    let root = std::path::PathBuf::from(std::env::var_os("CARGO_MANIFEST_DIR").unwrap())
        .join("../../skills/agentnave-manager");
    println!("cargo:rerun-if-changed={}", root.display());
    let mut files = Vec::new();
    collect(&root, &root, &mut files);
    files.sort();
    let mut source = String::from("pub const FILES: &[(&str, &[u8])] = &[\n");
    for (name, path) in files {
        source.push_str(&format!("({name:?}, include_bytes!({path:?})),\n"));
    }
    source.push_str("];\n");
    std::fs::write(
        std::path::PathBuf::from(std::env::var_os("OUT_DIR").unwrap()).join("manager_skill.rs"),
        source,
    )
    .unwrap();
    tauri_build::build()
}

fn collect(root: &std::path::Path, dir: &std::path::Path, out: &mut Vec<(String, String)>) {
    for entry in std::fs::read_dir(dir).unwrap() {
        let path = entry.unwrap().path();
        assert!(
            !path.is_symlink(),
            "embedded Skill must contain regular files"
        );
        if path.is_dir() {
            collect(root, &path, out);
        } else {
            out.push((
                path.strip_prefix(root)
                    .unwrap()
                    .to_str()
                    .unwrap()
                    .replace('\\', "/"),
                path.to_str().unwrap().to_owned(),
            ));
        }
    }
}
