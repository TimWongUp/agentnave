fn main() {
    // Embed the entire matching Skills, including references, without a runtime checkout.
    let skills = std::path::PathBuf::from(std::env::var_os("CARGO_MANIFEST_DIR").unwrap())
        .join("../../skills");
    let mut source = String::new();
    for (constant, name) in [
        ("FILES", "agentnave-manager"),
        ("REVIEW_FILES", "agentnave-review"),
    ] {
        let root = skills.join(name);
        println!("cargo:rerun-if-changed={}", root.display());
        let mut files = Vec::new();
        collect(&root, &root, &mut files);
        files.sort();
        source.push_str(&format!("pub const {constant}: &[(&str, &[u8])] = &[\n"));
        for (name, path) in files {
            source.push_str(&format!("({name:?}, include_bytes!({path:?})),\n"));
        }
        source.push_str("];\n");
    }
    std::fs::write(
        std::path::PathBuf::from(std::env::var_os("OUT_DIR").unwrap()).join("manager_skill.rs"),
        source,
    )
    .unwrap();
    let art =
        std::path::PathBuf::from(std::env::var_os("CARGO_MANIFEST_DIR").unwrap()).join("ui/art");
    println!("cargo:rerun-if-changed={}", art.display());
    let mut files = Vec::new();
    if art.is_dir() {
        collect(&art, &art, &mut files);
    }
    files.sort();
    let mut source = String::from("pub const ART: &[(&str, &[u8])] = &[\n");
    for (name, path) in files {
        source.push_str(&format!("({name:?}, include_bytes!({path:?})),\n"));
    }
    source.push_str("];\n");
    std::fs::write(
        std::path::PathBuf::from(std::env::var_os("OUT_DIR").unwrap()).join("office_art.rs"),
        source,
    )
    .unwrap();
    write_embedded_wheel(&skills);
    tauri_build::build()
}

