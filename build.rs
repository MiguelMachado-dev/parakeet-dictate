fn main() {
    println!("cargo:rerun-if-changed=assets/parakeet.ico");
    println!("cargo:rerun-if-changed=assets/parakeet-dictate.manifest");
    let mut resources = winresource::WindowsResource::new();
    resources.set_icon_with_id("assets/parakeet.ico", "1");
    resources.set_manifest_file("assets/parakeet-dictate.manifest");
    resources.compile().expect("the icon and manifest compile");
}
