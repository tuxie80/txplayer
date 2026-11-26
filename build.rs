//! Build script: embeds the multi-resolution Windows application icon (`assets/TxPlayer.ico`)
//! and PE version metadata (`TxPlayer`) into `txplayer.exe` on Windows builds so Explorer,
//! Total Commander, Taskbar, Settings/Uninstall, and Open With display the TxPlayer icon everywhere.

fn main() {
    println!("cargo:rerun-if-changed=assets/TxPlayer.ico");
    println!("cargo:rerun-if-changed=Cargo.toml");

    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("windows") {
        return;
    }

    embed_windows_resources();
}

fn embed_windows_resources() {
    use std::path::{Path, PathBuf};
    use std::process::Command;

    let manifest_dir = PathBuf::from(
        std::env::var("CARGO_MANIFEST_DIR").expect("CARGO_MANIFEST_DIR not set"),
    );
    let out_dir = PathBuf::from(std::env::var("OUT_DIR").expect("OUT_DIR not set"));
    let ico_path = manifest_dir.join("assets").join("TxPlayer.ico");
    assert!(
        ico_path.is_file(),
        "Missing Windows icon asset at {}",
        ico_path.display()
    );

    let ver_str = std::env::var("CARGO_PKG_VERSION").unwrap_or_else(|_| "1.0.0".to_string());
    let mut parts = ver_str
        .split('.')
        .map(|s| s.parse::<u16>().unwrap_or(0));
    let maj = parts.next().unwrap_or(1);
    let min = parts.next().unwrap_or(0);
    let pat = parts.next().unwrap_or(0);

    // Escape backslashes for Windows RC string literal; use numeric VOS/VFT constants so zero SDK headers are needed.
    let ico_escaped = ico_path.to_string_lossy().replace('\\', "\\\\");
    let rc_content = format!(
        r#"1 ICON "{ico_escaped}"

1 VERSIONINFO
FILEVERSION {maj},{min},{pat},0
PRODUCTVERSION {maj},{min},{pat},0
FILEFLAGSMASK 0x3fL
FILEFLAGS 0x0L
FILEOS 0x00040004L
FILETYPE 0x00000001L
FILESUBTYPE 0x0L
BEGIN
    BLOCK "StringFileInfo"
    BEGIN
        BLOCK "040904b0"
        BEGIN
            VALUE "CompanyName", "Tuxie"
            VALUE "FileDescription", "TxPlayer - High-Resolution Music Player"
            VALUE "FileVersion", "{ver_str}"
            VALUE "InternalName", "txplayer"
            VALUE "LegalCopyright", "MIT License - Tuxie"
            VALUE "OriginalFilename", "txplayer.exe"
            VALUE "ProductName", "TxPlayer"
            VALUE "ProductVersion", "{ver_str}"
        END
    END
    BLOCK "VarFileInfo"
    BEGIN
        VALUE "Translation", 0x0409, 1200
    END
END
"#
    );

    let rc_path = out_dir.join("txplayer.rc");
    let res_path = out_dir.join("txplayer.res");
    std::fs::write(&rc_path, rc_content).expect("failed to write txplayer.rc");

    let target_env = std::env::var("CARGO_CFG_TARGET_ENV").unwrap_or_default();
    if target_env == "gnu" {
        let obj_path = out_dir.join("txplayer_res.o");
        let status = Command::new("windres")
            .arg(&rc_path)
            .arg("-O")
            .arg("coff")
            .arg("-o")
            .arg(&obj_path)
            .status()
            .expect("failed to invoke windres for Windows icon embedding");
        assert!(status.success(), "windres failed with status {status}");
        println!("cargo:rustc-link-arg-bin=txplayer={}", obj_path.display());
        return;
    }

    let rc_exe = find_rc_exe().expect(
        "Could not locate Windows SDK rc.exe to embed TxPlayer.ico into txplayer.exe",
    );
    let status = Command::new(&rc_exe)
        .arg("/nologo")
        .arg("/fo")
        .arg(&res_path)
        .arg(&rc_path)
        .status()
        .unwrap_or_else(|e| panic!("failed to run {}: {e}", rc_exe.display()));
    assert!(status.success(), "rc.exe failed with status {status}");
    println!("cargo:rustc-link-arg-bin=txplayer={}", res_path.display());

    fn find_rc_exe() -> Option<PathBuf> {
        if Command::new("rc.exe").arg("/?").output().is_ok() {
            return Some(PathBuf::from("rc.exe"));
        }
        let arch = match std::env::var("CARGO_CFG_TARGET_ARCH").as_deref() {
            Ok("x86") => "x86",
            Ok("aarch64") => "arm64",
            _ => "x64",
        };
        let kit_roots = [
            Path::new(r"C:\Program Files (x86)\Windows Kits\10\bin"),
            Path::new(r"C:\Program Files\Windows Kits\10\bin"),
        ];
        let mut candidates: Vec<PathBuf> = Vec::new();
        for root in kit_roots {
            if let Ok(entries) = std::fs::read_dir(root) {
                for entry in entries.flatten() {
                    let p = entry.path().join(arch).join("rc.exe");
                    if p.is_file() {
                        candidates.push(p);
                    }
                }
            }
            let direct = root.join(arch).join("rc.exe");
            if direct.is_file() {
                candidates.push(direct);
            }
        }
        candidates.sort();
        candidates.pop()
    }
}
