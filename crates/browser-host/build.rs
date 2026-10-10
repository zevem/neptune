fn main() {
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("linux") {
        // Chromium interposes close() and resolves libc with RTLD_NEXT.
        // Make CEF a direct, early dependency before Rust's libc users.
        println!("cargo:rustc-link-lib=dylib=cef");
        println!("cargo:rustc-link-arg=-Wl,-rpath,$ORIGIN");
    }
}
