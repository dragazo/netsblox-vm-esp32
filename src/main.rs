use esp_idf_sys as _; // If using the `binstart` feature of `esp-idf-sys`, always keep this module imported

// use netsblox_vm::project::Project;
// use netsblox_vm::ast::Parser;

use esp_idf_svc::http::server::{EspHttpServer, EspHttpConnection};
use embedded_svc::http::Method;
use embedded_svc::http::server::{Handler, HandlerResult};
use embedded_svc::wifi::AuthMethod;

mod wifi;

struct RootHandler;
impl Handler<EspHttpConnection<'_>> for RootHandler {
    fn handle(&self, connection: &mut EspHttpConnection<'_>) -> HandlerResult {
        connection.write(b"it's working!")?;
        Ok(())
    }
}

fn main() {
    // It is necessary to call this function once. Otherwise some patches to the runtime
    // implemented by esp-idf-sys might not link properly. See https://github.com/esp-rs/esp-idf-template/issues/71
    esp_idf_sys::link_patches();

    let wifi = wifi::Wifi::new("nbvm-esp32c3", "password", AuthMethod::WPA2Personal).unwrap();

    let mut server = EspHttpServer::new(&Default::default()).unwrap();
    server.handler("/", Method::Get, RootHandler).unwrap();

    // let mut file = File::create("test.txt").unwrap();
    // file.write_all(b"hello world").unwrap();

    // let server = Server;


    // println!("before");
    // let ast = Parser::builder().build().unwrap().parse(include_str!("../test.xml")).unwrap();
    // println!("ast: {ast:?}");
    // println!("after");
}
