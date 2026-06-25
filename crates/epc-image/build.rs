fn main() {
    println!("cargo:rerun-if-env-changed=PKG_CONFIG_PATH");
    println!("cargo:rerun-if-env-changed=JXL_NO_PKG_CONFIG");

    if std::env::var_os("CARGO_FEATURE_JXL_ENCODE_LIBJXL").is_none() {
        return;
    }

    if std::env::var_os("JXL_NO_PKG_CONFIG").is_none() {
        if pkg_config::Config::new()
            .atleast_version("0.11")
            .probe("libjxl")
            .is_ok()
        {
            let _ = pkg_config::Config::new()
                .atleast_version("0.11")
                .probe("libjxl_threads");
            return;
        }
    }

    println!("cargo:rustc-link-lib=jxl");
    println!("cargo:rustc-link-lib=jxl_threads");
}
