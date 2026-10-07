fn main() {
    println!("cargo::rerun-if-changed=assets/icon.ico");

    #[cfg(windows)]
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows") {
        let mut res = winresource::WindowsResource::new();
        res.set_icon("assets/icon.ico");
        res.set("FileDescription", "Disk Usage");
        res.set("ProductName", "Disk Usage");
        res.compile().expect("failed to embed Windows resources");
    }
}
