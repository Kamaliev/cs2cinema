fn main() {
    println!("cargo:rerun-if-changed=proto");

    // protoc не нужно ставить руками: если PROTOC не задан, берём вшитый.
    if std::env::var_os("PROTOC").is_none() {
        let protoc = protoc_bin_vendored::protoc_bin_path().expect("не найден вшитый protoc");
        // SAFETY: build-скрипт однопоточный.
        unsafe { std::env::set_var("PROTOC", protoc) };
    }

    prost_build::compile_protos(&["proto/demo.proto", "proto/events.proto"], &["proto"])
        .expect("не удалось сгенерировать код из protobuf");
}
