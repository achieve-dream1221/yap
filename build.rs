use std::{env, fs};

use copy_to_output::copy_to_output;

fn main() {
    let items_to_copy = [
        "example_configs/yap_colors.toml",
        "example_configs/yap_espflash_profiles.toml",
        "example_configs/yap_keybinds.toml",
        "example_configs/macros",
    ];

    for file in &items_to_copy {
        // Skip files that don't exist.
        if !fs::exists(file).unwrap() {
            continue;
        }
        copy_to_output(file, &env::var("PROFILE").unwrap())
            .unwrap_or_else(|_| panic!("Could not copy {file}"));
    }

    // Don't need to invalidate anything presently,
    // as all current files are read by the app after being compiled and ran.
    // for file in &items_to_copy {
    //     println!("cargo:rerun-if-changed={file}");
    // }
}
