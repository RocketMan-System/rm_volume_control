fn main() {
    slint_build::compile_with_config(
        "ui/app.slint",
        slint_build::CompilerConfiguration::new().with_style("fluent".into()),
    )
    .expect("failed to compile UI");

    // Build scripts run on the host, so check the target OS through Cargo instead of cfg!.
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows") {
        let mut res = winresource::WindowsResource::new();
        res.set_icon("assets/icon.ico");
        res.set("ProductName", "Apps Volume Control");
        res.set("FileDescription", "Apps Volume Control");
        if let Err(err) = res.compile() {
            println!("cargo:warning=failed to embed Windows resources: {err}");
        }
    }
}
