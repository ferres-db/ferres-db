fn main() {
    #[cfg(feature = "grpc")]
    {
        tonic_build::configure()
            .build_server(true)
            .build_client(true)
            .compile_protos(&["proto/ferresdb.proto"], &["proto"])
            .expect("failed to compile proto/ferresdb.proto");
    }
}
