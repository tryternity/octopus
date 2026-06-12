fn main() {
    tauri_build::build();

    #[cfg(feature = "remote-grpc")]
    {
        let proto_path = "proto/asr.proto";
        tonic_build::configure()
            .build_server(false)
            .build_client(true)
            .compile_protos(&[proto_path], &["proto/"])
            .expect("Failed to compile protobuf");
    }
}
