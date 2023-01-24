use esp_idf_sys as _; // If using the `binstart` feature of `esp-idf-sys`, always keep this module imported

use netsblox_vm::project::Project;
use netsblox_vm::ast::Parser;

fn main() {
    // It is necessary to call this function once. Otherwise some patches to the runtime
    // implemented by esp-idf-sys might not link properly. See https://github.com/esp-rs/esp-idf-template/issues/71
    esp_idf_sys::link_patches();

    println!("before");
    let ast = Parser::builder().build().unwrap().parse(include_str!("../test.xml")).unwrap();
    println!("ast: {ast:?}");
    println!("after");
}
