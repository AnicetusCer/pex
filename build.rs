fn main() {
    if std::env::var("CARGO_CFG_TARGET_OS")
        .map(|os| os == "windows")
        .unwrap_or(false)
    {
        let mut res = winresource::WindowsResource::new();
        res.set_icon("src/assets/PEX.ico");
        res
            .compile()
            .expect("Failed to embed Windows resources (icon)");
    }
}
