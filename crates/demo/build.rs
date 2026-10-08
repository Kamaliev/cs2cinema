
fn main() {
    println!("cargo:rerun-if-changed=proto");

    prost_build::compile_protos(
        &["proto/demo.proto"],
        &["proto"],
    )
        .expect("не удалось сгенерировать код из protobuf");
}