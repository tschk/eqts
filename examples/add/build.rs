fn main() {
    #[cfg(feature = "node-napi")]
    napi_build::setup();
}
