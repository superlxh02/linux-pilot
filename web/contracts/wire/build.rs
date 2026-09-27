fn main() -> Result<(), Box<dyn std::error::Error>> {
    // vendored protoc 使 macOS 开发与 Linux Docker 构建使用同一份协议定义。
    let protoc = protoc_bin_vendored::protoc_bin_path()?;
    // SAFETY: 构建脚本在调用代码生成器之前单线程设置环境变量，没有并发读写。
    unsafe { std::env::set_var("PROTOC", protoc) };
    tonic_prost_build::compile_protos("proto/agent.proto")?;
    println!("cargo:rerun-if-changed=proto/agent.proto");
    Ok(())
}
