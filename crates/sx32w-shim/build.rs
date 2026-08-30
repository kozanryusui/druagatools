fn main() {
    let target_os = std::env::var("CARGO_CFG_TARGET_OS").unwrap_or_default();
    let target_env = std::env::var("CARGO_CFG_TARGET_ENV").unwrap_or_default();

    if target_os == "windows" && target_env == "gnu" {
        // The 32-bit GNU linker adds stdcall suffixes such as `@4`.
        // The original executable imports the six names without these suffixes.
        println!("cargo::rustc-cdylib-link-arg=-Wl,--kill-at");
    }
}
