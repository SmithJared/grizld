fn main() {
    // Link CoreVideo framework on macOS for hardware decoding
    #[cfg(target_os = "macos")]
    {
        println!("cargo:rustc-link-lib=framework=CoreVideo");
        println!("cargo:rustc-link-lib=framework=VideoToolbox");
        println!("cargo:rustc-link-lib=framework=IOSurface");
    }
}
