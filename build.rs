//! Embeds the icon, version info and manifest into the Windows executable.
//!
//! Uses `zig rc` when zig is on PATH (works for both the GNU and MSVC
//! toolchains), otherwise falls back to `embed-resource` (rc.exe / windres).
//! Shared by all Zenless desktop apps; only the constants below differ.

use std::{env, path::PathBuf, process::Command};

const PRODUCT: &str = "Zenless Download Manager";
const DESCRIPTION: &str = "Zenless Download Manager";

fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-changed=assets/icon.ico");
    println!("cargo:rerun-if-changed=assets/app.manifest");
    if env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("windows") {
        return;
    }

    let root = PathBuf::from(env::var("CARGO_MANIFEST_DIR").unwrap());
    let out = PathBuf::from(env::var("OUT_DIR").unwrap());
    let esc = |p: PathBuf| p.display().to_string().replace('\\', "\\\\");
    let version = env::var("CARGO_PKG_VERSION").unwrap();
    let mut parts: Vec<u16> = version.split(['.', '-']).filter_map(|s| s.parse().ok()).collect();
    parts.resize(4, 0);
    let bin_version = parts.iter().map(u16::to_string).collect::<Vec<_>>().join(",");
    let exe = format!("{}.exe", env::var("CARGO_PKG_NAME").unwrap());

    let rc = format!(
        r#"#pragma code_page(65001)
1 ICON "{icon}"
1 24 "{manifest}"
1 VERSIONINFO
FILEVERSION {bin_version}
PRODUCTVERSION {bin_version}
FILEOS 0x40004
FILETYPE 0x1
BEGIN
  BLOCK "StringFileInfo"
  BEGIN
    BLOCK "040904B0"
    BEGIN
      VALUE "CompanyName", "Zenless"
      VALUE "FileDescription", "{DESCRIPTION}"
      VALUE "FileVersion", "{version}"
      VALUE "InternalName", "{exe}"
      VALUE "LegalCopyright", "MIT License"
      VALUE "OriginalFilename", "{exe}"
      VALUE "ProductName", "{PRODUCT}"
      VALUE "ProductVersion", "{version}"
    END
  END
  BLOCK "VarFileInfo"
  BEGIN
    VALUE "Translation", 0x409, 1200
  END
END
"#,
        icon = esc(root.join("assets").join("icon.ico")),
        manifest = esc(root.join("assets").join("app.manifest")),
    );
    let rc_path = out.join("app.rc");
    std::fs::write(&rc_path, rc).expect("write app.rc");

    let obj = out.join("app-resources.o");
    let zig_ok = Command::new("zig")
        .arg("rc")
        .arg("/:output-format")
        .arg("coff")
        .arg("/:target")
        .arg(env::var("CARGO_CFG_TARGET_ARCH").unwrap_or_else(|_| "x86_64".into()))
        .arg(&rc_path)
        .arg(&obj)
        .status()
        .map(|s| s.success())
        .unwrap_or(false);
    if zig_ok {
        println!("cargo:rustc-link-arg-bins={}", obj.display());
    } else {
        let _ = embed_resource::compile(&rc_path, embed_resource::NONE);
    }
}
