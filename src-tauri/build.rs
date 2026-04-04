fn main() {
    // Determine build profile: "dev", "prod", or "stage".
    // BUILD_PROFILE env var takes precedence; otherwise default based on cargo profile.
    let profile = std::env::var("BUILD_PROFILE").unwrap_or_else(|_| {
        let cargo_profile = std::env::var("PROFILE").unwrap_or_default();
        if cargo_profile == "release" {
            "prod"
        } else {
            "dev"
        }
        .to_string()
    });

    // Make BUILD_PROFILE available via env!("BUILD_PROFILE") in Rust code
    println!("cargo:rustc-env=BUILD_PROFILE={}", profile);
    println!("cargo:rerun-if-env-changed=BUILD_PROFILE");

    tauri_build::build()
}
