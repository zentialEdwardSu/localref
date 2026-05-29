fn main() {
    println!("cargo:rerun-if-changed=src/win32_native.cpp");

    if !cfg!(windows) {
        return;
    }

    cc::Build::new()
        .cpp(true)
        .std("c++17")
        .file("src/win32_native.cpp")
        .compile("localref_win32_native");

    println!("cargo:rustc-link-lib=shell32");
    println!("cargo:rustc-link-lib=ole32");
    println!("cargo:rustc-link-lib=propsys");
    println!("cargo:rustc-link-lib=runtimeobject");
    println!("cargo:rustc-link-lib=windowsapp");
}
