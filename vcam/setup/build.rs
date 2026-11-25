// Embeds the media source DLL named by VCAM_DLL. Without it an empty file is
// embedded, the tests still build, and `install` refuses.
//
// The setup runs elevated from the app's resources folder, which the user can
// write. `SetDefaultDllDirectories` in `main` comes too late for the DLLs the
// program itself imports: Windows loads those before `main`, and one that is
// no Known DLL (Rust's std imports bcryptprimitives.dll) was looked for in the
// setup's own folder first. /DEPENDENTLOADFLAG puts LOAD_LIBRARY_SEARCH_SYSTEM32
// (0x800) into the image, so even those come from System32 only.
fn main() {
    println!("cargo:rerun-if-env-changed=VCAM_DLL");
    if std::env::var("CARGO_CFG_TARGET_ENV").as_deref() == Ok("msvc") {
        println!("cargo:rustc-link-arg-bins=/DEPENDENTLOADFLAG:0x800");
    }
    let out = std::path::PathBuf::from(std::env::var("OUT_DIR").expect("OUT_DIR")).join("dll.bin");
    match std::env::var("VCAM_DLL") {
        Ok(path) => {
            println!("cargo:rerun-if-changed={path}");
            std::fs::copy(&path, &out).expect("VCAM_DLL names a file");
        }
        Err(_) => std::fs::write(&out, b"").expect("OUT_DIR is writable"),
    }
}
