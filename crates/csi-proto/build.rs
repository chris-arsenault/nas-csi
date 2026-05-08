fn main() -> Result<(), Box<dyn std::error::Error>> {
    let protoc = protoc_bin_vendored::protoc_bin_path()?;
    // Build scripts are single-threaded for this process and tonic/prost read
    // PROTOC during code generation.
    unsafe {
        std::env::set_var("PROTOC", protoc);
    }

    tonic_prost_build::configure()
        .build_client(false)
        .build_server(true)
        .compile_protos(&["proto/csi.proto"], &["proto"])?;

    println!("cargo:rerun-if-changed=proto/csi.proto");
    Ok(())
}
