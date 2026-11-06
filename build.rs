fn main() {
    slint_build::compile("src/ui/main.slint").expect("Failed to compile Slint UI files");

    #[cfg(target_os = "windows")]
    {
        println!("cargo:rerun-if-changed=assets/app.ico");
        let mut resource = winresource::WindowsResource::new();
        resource.set_icon("assets/app.ico");
        resource.set("ProductName", "Gomo");
        resource.set("FileDescription", "Gomo - study timer");
        resource.set("OriginalFilename", "gomo.exe");
        resource.set("InternalName", "gomo");
        resource.set("CompanyName", "Gomo");
        if let Err(err) = resource.compile() {
            println!("cargo:warning=failed to embed Windows icon: {err}");
        }
    }
}
