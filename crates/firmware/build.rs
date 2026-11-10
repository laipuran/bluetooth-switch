fn main() {
    embuild::espidf::sysenv::output();
    // The USB HID driver lives in an extra managed component, so its cfg is
    // not known to rustc's built-in check-cfg list.
    println!("cargo::rustc-check-cfg=cfg(esp_idf_comp_espressif__usb_host_hid_enabled)");
}
