use std::path::PathBuf;

fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-changed=libs");

    // macOS-specific linking
    #[cfg(target_os = "macos")]
    {
        // Add rpath for Swift runtime libraries (required by screencapturekit crate)
        println!("cargo:rustc-link-arg=-Wl,-rpath,/usr/lib/swift");
        println!("cargo:rustc-link-arg=-Wl,-rpath,@executable_path/../Frameworks");

        // Add rpath for projectM libraries in the same directory as executable
        println!("cargo:rustc-link-arg=-Wl,-rpath,@executable_path");
        println!("cargo:rustc-link-arg=-Wl,-rpath,@executable_path/.");

        // Also try the libs directory relative to manifest
        let manifest_dir = std::env::var("CARGO_MANIFEST_DIR").unwrap();
        println!("cargo:rustc-link-arg=-Wl,-rpath,{}/libs", manifest_dir);

        // Weak link frameworks for macOS 26 (frameworks in dyld cache only)
        println!("cargo:rustc-link-arg=-Wl,-weak_framework,Metal");
        println!("cargo:rustc-link-arg=-Wl,-weak_framework,ScreenCaptureKit");
        println!("cargo:rustc-link-arg=-Wl,-weak_framework,QuartzCore");
        println!("cargo:rustc-link-arg=-Wl,-weak_framework,CoreMedia");
        println!("cargo:rustc-link-arg=-Wl,-weak_framework,CoreVideo");
        println!("cargo:rustc-link-arg=-Wl,-weak_framework,CoreGraphics");
        println!("cargo:rustc-link-arg=-Wl,-weak_framework,IOSurface");
        println!("cargo:rustc-link-arg=-Wl,-weak_framework,AVFoundation");

        // ProjectM: Replace projectm-sys built library with our 4.4 version that has FBO support
        // projectm-sys bundles 4.1.3 which lacks projectm_opengl_render_frame_fbo
        {
            let manifest_dir = std::env::var("CARGO_MANIFEST_DIR").unwrap();
            let out_dir = std::env::var("OUT_DIR").unwrap();
            let libs_dir = PathBuf::from(&manifest_dir).join("libs");

            // Find the projectm-sys output directory in the build tree
            let out_path = PathBuf::from(&out_dir);
            if let Some(build_dir) = out_path.parent().and_then(|p| p.parent()) {
                if let Ok(entries) = std::fs::read_dir(build_dir) {
                    for entry in entries.flatten() {
                        let name_str = entry.file_name().to_string_lossy().to_string();
                        if name_str.starts_with("projectm-sys-") {
                            let projectm_sys_lib = entry.path().join("out").join("lib");
                            if projectm_sys_lib.exists() {
                                // Source: our libs with v4.1.6 fixes (blur, Milkdrop 1.4, fwidth, etc.)
                                let src_main = libs_dir.join("libprojectM-4.4.dylib");
                                let src_playlist = libs_dir.join("libprojectM-4-playlist.4.dylib");

                                // Replace both debug and release versions that projectm-sys creates
                                let targets = [
                                    // Debug versions (cargo build)
                                    ("libprojectM-4d.4.1.3.dylib", "libprojectM-4-playlistd.4.1.3.dylib"),
                                    // Release versions (cargo build --release)
                                    ("libprojectM-4.4.1.3.dylib", "libprojectM-4-playlist.4.1.3.dylib"),
                                ];

                                for (main_name, playlist_name) in targets {
                                    let dst_main = projectm_sys_lib.join(main_name);
                                    let dst_playlist = projectm_sys_lib.join(playlist_name);

                                    // Replace with read+write to bypass macOS file attribute issues
                                    if src_main.exists() && dst_main.exists() {
                                        if let Ok(data) = std::fs::read(&src_main) {
                                            let _ = std::fs::write(&dst_main, &data);
                                        }
                                    }
                                    if src_playlist.exists() && dst_playlist.exists() {
                                        if let Ok(data) = std::fs::read(&src_playlist) {
                                            let _ = std::fs::write(&dst_playlist, &data);
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }
    }

    // === License verification key (from the environment, never the source tree) ===
    emit_license_key();
}

/// Emit the license-verification key consumed by `src/auth/license.rs`.
///
/// The key is deliberately NOT in the source tree. Official builds pass it in
/// as `REACT_LICENSE_HMAC_KEY` (hex); `build.sh` and `start-react.sh` load it
/// from the untracked `.env.local`. Without it the generated key is empty and
/// signed license blobs are rejected (see `verify_and_parse`).
///
/// The key is written XOR-split across two arrays so the plaintext never
/// appears contiguously in the binary — a speed bump, not crypto-grade.
fn emit_license_key() {
    println!("cargo:rerun-if-env-changed=REACT_LICENSE_HMAC_KEY");

    let key = match std::env::var("REACT_LICENSE_HMAC_KEY") {
        Ok(hex) if !hex.trim().is_empty() => decode_hex(hex.trim())
            .expect("REACT_LICENSE_HMAC_KEY must be an even-length hex string"),
        _ => Vec::new(),
    };

    // Deterministic pad (LCG), so identical inputs produce identical builds.
    let mut state: u32 = 0x2545_F491;
    let pad: Vec<u8> = (0..key.len())
        .map(|_| {
            state = state.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
            (state >> 24) as u8
        })
        .collect();
    let masked: Vec<u8> = key.iter().zip(&pad).map(|(k, p)| k ^ p).collect();

    let generated = format!(
        "// Generated by build.rs — do not edit\n\
         const HMAC_KEY_XOR: &[u8] = &{:?};\n\
         const HMAC_KEY_ROT: &[u8] = &{:?};\n",
        masked, pad
    );

    let out_path = PathBuf::from(std::env::var("OUT_DIR").unwrap()).join("license_key.rs");
    std::fs::write(&out_path, generated).expect("Failed to write license key");
}

fn decode_hex(s: &str) -> Option<Vec<u8>> {
    if s.len() % 2 != 0 {
        return None;
    }
    (0..s.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(s.get(i..i + 2)?, 16).ok())
        .collect()
}
