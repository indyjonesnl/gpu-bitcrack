//! Ensure that the WGSL shader parses and validates on the CPU.

#[test]
fn seq_wgsl_compiles() {
    let src = include_str!("../shaders/seq.wgsl");

    // Parse WGSL source using Naga (CPU implementation of WGSL frontend)
    let module = naga::front::wgsl::parse_str(src).expect("WGSL parse");

    // Validate the module – catches type or binding errors without requiring a GPU device
    let mut validator = naga::valid::Validator::new(
        naga::valid::ValidationFlags::all(),
        naga::valid::Capabilities::all(),
    );

    validator.validate(&module).expect("WGSL validation");
}