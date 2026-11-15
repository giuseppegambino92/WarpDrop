fn main() {
    // Rebuild when the bundled translations change (they are `include_str!`-ed).
    println!("cargo:rerun-if-changed=../src/i18n/en.json");
    println!("cargo:rerun-if-changed=../src/i18n/it.json");
    tauri_build::build()
}
