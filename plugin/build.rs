fn main() {
    // XPLM symbols are provided by X-Plane at load time. On macOS the linker must be told
    // to leave them undefined; Linux allows it by default and Windows uses raw-dylib.
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("macos") {
        println!("cargo:rustc-cdylib-link-arg=-undefined");
        println!("cargo:rustc-cdylib-link-arg=dynamic_lookup");
    }
}