fn collect(root: &std::path::Path, dir: &std::path::Path, out: &mut Vec<(String, String)>) {
    println!("cargo:rerun-if-changed={}", dir.display());
    for entry in std::fs::read_dir(dir).unwrap() {
        let path = entry.unwrap().path();
        assert!(
            !path.is_symlink(),
            "embedded Skill must contain regular files"
        );
        let name = path.file_name().unwrap().to_string_lossy();
        if name.starts_with('.') || name == "__pycache__" || name.ends_with(".pyc") {
            continue;
        }
        if path.is_dir() {
            collect(root, &path, out);
        } else {
            println!("cargo:rerun-if-changed={}", path.display());
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

fn write_embedded_wheel(skills: &std::path::Path) {
    let manifest_dir = std::path::PathBuf::from(std::env::var_os("CARGO_MANIFEST_DIR").unwrap());
    let pkg = manifest_dir.join("../../src/agentnave");
    println!("cargo:rerun-if-changed={}", pkg.display());
    let version = std::env::var("CARGO_PKG_VERSION").unwrap();
    let mut entries: Vec<(String, Vec<u8>)> = Vec::new();
    let mut pkg_files = Vec::new();
    collect(&pkg, &pkg, &mut pkg_files);
    pkg_files.sort();
    for (rel, path) in pkg_files {
        entries.push((format!("agentnave/{rel}"), std::fs::read(path).unwrap()));
    }
    for skill in ["agentnave-manager", "agentnave-review"] {
        let root = skills.join(skill);
        let mut files = Vec::new();
        collect(&root, &root, &mut files);
        files.sort();
        for (rel, path) in files {
            entries.push((
                format!("agentnave/skills/{skill}/{rel}"),
                std::fs::read(path).unwrap(),
            ));
        }
    }
    let dist = format!("agentnave-{version}.dist-info");
    let metadata = format!(
        "Metadata-Version: 2.4\nName: agentnave\nVersion: {version}\nSummary: Local MCP server for CLI-based subagents with saved, reviewable multi-CLI meetings\nLicense-Expression: MIT\nRequires-Python: >=3.12\nRequires-Dist: mcp>=2.1.1,<3\n"
    );
    let wheel_meta =
        "Wheel-Version: 1.0\nGenerator: agentnave-tray\nRoot-Is-Purelib: true\nTag: py3-none-any\n";
    let entry_points = "[console_scripts]\nagentnave-mcp = agentnave.relay:main\n";
    entries.push((format!("{dist}/METADATA"), metadata.into_bytes()));
    entries.push((format!("{dist}/WHEEL"), wheel_meta.as_bytes().to_vec()));
    entries.push((
        format!("{dist}/entry_points.txt"),
        entry_points.as_bytes().to_vec(),
    ));
    let mut record = String::new();
    for (name, _) in &entries {
        record.push_str(name);
        record.push_str(",,\n");
    }
    record.push_str(&format!("{dist}/RECORD,,\n"));
    entries.push((format!("{dist}/RECORD"), record.into_bytes()));

    let zip = zip_stored(&entries);
    let mut framed = Vec::with_capacity(24 + version.len() + zip.len());
    framed.extend_from_slice(b"\x89ANWHEEL\r\n\x1a\n\x00\x00\x00\x01");
    framed.extend_from_slice(&(version.len() as u32).to_le_bytes());
    framed.extend_from_slice(version.as_bytes());
    framed.extend_from_slice(&(zip.len() as u32).to_le_bytes());
    framed.extend_from_slice(&zip);
    let out = std::path::PathBuf::from(std::env::var_os("OUT_DIR").unwrap()).join("agentnave.whl");
    std::fs::write(out, framed).unwrap();
}

fn crc32(bytes: &[u8]) -> u32 {
    let mut crc = 0xffff_ffffu32;
    for &byte in bytes {
        crc ^= u32::from(byte);
        for _ in 0..8 {
            crc = if crc & 1 != 0 {
                (crc >> 1) ^ 0xedb8_8320
            } else {
                crc >> 1
            };
        }
    }
    !crc
}

fn zip_stored(entries: &[(String, Vec<u8>)]) -> Vec<u8> {
    let mut body = Vec::new();
    let mut central = Vec::new();
    for (name, data) in entries {
        let offset = body.len() as u32;
        let crc = crc32(data);
        let size = data.len() as u32;
        let name_bytes = name.as_bytes();
        let name_len = name_bytes.len() as u16;
        body.extend_from_slice(&0x0403_4b50u32.to_le_bytes());
        body.extend_from_slice(&20u16.to_le_bytes());
        body.extend_from_slice(&0u16.to_le_bytes());
        body.extend_from_slice(&0u16.to_le_bytes());
        body.extend_from_slice(&0u16.to_le_bytes());
        body.extend_from_slice(&0x0021u16.to_le_bytes());
        body.extend_from_slice(&crc.to_le_bytes());
        body.extend_from_slice(&size.to_le_bytes());
        body.extend_from_slice(&size.to_le_bytes());
        body.extend_from_slice(&name_len.to_le_bytes());
        body.extend_from_slice(&0u16.to_le_bytes());
        body.extend_from_slice(name_bytes);
        body.extend_from_slice(data);

        central.extend_from_slice(&0x0201_4b50u32.to_le_bytes());
        central.extend_from_slice(&20u16.to_le_bytes());
        central.extend_from_slice(&20u16.to_le_bytes());
        central.extend_from_slice(&0u16.to_le_bytes());
        central.extend_from_slice(&0u16.to_le_bytes());
        central.extend_from_slice(&0u16.to_le_bytes());
        central.extend_from_slice(&0x0021u16.to_le_bytes());
        central.extend_from_slice(&crc.to_le_bytes());
        central.extend_from_slice(&size.to_le_bytes());
        central.extend_from_slice(&size.to_le_bytes());
        central.extend_from_slice(&name_len.to_le_bytes());
        central.extend_from_slice(&0u16.to_le_bytes());
        central.extend_from_slice(&0u16.to_le_bytes());
        central.extend_from_slice(&0u16.to_le_bytes());
        central.extend_from_slice(&0u16.to_le_bytes());
        central.extend_from_slice(&0u32.to_le_bytes());
        central.extend_from_slice(&offset.to_le_bytes());
        central.extend_from_slice(name_bytes);
    }
    let count = entries.len() as u16;
    let cd_offset = body.len() as u32;
    let cd_size = central.len() as u32;
    body.extend_from_slice(&central);
    body.extend_from_slice(&0x0605_4b50u32.to_le_bytes());
    body.extend_from_slice(&0u16.to_le_bytes());
    body.extend_from_slice(&0u16.to_le_bytes());
    body.extend_from_slice(&count.to_le_bytes());
    body.extend_from_slice(&count.to_le_bytes());
    body.extend_from_slice(&cd_size.to_le_bytes());
    body.extend_from_slice(&cd_offset.to_le_bytes());
    body.extend_from_slice(&0u16.to_le_bytes());
    body
}
