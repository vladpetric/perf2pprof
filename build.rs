fn main() -> Result<(), Box<dyn std::error::Error>> {
    prost_build::Config::new().compile_protos(&["profile.proto"], &["."])?;
    println!("cargo:rerun-if-changed=profile.proto");
    Ok(())
}
