//! Ensure that every WGSL shader parses and validates on the CPU.

use std::fs;

#[test]
fn wgsl_files_compile() {
    for entry in fs::read_dir("shaders").expect("read shaders dir") {
        let path = entry.expect("dir entry").path();
        if path.extension().and_then(|s| s.to_str()) != Some("wgsl") {
            continue;
        }

        let src = fs::read_to_string(&path).expect("read WGSL file");

        // Parse WGSL source using Naga (CPU implementation of WGSL frontend)
        let module = naga::front::wgsl::parse_str(&src).expect("WGSL parse");

        // Validate the module – catches type or binding errors without requiring a GPU device
        let mut validator = naga::valid::Validator::new(
            naga::valid::ValidationFlags::all(),
            naga::valid::Capabilities::all(),
        );

        validator.validate(&module).expect("WGSL validation");
    }
}
