use std::env;

use copy_to_output::copy_to_output;

fn main() {
    let items_to_copy = [
        "example_configs/yap_colors.toml",
        "example_configs/yap_espflash_profiles.toml",
        "example_configs/yap_keybinds.toml",
        "example_configs/macros",
    ];

    for file in &items_to_copy {
        copy_to_output(file, &env::var("PROFILE").unwrap()).expect("Could not copy");
    }

    // Invalidate the build if the files change
    for file in &items_to_copy {
        println!("cargo:rerun-if-changed={}", file);
    }
}
