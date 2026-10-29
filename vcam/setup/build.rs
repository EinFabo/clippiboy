// Embeds the media source DLL named by VCAM_DLL. Without it an empty file is
// embedded, the tests still build, and `install` refuses.
fn main() {
    println!("cargo:rerun-if-env-changed=VCAM_DLL");
    let out = std::path::PathBuf::from(std::env::var("OUT_DIR").expect("OUT_DIR")).join("dll.bin");
    match std::env::var("VCAM_DLL") {
        Ok(path) => {
            println!("cargo:rerun-if-changed={path}");
            std::fs::copy(&path, &out).expect("VCAM_DLL names a file");
        }
        Err(_) => std::fs::write(&out, b"").expect("OUT_DIR is writable"),
    }
}
